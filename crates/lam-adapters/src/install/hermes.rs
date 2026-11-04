use super::common::backup_file;
use super::common::hook_command;
use super::common::mcp_argv;
use super::common::HostStatus;
use super::common::InstallReport;
use super::common::Target;
use std::path::Path;
use std::path::PathBuf;

fn hermes_config_path(home: &Path) -> PathBuf {
    home.join("config.yaml")
}

fn hermes_plugin_dir(home: &Path) -> PathBuf {
    home.join("plugins").join("agent-memory")
}

fn hermes_bridge_config_path(home: &Path) -> PathBuf {
    home.join("agent-memory.json")
}

const HERMES_PLUGIN_INIT: &str =
    include_str!("../../../../integrations/hermes/agent-memory/__init__.py");
const HERMES_PLUGIN_YAML: &str =
    include_str!("../../../../integrations/hermes/agent-memory/plugin.yaml");

fn hermes_hook_plan() -> Vec<(&'static str, &'static str, i64)> {
    vec![
        ("pre_llm_call", "context", 10),
        ("post_llm_call", "ingest", 10),
        ("post_tool_call", "ingest", 10),
        ("on_session_start", "context", 10),
        ("on_session_end", "ingest", 5),
        ("on_session_finalize", "ingest", 5),
        ("subagent_stop", "ingest", 5),
    ]
}

fn read_yaml_or_empty(path: &Path) -> Result<serde_yaml::Value, lam_core::LamError> {
    if !path.exists() {
        return Ok(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    }
    serde_yaml::from_str(&text).map_err(|e| {
        lam_core::LamError::validation(
            "E_HOST_CONFIG_INVALID",
            format!("{} is not valid YAML: {e}", path.to_string_lossy()),
        )
    })
}

fn write_yaml_preserving_mode(
    path: &Path,
    value: &serde_yaml::Value,
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
    let text = serde_yaml::to_string(value).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = previous_mode.unwrap_or(0o600);
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    Ok(())
}

fn mapping_mut(value: &mut serde_yaml::Value) -> Option<&mut serde_yaml::Mapping> {
    if value.is_null() {
        *value = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    value.as_mapping_mut()
}

fn is_our_command(text: &str) -> bool {
    text.contains("agent-memory") && (text.contains("hook ingest") || text.contains("hook context"))
}

fn merge_hermes_mcp(doc: &mut serde_yaml::Value, target: &Target) -> bool {
    let root = match mapping_mut(doc) {
        Some(found) => found,
        None => return false,
    };
    let key = serde_yaml::Value::String("mcp_servers".to_string());
    if root.get(&key).is_none() {
        root.insert(
            key.clone(),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    let servers = match root.get_mut(&key).and_then(|v| v.as_mapping_mut()) {
        Some(found) => found,
        None => return false,
    };
    let argv = mcp_argv(target);
    let mut args_seq = serde_yaml::Sequence::new();
    for item in argv.iter().skip(1) {
        args_seq.push(serde_yaml::Value::String(item.clone()));
    }
    let mut entry = serde_yaml::Mapping::new();
    entry.insert(
        serde_yaml::Value::String("command".to_string()),
        serde_yaml::Value::String(argv[0].clone()),
    );
    entry.insert(
        serde_yaml::Value::String("args".to_string()),
        serde_yaml::Value::Sequence(args_seq),
    );
    let name = serde_yaml::Value::String("agent-memory".to_string());
    let wanted = serde_yaml::Value::Mapping(entry);
    if servers.get(&name) == Some(&wanted) {
        return false;
    }
    servers.insert(name, wanted);
    true
}

fn merge_hermes_hooks(doc: &mut serde_yaml::Value, target: &Target) -> bool {
    let root = match mapping_mut(doc) {
        Some(found) => found,
        None => return false,
    };
    let key = serde_yaml::Value::String("hooks".to_string());
    if root.get(&key).is_none() {
        root.insert(
            key.clone(),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    let hooks = match root.get_mut(&key).and_then(|v| v.as_mapping_mut()) {
        Some(found) => found,
        None => return false,
    };
    let mut changed = false;
    for (event, sub, timeout) in hermes_hook_plan() {
        let command = hook_command(target, sub, "hermes", event);
        let event_key = serde_yaml::Value::String(event.to_string());
        if hooks.get(&event_key).is_none() {
            hooks.insert(
                event_key.clone(),
                serde_yaml::Value::Sequence(serde_yaml::Sequence::new()),
            );
        }
        let entries = match hooks.get_mut(&event_key).and_then(|v| v.as_sequence_mut()) {
            Some(found) => found,
            None => continue,
        };
        let mut matched = false;
        for entry in entries.iter_mut() {
            let current = entry.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if is_our_command(current) {
                matched = true;
                let timeout_value = serde_yaml::Value::Number(serde_yaml::Number::from(timeout));
                if current != command || entry.get("timeout") != Some(&timeout_value) {
                    if let Some(map) = entry.as_mapping_mut() {
                        map.insert(
                            serde_yaml::Value::String("command".to_string()),
                            serde_yaml::Value::String(command.clone()),
                        );
                        map.insert(
                            serde_yaml::Value::String("timeout".to_string()),
                            timeout_value,
                        );
                    }
                    changed = true;
                }
                break;
            }
        }
        if !matched {
            let mut item = serde_yaml::Mapping::new();
            item.insert(
                serde_yaml::Value::String("command".to_string()),
                serde_yaml::Value::String(command),
            );
            item.insert(
                serde_yaml::Value::String("timeout".to_string()),
                serde_yaml::Value::Number(serde_yaml::Number::from(timeout)),
            );
            entries.push(serde_yaml::Value::Mapping(item));
            changed = true;
        }
    }
    changed
}

fn write_bridge_config(target: &Target) -> Result<Option<PathBuf>, lam_core::LamError> {
    let path = hermes_bridge_config_path(&target.home);
    let wanted = serde_json::json!({
        "binary": target.binary.to_string_lossy(),
        "data_home": target.data_home.to_string_lossy(),
    });
    let current: Option<serde_json::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    if current.as_ref() == Some(&wanted) {
        return Ok(None);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    }
    let text =
        serde_json::to_string_pretty(&wanted).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    std::fs::write(&path, format!("{text}\n"))
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(Some(path))
}

fn install_plugin_files(target: &Target) -> Result<(Vec<PathBuf>, bool), lam_core::LamError> {
    let dir = hermes_plugin_dir(&target.home);
    std::fs::create_dir_all(&dir).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let mut written = Vec::new();
    let mut changed = false;
    for (name, content) in [
        ("__init__.py", HERMES_PLUGIN_INIT),
        ("plugin.yaml", HERMES_PLUGIN_YAML),
    ] {
        let path = dir.join(name);
        let needs_write = std::fs::read_to_string(&path)
            .map(|existing| existing != content)
            .unwrap_or(true);
        if needs_write {
            std::fs::write(&path, content).map_err(|e| lam_core::LamError::io(e.to_string()))?;
            written.push(path);
            changed = true;
        }
    }
    Ok((written, changed))
}

fn plugin_is_ours(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("__init__.py"))
        .map(|text| text.contains("AgentMemoryProvider") && text.contains("agent-memory"))
        .unwrap_or(false)
}

pub fn install_hermes(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "hermes",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    let config_path = hermes_config_path(&target.home);
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    }
    let had_config = config_path.exists();
    if let Some(backup) = backup_file(&config_path)? {
        report.backups.push(backup);
    }
    let mut doc = read_yaml_or_empty(&config_path)?;
    let mcp_changed = merge_hermes_mcp(&mut doc, target);
    let hooks_changed = merge_hermes_hooks(&mut doc, target);
    if mcp_changed || hooks_changed || !had_config {
        write_yaml_preserving_mode(&config_path, &doc)?;
        report.files_written.push(config_path);
        report.changed = true;
    }
    if let Some(path) = write_bridge_config(target)? {
        report.files_written.push(path);
        report.changed = true;
    }
    let (plugin_files, plugin_changed) = install_plugin_files(target)?;
    report.files_written.extend(plugin_files);
    if plugin_changed {
        report.changed = true;
    }
    Ok(report)
}

pub fn uninstall_hermes(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "hermes",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    let config_path = hermes_config_path(&target.home);
    if config_path.exists() {
        if let Some(backup) = backup_file(&config_path)? {
            report.backups.push(backup);
        }
        let mut doc = read_yaml_or_empty(&config_path)?;
        let mut changed = false;
        if let Some(root) = doc.as_mapping_mut() {
            let servers_key = serde_yaml::Value::String("mcp_servers".to_string());
            if let Some(servers) = root.get_mut(&servers_key).and_then(|v| v.as_mapping_mut()) {
                let name = serde_yaml::Value::String("agent-memory".to_string());
                if servers.remove(&name).is_some() {
                    changed = true;
                }
                if servers.is_empty() {
                    root.remove(&servers_key);
                }
            }
            let hooks_key = serde_yaml::Value::String("hooks".to_string());
            if let Some(hooks) = root.get_mut(&hooks_key).and_then(|v| v.as_mapping_mut()) {
                let events: Vec<serde_yaml::Value> = hooks.keys().cloned().collect();
                for event in events {
                    if let Some(entries) = hooks.get_mut(&event).and_then(|v| v.as_sequence_mut()) {
                        let before = entries.len();
                        entries.retain(|entry| {
                            let current =
                                entry.get("command").and_then(|v| v.as_str()).unwrap_or("");
                            !is_our_command(current)
                        });
                        if entries.len() != before {
                            changed = true;
                        }
                    }
                    let empty = hooks
                        .get(&event)
                        .and_then(|v| v.as_sequence())
                        .map(|s| s.is_empty())
                        .unwrap_or(false);
                    if empty {
                        hooks.remove(&event);
                        changed = true;
                    }
                }
                if hooks.is_empty() {
                    root.remove(&hooks_key);
                }
            }
        }
        if changed {
            write_yaml_preserving_mode(&config_path, &doc)?;
            report.files_written.push(config_path);
            report.changed = true;
        }
    }
    let bridge = hermes_bridge_config_path(&target.home);
    if bridge.exists() {
        std::fs::remove_file(&bridge).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        report.changed = true;
    }
    let plugin_dir = hermes_plugin_dir(&target.home);
    if plugin_dir.is_dir() && plugin_is_ours(&plugin_dir) {
        std::fs::remove_dir_all(&plugin_dir).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        report.changed = true;
    }
    Ok(report)
}

pub fn status_hermes(target: &Target) -> Result<HostStatus, lam_core::LamError> {
    let doc = read_yaml_or_empty(&hermes_config_path(&target.home))?;
    let mut hooks_installed = false;
    let mut mcp_installed = false;
    if let Some(root) = doc.as_mapping() {
        let servers_key = serde_yaml::Value::String("mcp_servers".to_string());
        mcp_installed = root
            .get(&servers_key)
            .and_then(|v| v.as_mapping())
            .map(|servers| {
                servers.contains_key(serde_yaml::Value::String("agent-memory".to_string()))
            })
            .unwrap_or(false);
        let hooks_key = serde_yaml::Value::String("hooks".to_string());
        if let Some(hooks) = root.get(&hooks_key).and_then(|v| v.as_mapping()) {
            'outer: for entries in hooks.values() {
                if let Some(items) = entries.as_sequence() {
                    for entry in items {
                        if entry
                            .get("command")
                            .and_then(|v| v.as_str())
                            .map(is_our_command)
                            .unwrap_or(false)
                        {
                            hooks_installed = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
    }
    let plugin_present = hermes_plugin_dir(&target.home).join("__init__.py").exists();
    Ok(HostStatus {
        host: "hermes",
        hooks_installed: hooks_installed && plugin_present,
        mcp_installed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_in(home: &std::path::Path) -> Target {
        Target {
            home: home.to_path_buf(),
            binary: PathBuf::from("/opt/lam/bin/agent-memory"),
            data_home: home.join("lam-data"),
        }
    }

    fn seed_hermes(home: &std::path::Path) {
        std::fs::create_dir_all(home).expect("hermes home");
        std::fs::write(
            home.join("config.yaml"),
            "memory:\n  provider: \"\"\n  memory_enabled: true\nmcp_servers:\n  unrelated:\n    command: \"/bin/unrelated\"\n    args: []\nhooks:\n  pre_tool_call:\n    - command: \"echo unrelated-keeper\"\n      timeout: 30\n",
        )
        .expect("seed hermes config");
    }

    #[test]
    fn hermes_install_preserves_unrelated_entries() {
        let home = tempfile::tempdir().expect("tempdir");
        let hermes_home = home.path().join("hermes-home");
        seed_hermes(&hermes_home);
        let target = target_in(&hermes_home);
        let first = install_hermes(&target).expect("install");
        assert!(first.changed);
        let second = install_hermes(&target).expect("reinstall");
        assert!(!second.changed);
        let config_text =
            std::fs::read_to_string(hermes_config_path(&target.home)).expect("config");
        assert!(config_text.contains("unrelated-keeper"));
        assert!(config_text.contains("/bin/unrelated"));
        assert!(config_text.contains("agent-memory"));
        assert!(config_text.contains("memory_enabled: true"));
        let status = status_hermes(&target).expect("status");
        assert!(status.hooks_installed && status.mcp_installed);
        assert!(hermes_plugin_dir(&target.home).join("__init__.py").exists());
        assert!(hermes_bridge_config_path(&target.home).exists());
        let removed = uninstall_hermes(&target).expect("uninstall");
        assert!(removed.changed);
        let after =
            std::fs::read_to_string(hermes_config_path(&target.home)).expect("config after");
        assert!(after.contains("unrelated-keeper"));
        assert!(!after.contains("agent-memory"));
        assert!(!hermes_plugin_dir(&target.home).exists());
        let second_remove = uninstall_hermes(&target).expect("second uninstall");
        assert!(!second_remove.changed);
    }

    #[test]
    fn hermes_install_creates_config_from_scratch() {
        let home = tempfile::tempdir().expect("tempdir");
        let hermes_home = home.path().join("fresh");
        let target = target_in(&hermes_home);
        install_hermes(&target).expect("install");
        let status = status_hermes(&target).expect("status");
        assert!(status.hooks_installed && status.mcp_installed);
        let doc = read_yaml_or_empty(&hermes_config_path(&target.home)).expect("yaml");
        let hooks = doc
            .get("hooks")
            .and_then(|v| v.as_mapping())
            .expect("hooks mapping");
        assert!(hooks.contains_key("pre_llm_call"));
        assert!(hooks.contains_key("post_tool_call"));
        assert!(hooks.contains_key("on_session_start"));
    }

    #[test]
    fn hermes_rejects_invalid_yaml() {
        let home = tempfile::tempdir().expect("tempdir");
        let hermes_home = home.path().join("broken");
        std::fs::create_dir_all(&hermes_home).expect("dir");
        std::fs::write(hermes_home.join("config.yaml"), "hooks: [unclosed\n").expect("seed");
        let target = target_in(&hermes_home);
        let result = install_hermes(&target);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_HOST_CONFIG_INVALID");
    }
}
