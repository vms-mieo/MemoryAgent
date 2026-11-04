use crate::Host;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedHost {
    pub host: Host,
    pub config_present: bool,
    pub binary_present: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DetectInputs<'a> {
    pub home: &'a Path,
    pub path: Option<&'a str>,
    pub xdg_config_home: Option<&'a Path>,
    pub hermes_home: Option<&'a Path>,
}

pub fn detect_hosts(inputs: DetectInputs<'_>) -> Vec<DetectedHost> {
    let mut found = Vec::new();
    for host in Host::ALL {
        let config_present = match host {
            Host::Codex => inputs.home.join(".codex").is_dir(),
            Host::ClaudeCode => {
                inputs.home.join(".claude").is_dir() || inputs.home.join(".claude.json").is_file()
            }
            Host::OpenCode => opencode_config_present(inputs.home, inputs.xdg_config_home),
            Host::Hermes => hermes_config_present(inputs.home, inputs.hermes_home),
        };
        let binary_present = binary_on_path(host.binary_name(), inputs.path);
        if config_present || binary_present {
            found.push(DetectedHost {
                host,
                config_present,
                binary_present,
            });
        }
    }
    found
}

pub fn detect_hosts_from_env(home: &Path) -> Vec<DetectedHost> {
    let path_value = std::env::var("PATH").ok();
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let hermes = std::env::var_os("HERMES_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    detect_hosts(DetectInputs {
        home,
        path: path_value.as_deref(),
        xdg_config_home: xdg.as_deref(),
        hermes_home: hermes.as_deref(),
    })
}

fn opencode_config_present(home: &Path, xdg_config_home: Option<&Path>) -> bool {
    if home.join(".config").join("opencode").is_dir() {
        return true;
    }
    xdg_config_home
        .map(|root| root.join("opencode").is_dir())
        .unwrap_or(false)
}

fn hermes_config_present(home: &Path, hermes_home: Option<&Path>) -> bool {
    if home.join(".hermes").is_dir() {
        return true;
    }
    hermes_home
        .map(|root| root.is_dir() || root.join("config.yaml").is_file())
        .unwrap_or(false)
}

fn binary_on_path(name: &str, path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if is_executable(&dir.join(name)) {
            return true;
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            if is_executable(&dir.join(format!("{name}.{ext}"))) {
                return true;
            }
        }
    }
    false
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_binary(dir: &Path, name: &str) -> String {
        std::fs::create_dir_all(dir).expect("bin dir");
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").expect("bin file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        dir.to_string_lossy().to_string()
    }

    fn hosts(found: &[DetectedHost]) -> Vec<Host> {
        found.iter().map(|item| item.host).collect()
    }

    #[test]
    fn detects_codex_from_config_directory() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(home.path().join(".codex")).expect("codex dir");
        let found = detect_hosts(DetectInputs {
            home: home.path(),
            path: None,
            xdg_config_home: None,
            hermes_home: None,
        });
        assert_eq!(hosts(&found), vec![Host::Codex]);
        assert!(found[0].config_present);
        assert!(!found[0].binary_present);
    }

    #[test]
    fn detects_claude_from_config_file() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(home.path().join(".claude.json"), "{}").expect("claude json");
        let found = detect_hosts(DetectInputs {
            home: home.path(),
            path: None,
            xdg_config_home: None,
            hermes_home: None,
        });
        assert_eq!(hosts(&found), vec![Host::ClaudeCode]);
    }

    #[test]
    fn detects_hosts_from_path_binaries() {
        let home = tempfile::tempdir().expect("tempdir");
        let bin = tempfile::tempdir().expect("bin");
        seed_binary(bin.path(), "claude");
        seed_binary(bin.path(), "opencode");
        let path_value = bin.path().to_string_lossy().to_string();
        let found = detect_hosts(DetectInputs {
            home: home.path(),
            path: Some(&path_value),
            xdg_config_home: None,
            hermes_home: None,
        });
        assert_eq!(hosts(&found), vec![Host::ClaudeCode, Host::OpenCode]);
        assert!(found.iter().all(|item| item.binary_present));
    }

    #[test]
    fn detects_opencode_from_xdg_config() {
        let home = tempfile::tempdir().expect("tempdir");
        let xdg = tempfile::tempdir().expect("xdg");
        std::fs::create_dir_all(xdg.path().join("opencode")).expect("opencode dir");
        let found = detect_hosts(DetectInputs {
            home: home.path(),
            path: None,
            xdg_config_home: Some(xdg.path()),
            hermes_home: None,
        });
        assert_eq!(hosts(&found), vec![Host::OpenCode]);
    }

    #[test]
    fn detects_hermes_from_env_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let hermes = tempfile::tempdir().expect("hermes");
        let found = detect_hosts(DetectInputs {
            home: home.path(),
            path: None,
            xdg_config_home: None,
            hermes_home: Some(hermes.path()),
        });
        assert_eq!(hosts(&found), vec![Host::Hermes]);
    }

    #[test]
    fn detects_nothing_for_empty_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let empty_bin = tempfile::tempdir().expect("bin");
        let path_value = empty_bin.path().to_string_lossy().to_string();
        let found = detect_hosts(DetectInputs {
            home: home.path(),
            path: Some(&path_value),
            xdg_config_home: None,
            hermes_home: None,
        });
        assert!(found.is_empty());
    }
}
