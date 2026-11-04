use super::common::backup_file;
use super::common::mcp_argv;
use super::common::read_json_or_empty;
use super::common::write_json_preserving_mode;
use super::common::HostStatus;
use super::common::InstallReport;
use super::common::Target;
use std::path::Path;
use std::path::PathBuf;

fn opencode_config_path(home: &Path) -> PathBuf {
    home.join(".config").join("opencode").join("opencode.json")
}

fn opencode_plugin_dir(home: &Path) -> PathBuf {
    let singular = home.join(".config").join("opencode").join("plugin");
    let plural = home.join(".config").join("opencode").join("plugins");
    if singular.is_dir() || !plural.is_dir() {
        singular
    } else {
        plural
    }
}

pub const OPENCODE_PLUGIN_FILE: &str = "agent-memory.js";
pub const OPENCODE_PLUGIN_TEMPLATE: &str =
    include_str!("../../../../integrations/opencode/agent-memory-plugin.js");

pub fn render_opencode_plugin(target: &Target) -> String {
    let bin = serde_json::to_string(&target.binary.to_string_lossy())
        .unwrap_or("\"agent-memory\"".to_string());
    let home =
        serde_json::to_string(&target.data_home.to_string_lossy()).unwrap_or("\".\"".to_string());
    OPENCODE_PLUGIN_TEMPLATE
        .replacen("__LAM_BIN__", &bin, 1)
        .replacen("__LAM_DATA_HOME__", &home, 1)
}
pub fn install_opencode(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "opencode",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    let plugin_dir = opencode_plugin_dir(&target.home);
    std::fs::create_dir_all(&plugin_dir).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let plugin_path = plugin_dir.join(OPENCODE_PLUGIN_FILE);
    let rendered = render_opencode_plugin(target);
    let needs_write = std::fs::read_to_string(&plugin_path)
        .map(|existing| existing != rendered)
        .unwrap_or(true);
    if needs_write {
        if let Some(backup) = backup_file(&plugin_path)? {
            report.backups.push(backup);
        }
        std::fs::write(&plugin_path, rendered)
            .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        report.files_written.push(plugin_path);
        report.changed = true;
    }
    let config_path = opencode_config_path(&target.home);
    let mut config = read_json_or_empty(&config_path)?;
    let argv = mcp_argv(target);
    let wanted = serde_json::json!({
        "type": "local",
        "command": argv,
        "enabled": true,
    });
    let servers = config
        .as_object_mut()
        .and_then(|root| {
            root.entry("mcp".to_string())
                .or_insert_with(|| serde_json::json!({}));
            root.get_mut("mcp")
        })
        .and_then(|mcp| mcp.as_object_mut());
    if let Some(servers) = servers {
        if servers.get("agent-memory") != Some(&wanted) {
            if config_path.exists() {
                if let Some(backup) = backup_file(&config_path)? {
                    report.backups.push(backup);
                }
            } else if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            }
            servers.insert("agent-memory".to_string(), wanted);
            write_json_preserving_mode(&config_path, &config)?;
            report.files_written.push(config_path);
            report.changed = true;
        }
    }
    Ok(report)
}

pub fn uninstall_opencode(target: &Target) -> Result<InstallReport, lam_core::LamError> {
    let mut report = InstallReport {
        host: "opencode",
        files_written: Vec::new(),
        backups: Vec::new(),
        changed: false,
    };
    for candidate in [
        target
            .home
            .join(".config")
            .join("opencode")
            .join("plugin")
            .join(OPENCODE_PLUGIN_FILE),
        target
            .home
            .join(".config")
            .join("opencode")
            .join("plugins")
            .join(OPENCODE_PLUGIN_FILE),
    ] {
        if candidate.exists() {
            let text = std::fs::read_to_string(&candidate)
                .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            if text.contains("__LAM_BIN__")
                || text.contains(&target.binary.to_string_lossy() as &str)
            {
                std::fs::remove_file(&candidate)
                    .map_err(|e| lam_core::LamError::io(e.to_string()))?;
                report.changed = true;
            }
        }
    }
    let config_path = opencode_config_path(&target.home);
    if config_path.exists() {
        let mut config = read_json_or_empty(&config_path)?;
        let removed = config
            .as_object_mut()
            .and_then(|root| root.get_mut("mcp"))
            .and_then(|mcp| mcp.as_object_mut())
            .map(|servers| servers.remove("agent-memory").is_some())
            .unwrap_or(false);
        if removed {
            if let Some(backup) = backup_file(&config_path)? {
                report.backups.push(backup);
            }
            write_json_preserving_mode(&config_path, &config)?;
            report.files_written.push(config_path);
            report.changed = true;
        }
    }
    Ok(report)
}

pub fn status_opencode(target: &Target) -> Result<HostStatus, lam_core::LamError> {
    let plugin_present = [
        target
            .home
            .join(".config")
            .join("opencode")
            .join("plugin")
            .join(OPENCODE_PLUGIN_FILE),
        target
            .home
            .join(".config")
            .join("opencode")
            .join("plugins")
            .join(OPENCODE_PLUGIN_FILE),
    ]
    .iter()
    .any(|path| path.exists());
    let config = read_json_or_empty(&opencode_config_path(&target.home))?;
    let mcp_installed = config
        .get("mcp")
        .and_then(|mcp| mcp.get("agent-memory"))
        .is_some();
    Ok(HostStatus {
        host: "opencode",
        hooks_installed: plugin_present,
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

    fn seed_opencode(home: &std::path::Path) {
        let dir = home.join(".config").join("opencode");
        std::fs::create_dir_all(&dir).expect("opencode dir");
        std::fs::write(
            dir.join("opencode.json"),
            serde_json::json!({
                "$schema": "https://opencode.ai/config.json",
                "plugin": ["someone-else"],
                "mcp": {
                    "unrelated": {"type": "local", "command": ["/bin/unrelated"]},
                },
            })
            .to_string(),
        )
        .expect("seed opencode");
    }

    #[test]
    fn opencode_install_renders_plugin_and_mcp() {
        let home = tempfile::tempdir().expect("tempdir");
        seed_opencode(home.path());
        let target = target_in(home.path());
        install_opencode(&target).expect("install");
        let plugin_path = opencode_plugin_dir(&target.home).join(OPENCODE_PLUGIN_FILE);
        let rendered = std::fs::read_to_string(&plugin_path).expect("plugin");
        assert!(!rendered.contains("__LAM_BIN__"));
        assert!(!rendered.contains("__LAM_DATA_HOME__"));
        assert!(rendered.contains("/opt/lam/bin/agent-memory"));
        let config_text =
            std::fs::read_to_string(opencode_config_path(&target.home)).expect("config");
        assert!(config_text.contains("someone-else"));
        assert!(config_text.contains("unrelated"));
        let status = status_opencode(&target).expect("status");
        assert!(status.hooks_installed && status.mcp_installed);
        uninstall_opencode(&target).expect("uninstall");
        assert!(!plugin_path.exists());
        let config_after =
            std::fs::read_to_string(opencode_config_path(&target.home)).expect("config after");
        assert!(config_after.contains("someone-else"));
        assert!(!config_after.contains("agent-memory"));
    }
}
