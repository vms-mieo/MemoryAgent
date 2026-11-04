use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPaths {
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub db_file: PathBuf,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn data_dir_default() -> PathBuf {
    if let Some(base) = home_dir() {
        if cfg!(target_os = "macos") {
            return base
                .join("Library")
                .join("Application Support")
                .join("agent-memory");
        }
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
            return xdg.join("agent-memory");
        }
        return base.join(".local").join("share").join("agent-memory");
    }
    PathBuf::from(".agent-memory")
}

fn config_file_default(data_dir: &std::path::Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        return data_dir.join("config.toml");
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        let candidate = xdg.join("agent-memory").join("config.toml");
        if std::env::var_os("AGENT_MEMORY_HOME").is_none() {
            return candidate;
        }
    }
    let legacy = home_dir().map(|h| h.join(".config").join("agent-memory").join("config.toml"));
    if let Some(path) = legacy {
        if path.exists() {
            return path;
        }
    }
    data_dir.join("config.toml")
}

pub fn resolve_paths(home_override: Option<PathBuf>) -> ResolvedPaths {
    let has_override = home_override.is_some();
    let data_dir = if let Some(path) = home_override {
        path
    } else if let Some(env) = std::env::var_os("AGENT_MEMORY_HOME").map(PathBuf::from) {
        env
    } else {
        data_dir_default()
    };
    let config_file = if std::env::var_os("AGENT_MEMORY_HOME").is_some() || has_override {
        data_dir.join("config.toml")
    } else {
        config_file_default(&data_dir)
    };
    let db_file = data_dir.join("memory.db");
    ResolvedPaths {
        data_dir,
        config_file,
        db_file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn override_wins_over_env_and_default() {
        let custom = PathBuf::from("/tmp/lam-test-home-override");
        let resolved = resolve_paths(Some(custom.clone()));
        assert_eq!(resolved.data_dir, custom);
        assert_eq!(resolved.config_file, custom.join("config.toml"));
        assert_eq!(resolved.db_file, custom.join("memory.db"));
    }
    #[test]
    fn default_paths_end_with_expected_names() {
        let resolved = resolve_paths(Some(PathBuf::from("/tmp/lam-default-check")));
        assert!(resolved.db_file.ends_with("memory.db"));
        assert!(resolved.config_file.ends_with("config.toml"));
    }
}
