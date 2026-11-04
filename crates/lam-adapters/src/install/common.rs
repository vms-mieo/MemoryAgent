use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Target {
    pub home: PathBuf,
    pub binary: PathBuf,
    pub data_home: PathBuf,
}

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub host: &'static str,
    pub files_written: Vec<PathBuf>,
    pub backups: Vec<PathBuf>,
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct HostStatus {
    pub host: &'static str,
    pub hooks_installed: bool,
    pub mcp_installed: bool,
}

pub(crate) fn shell_quote(raw: &str) -> String {
    if raw.contains(' ') || raw.contains('"') || raw.contains('\'') {
        format!("\"{}\"", raw.replace('"', "\\\""))
    } else {
        raw.to_string()
    }
}

pub fn hook_command(target: &Target, sub: &str, host: &str, event: &str) -> String {
    format!(
        "{} --home {} hook {} --host {} --event {}",
        shell_quote(&target.binary.to_string_lossy()),
        shell_quote(&target.data_home.to_string_lossy()),
        sub,
        host,
        event
    )
}

pub fn mcp_argv(target: &Target) -> Vec<String> {
    vec![
        target.binary.to_string_lossy().to_string(),
        "--home".to_string(),
        target.data_home.to_string_lossy().to_string(),
        "mcp".to_string(),
    ]
}

pub(crate) fn is_our_hook_command(command: &str) -> bool {
    command.contains("agent-memory")
        && (command.contains("hook ingest") || command.contains("hook context"))
}

pub(crate) fn backup_file(path: &Path) -> Result<Option<PathBuf>, lam_core::LamError> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = path.with_extension(format!("bak-{stamp}"));
    std::fs::copy(path, &backup).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    Ok(Some(backup))
}

pub(crate) fn read_json_or_empty(path: &Path) -> Result<serde_json::Value, lam_core::LamError> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let text = std::fs::read_to_string(path).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&text).map_err(|e| {
        lam_core::LamError::validation(
            "E_HOST_CONFIG_INVALID",
            format!("{} is not valid JSON: {e}", path.to_string_lossy()),
        )
    })
}

pub(crate) fn write_json_preserving_mode(
    path: &Path,
    value: &serde_json::Value,
) -> Result<(), lam_core::LamError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        }
    }
    #[cfg(unix)]
    let previous_mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
    };
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    std::fs::write(path, format!("{text}\n")).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = previous_mode.unwrap_or(0o600);
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    Ok(())
}
pub(crate) fn merge_hook_handler(
    hooks_root: &mut serde_json::Value,
    event: &str,
    command: &str,
    timeout: u64,
    matcher: Option<&str>,
) -> bool {
    let events = hooks_root
        .as_object_mut()
        .and_then(|root| root.get_mut("hooks"))
        .and_then(|hooks| hooks.as_object_mut());
    let events = match events {
        Some(found) => found,
        None => return false,
    };
    let groups = events
        .entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]));
    let groups = match groups.as_array_mut() {
        Some(found) => found,
        None => return false,
    };
    for group in groups.iter_mut() {
        let group_matcher = group.get("matcher").and_then(|v| v.as_str());
        let matcher_fits = match (matcher, group_matcher) {
            (None, None) => true,
            (Some(wanted), Some(existing)) => wanted == existing,
            _ => false,
        };
        if !matcher_fits {
            continue;
        }
        let handlers = match group
            .get_mut("hooks")
            .and_then(|handlers| handlers.as_array_mut())
        {
            Some(found) => found,
            None => continue,
        };
        for handler in handlers.iter_mut() {
            let current = handler
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if is_our_hook_command(current) {
                let changed = current != command
                    || handler.get("timeout").and_then(|v| v.as_u64()) != Some(timeout);
                handler["command"] = serde_json::json!(command);
                handler["timeout"] = serde_json::json!(timeout);
                return changed;
            }
        }
        let mut entry = serde_json::json!({
            "type": "command",
            "command": command,
            "timeout": timeout,
        });
        if command.contains("context") {
            entry["statusMessage"] = serde_json::json!("Loading agent memory");
        }
        handlers.push(entry);
        return true;
    }
    let mut group = serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
        }],
    });
    if let Some(wanted) = matcher {
        group["matcher"] = serde_json::json!(wanted);
    }
    groups.push(group);
    true
}

pub(crate) fn remove_our_handlers(hooks_root: &mut serde_json::Value) -> bool {
    let mut changed = false;
    let events = hooks_root
        .as_object_mut()
        .and_then(|root| root.get_mut("hooks"))
        .and_then(|hooks| hooks.as_object_mut());
    let events = match events {
        Some(found) => found,
        None => return false,
    };
    let names: Vec<String> = events.keys().cloned().collect();
    for name in names {
        let groups = match events.get_mut(&name).and_then(|v| v.as_array_mut()) {
            Some(found) => found,
            None => continue,
        };
        let handlers_before: usize = groups
            .iter()
            .filter_map(|group| {
                group
                    .get("hooks")
                    .and_then(|v| v.as_array())
                    .map(|h| h.len())
            })
            .sum();
        let groups_before = groups.len();
        let mut keep_groups: Vec<serde_json::Value> = Vec::new();
        for group in groups.drain(..) {
            let mut group = group;
            if let Some(handlers) = group.get_mut("hooks").and_then(|v| v.as_array_mut()) {
                handlers.retain(|handler| {
                    let current = handler
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    !is_our_hook_command(current)
                });
                if handlers.is_empty() && group.get("matcher").is_none() {
                    continue;
                }
            }
            keep_groups.push(group);
        }
        let handlers_after: usize = keep_groups
            .iter()
            .filter_map(|group| {
                group
                    .get("hooks")
                    .and_then(|v| v.as_array())
                    .map(|h| h.len())
            })
            .sum();
        if handlers_after != handlers_before || keep_groups.len() != groups_before {
            changed = true;
        }
        if keep_groups.is_empty() {
            events.remove(&name);
            changed = true;
        } else {
            events.insert(name, serde_json::Value::Array(keep_groups));
        }
    }
    changed
}

pub(crate) fn count_our_handlers(hooks_root: &serde_json::Value) -> usize {
    let mut count = 0;
    if let Some(events) = hooks_root.get("hooks").and_then(|v| v.as_object()) {
        for groups in events.values() {
            if let Some(groups) = groups.as_array() {
                for group in groups {
                    if let Some(handlers) = group.get("hooks").and_then(|v| v.as_array()) {
                        for handler in handlers {
                            if handler
                                .get("command")
                                .and_then(|v| v.as_str())
                                .map(is_our_hook_command)
                                .unwrap_or(false)
                            {
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    count
}

pub(crate) fn ensure_hooks_object(root: &mut serde_json::Value) {
    if root.get("hooks").is_none() {
        if let Some(map) = root.as_object_mut() {
            map.insert("hooks".to_string(), serde_json::json!({}));
        }
    }
}
