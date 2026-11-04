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

fn codex_hooks_path(home: &Path) -> PathBuf {
    home.join(".codex").join("hooks.json")
}

fn codex_config_path(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}
fn codex_plan() -> Vec<(&'static str, &'static str, u64, Option<&'static str>)> {
    vec![
        ("SessionStart", "context", 10, None),
        ("UserPromptSubmit", "context", 10, None),
        (
            "PostToolUse",
            "ingest",
            10,
            Some("Bash|apply_patch|Edit|Write"),
        ),
        ("PreCompact", "ingest", 10, None),
        ("PostCompact", "ingest", 10, None),
        ("Stop", "ingest", 10, None),
        ("SessionEnd", "ingest", 3, None),
    ]
}
pub fn install_codex(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "codex",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    let hooks_path = codex_hooks_path(&target.home);
    if let Some(parent) = hooks_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    }
    if let Some(backup) = backup_file(&hooks_path)? {
        report.backups.push(backup);
    }
    let mut hooks_doc = read_json_or_empty(&hooks_path)?;
    ensure_hooks_object(&mut hooks_doc);
    for (event, sub, timeout, matcher) in codex_plan() {
        let command = hook_command(target, sub, "codex", event);
        if merge_hook_handler(&mut hooks_doc, event, &command, timeout, matcher) {
            report.changed = true;
        }
    }
    if report.changed {
        write_json_preserving_mode(&hooks_path, &hooks_doc)?;
        report.files_written.push(hooks_path);
    }
    let config_path = codex_config_path(&target.home);
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    }
    let (changed, rewrote) = merge_codex_mcp(target, &config_path)?;
    if let Some(backup) = rewrote {
        report.backups.push(backup);
    }
    if changed {
        report.files_written.push(config_path);
        report.changed = true;
    }
    Ok(report)
}

fn merge_codex_mcp(
    target: &Target,
    config_path: &Path,
) -> Result<(bool, Option<PathBuf>), lam_core::LamError> {
    use toml_edit::DocumentMut;
    let existing = if config_path.exists() {
        Some(
            std::fs::read_to_string(config_path)
                .map_err(|e| lam_core::LamError::io(e.to_string()))?,
        )
    } else {
        None
    };
    let mut backup = None;
    if existing.is_some() {
        backup = backup_file(config_path)?;
    }
    let mut document: DocumentMut = existing.as_deref().unwrap_or("").parse().map_err(|e| {
        lam_core::LamError::validation(
            "E_HOST_CONFIG_INVALID",
            format!("{} is not valid TOML: {e}", config_path.to_string_lossy()),
        )
    })?;
    let argv = mcp_argv(target);
    let current_command = document
        .get("mcp_servers")
        .and_then(|servers| servers.get("agent-memory"))
        .and_then(|server| server.get("command"))
        .and_then(|command| command.as_str())
        .unwrap_or("")
        .to_string();
    let current_args: Vec<String> = document
        .get("mcp_servers")
        .and_then(|servers| servers.get("agent-memory"))
        .and_then(|server| server.get("args"))
        .and_then(|args| args.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let wanted_args: Vec<String> = argv.iter().skip(1).cloned().collect();
    if current_command == argv[0] && current_args == wanted_args {
        return Ok((false, backup));
    }
    document["mcp_servers"]["agent-memory"]["command"] = toml_edit::value(argv[0].clone());
    let mut args_array = toml_edit::Array::new();
    for item in wanted_args {
        args_array.push(item);
    }
    document["mcp_servers"]["agent-memory"]["args"] = toml_edit::value(args_array);
    std::fs::write(config_path, document.to_string())
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    Ok((true, backup))
}

pub fn uninstall_codex(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "codex",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    let hooks_path = codex_hooks_path(&target.home);
    if hooks_path.exists() {
        if let Some(backup) = backup_file(&hooks_path)? {
            report.backups.push(backup);
        }
        let mut hooks_doc = read_json_or_empty(&hooks_path)?;
        if remove_our_handlers(&mut hooks_doc) {
            write_json_preserving_mode(&hooks_path, &hooks_doc)?;
            report.files_written.push(hooks_path);
            report.changed = true;
        }
    }
    let config_path = codex_config_path(&target.home);
    if config_path.exists() {
        use toml_edit::DocumentMut;
        let text = std::fs::read_to_string(&config_path)
            .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        let mut document: DocumentMut = text.parse().map_err(|e| {
            lam_core::LamError::validation(
                "E_HOST_CONFIG_INVALID",
                format!("{} is not valid TOML: {e}", config_path.to_string_lossy()),
            )
        })?;
        let present = document
            .get("mcp_servers")
            .map(|servers| {
                let in_table = servers
                    .as_table()
                    .and_then(|table| table.get("agent-memory"))
                    .is_some();
                let in_inline = servers
                    .as_inline_table()
                    .and_then(|table| table.get("agent-memory"))
                    .is_some();
                in_table || in_inline
            })
            .unwrap_or(false);
        if present {
            if let Some(backup) = backup_file(&config_path)? {
                report.backups.push(backup);
            }
            if let Some(servers) = document.get_mut("mcp_servers") {
                if let Some(table) = servers.as_table_mut() {
                    table.remove("agent-memory");
                }
                if let Some(table) = servers.as_inline_table_mut() {
                    table.remove("agent-memory");
                }
            }
            let empty = document
                .get("mcp_servers")
                .map(|servers| {
                    let table_empty = servers
                        .as_table()
                        .map(|table| table.is_empty())
                        .unwrap_or(false);
                    let inline_empty = servers
                        .as_inline_table()
                        .map(|table| table.is_empty())
                        .unwrap_or(false);
                    table_empty || inline_empty
                })
                .unwrap_or(false);
            if empty {
                document.as_table_mut().remove("mcp_servers");
            }
            std::fs::write(&config_path, document.to_string())
                .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            report.files_written.push(config_path);
            report.changed = true;
        }
    }
    Ok(report)
}

pub fn status_codex(target: &Target) -> Result<HostStatus, lam_core::LamError> {
    let hooks_doc = read_json_or_empty(&codex_hooks_path(&target.home))?;
    let hooks_installed = count_our_handlers(&hooks_doc) > 0;
    let mcp_installed = codex_config_path(&target.home)
        .exists()
        .then(|| std::fs::read_to_string(codex_config_path(&target.home)).unwrap_or_default())
        .and_then(|text| {
            text.parse::<toml_edit::DocumentMut>().ok().map(|document| {
                document
                    .get("mcp_servers")
                    .map(|servers| {
                        let in_table = servers
                            .as_table()
                            .and_then(|table| table.get("agent-memory"))
                            .is_some();
                        let in_inline = servers
                            .as_inline_table()
                            .and_then(|table| table.get("agent-memory"))
                            .is_some();
                        in_table || in_inline
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    Ok(HostStatus {
        host: "codex",
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

    fn seed_codex(home: &std::path::Path) {
        let dir = home.join(".codex");
        std::fs::create_dir_all(&dir).expect("codex dir");
        std::fs::write(
            dir.join("hooks.json"),
            serde_json::json!({
                "hooks": {
                    "SessionStart": [{
                        "hooks": [{
                            "type": "command",
                            "command": "echo unrelated-keeper",
                        }],
                    }],
                },
            })
            .to_string(),
        )
        .expect("seed hooks");
        std::fs::write(
            dir.join("config.toml"),
            "[mcp_servers.unrelated]\ncommand = \"/bin/unrelated\"\n",
        )
        .expect("seed config");
    }

    #[test]
    fn codex_install_preserves_unrelated_entries() {
        let home = tempfile::tempdir().expect("tempdir");
        seed_codex(home.path());
        let target = target_in(home.path());
        let first = install_codex(&target).expect("install");
        assert!(first.changed);
        let second = install_codex(&target).expect("reinstall");
        assert!(!second.changed);
        let hooks_text = std::fs::read_to_string(codex_hooks_path(&target.home)).expect("hooks");
        assert!(hooks_text.contains("echo unrelated-keeper"));
        assert!(hooks_text.contains("agent-memory"));
        let config_text = std::fs::read_to_string(codex_config_path(&target.home)).expect("config");
        assert!(config_text.contains("unrelated"));
        assert!(config_text.contains("agent-memory"));
        let status = status_codex(&target).expect("status");
        assert!(status.hooks_installed && status.mcp_installed);
        let removed = uninstall_codex(&target).expect("uninstall");
        assert!(removed.changed);
        let hooks_after =
            std::fs::read_to_string(codex_hooks_path(&target.home)).expect("hooks after");
        assert!(hooks_after.contains("echo unrelated-keeper"));
        assert!(!hooks_after.contains("agent-memory"));
        let config_after =
            std::fs::read_to_string(codex_config_path(&target.home)).expect("config after");
        assert!(config_after.contains("unrelated"));
        assert!(!config_after.contains("agent-memory"));
    }
}
