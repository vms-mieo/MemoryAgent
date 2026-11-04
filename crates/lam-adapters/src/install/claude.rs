use super::common::backup_file;
use super::common::count_our_handlers;
use super::common::ensure_hooks_object;
use super::common::hook_command;
use super::common::mcp_argv;
use super::common::merge_hook_handler;
use super::common::read_json_or_empty;
use super::common::remove_our_handlers;
use super::common::write_json_preserving_mode;
use super::common::HostStatus;
use super::common::InstallReport;
use super::common::Target;
use std::path::Path;
use std::path::PathBuf;

fn claude_settings_path(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

fn claude_mcp_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}
fn claude_plan() -> Vec<(&'static str, &'static str, u64)> {
    vec![
        ("SessionStart", "context", 10),
        ("UserPromptSubmit", "context", 10),
        ("PostToolUse", "ingest", 10),
        ("PreCompact", "ingest", 10),
        ("Stop", "ingest", 10),
        ("SessionEnd", "ingest", 5),
    ]
}
pub fn install_claude(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "claude-code",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    let settings_path = claude_settings_path(&target.home);
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    }
    if let Some(backup) = backup_file(&settings_path)? {
        report.backups.push(backup);
    }
    let mut settings = read_json_or_empty(&settings_path)?;
    ensure_hooks_object(&mut settings);
    for (event, sub, timeout) in claude_plan() {
        let command = hook_command(target, sub, "claude-code", event);
        if merge_hook_handler(&mut settings, event, &command, timeout, None) {
            report.changed = true;
        }
    }
    if report.changed {
        write_json_preserving_mode(&settings_path, &settings)?;
        report.files_written.push(settings_path);
    }
    let mcp_path = claude_mcp_path(&target.home);
    let mut mcp_doc = read_json_or_empty(&mcp_path)?;
    let argv = mcp_argv(target);
    let servers = mcp_doc
        .as_object_mut()
        .and_then(|root| {
            root.entry("mcpServers".to_string())
                .or_insert_with(|| serde_json::json!({}));
            root.get_mut("mcpServers")
        })
        .and_then(|servers| servers.as_object_mut());
    if let Some(servers) = servers {
        let wanted = serde_json::json!({
            "type": "stdio",
            "command": argv[0],
            "args": argv.iter().skip(1).collect::<Vec<_>>(),
            "env": {},
        });
        if servers.get("agent-memory") != Some(&wanted) {
            if mcp_path.exists() {
                if let Some(backup) = backup_file(&mcp_path)? {
                    report.backups.push(backup);
                }
            } else if let Some(parent) = mcp_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            }
            servers.insert("agent-memory".to_string(), wanted);
            write_json_preserving_mode(&mcp_path, &mcp_doc)?;
            report.files_written.push(mcp_path);
            report.changed = true;
        }
    }
    Ok(report)
}

pub fn uninstall_claude(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "claude-code",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    let settings_path = claude_settings_path(&target.home);
    if settings_path.exists() {
        if let Some(backup) = backup_file(&settings_path)? {
            report.backups.push(backup);
        }
        let mut settings = read_json_or_empty(&settings_path)?;
        if remove_our_handlers(&mut settings) {
            write_json_preserving_mode(&settings_path, &settings)?;
            report.files_written.push(settings_path.clone());
            report.changed = true;
        }
    }
    let mcp_path = claude_mcp_path(&target.home);
    if mcp_path.exists() {
        let mut mcp_doc = read_json_or_empty(&mcp_path)?;
        let removed = mcp_doc
            .as_object_mut()
            .and_then(|root| root.get_mut("mcpServers"))
            .and_then(|servers| servers.as_object_mut())
            .map(|servers| servers.remove("agent-memory").is_some())
            .unwrap_or(false);
        if removed {
            if let Some(backup) = backup_file(&mcp_path)? {
                report.backups.push(backup);
            }
            write_json_preserving_mode(&mcp_path, &mcp_doc)?;
            report.files_written.push(mcp_path);
            report.changed = true;
        }
    }
    Ok(report)
}

pub fn status_claude(target: &Target) -> Result<HostStatus, lam_core::LamError> {
    let settings = read_json_or_empty(&claude_settings_path(&target.home))?;
    let hooks_installed = count_our_handlers(&settings) > 0;
    let mcp_doc = read_json_or_empty(&claude_mcp_path(&target.home))?;
    let mcp_installed = mcp_doc
        .get("mcpServers")
        .and_then(|servers| servers.get("agent-memory"))
        .is_some();
    Ok(HostStatus {
        host: "claude-code",
        hooks_installed,
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

    fn seed_claude(home: &std::path::Path) {
        let dir = home.join(".claude");
        std::fs::create_dir_all(&dir).expect("claude dir");
        std::fs::write(
            dir.join("settings.json"),
            serde_json::json!({
                "model": "opus",
                "permissions": {"allow": ["Bash(echo *)"]},
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": "rtk hook claude"}],
                    }],
                },
            })
            .to_string(),
        )
        .expect("seed settings");
        std::fs::write(
            home.join(".claude.json"),
            serde_json::json!({
                "userID": "fixture-user",
                "mcpServers": {
                    "unrelated": {"type": "stdio", "command": "/bin/unrelated", "args": []},
                },
            })
            .to_string(),
        )
        .expect("seed claude json");
    }

    #[test]
    fn claude_install_preserves_unrelated_entries() {
        let home = tempfile::tempdir().expect("tempdir");
        seed_claude(home.path());
        let target = target_in(home.path());
        install_claude(&target).expect("install");
        install_claude(&target).expect("reinstall");
        let settings_text =
            std::fs::read_to_string(claude_settings_path(&target.home)).expect("settings");
        assert!(settings_text.contains("rtk hook claude"));
        assert!(settings_text.contains("\"model\": \"opus\""));
        assert!(settings_text.contains("agent-memory"));
        let mcp_text = std::fs::read_to_string(claude_mcp_path(&target.home)).expect("mcp");
        assert!(mcp_text.contains("unrelated"));
        assert!(mcp_text.contains("fixture-user"));
        let status = status_claude(&target).expect("status");
        assert!(status.hooks_installed && status.mcp_installed);
        uninstall_claude(&target).expect("uninstall");
        let settings_after =
            std::fs::read_to_string(claude_settings_path(&target.home)).expect("settings after");
        assert!(settings_after.contains("rtk hook claude"));
        assert!(!settings_after.contains("agent-memory"));
    }
}
