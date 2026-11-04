use std::path::PathBuf;

fn is_retryable(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("locked") || lower.contains("busy")
}

pub fn ensure_workspace(
    home: Option<PathBuf>,
) -> Result<(lam_config::ResolvedPaths, lam_config::Config), lam_core::LamError> {
    ensure_workspace_sized(home, None, 40)
}

pub fn ensure_workspace_sized(
    home: Option<PathBuf>,
    busy_override_ms: Option<u64>,
    max_attempts: u32,
) -> Result<(lam_config::ResolvedPaths, lam_config::Config), lam_core::LamError> {
    let paths = lam_config::resolve_paths(home);
    std::fs::create_dir_all(&paths.data_dir).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o700);
        let _ = std::fs::set_permissions(&paths.data_dir, perm);
    }
    let config_text = if paths.config_file.exists() {
        std::fs::read_to_string(&paths.config_file)
            .map_err(|e| lam_core::LamError::io(e.to_string()))?
    } else {
        let config = lam_config::Config::default();
        let text = config.to_toml_string()?;
        write_if_missing(&paths.config_file, &text)?;
        std::fs::read_to_string(&paths.config_file)
            .map_err(|e| lam_core::LamError::io(e.to_string()))?
    };
    let config = lam_config::Config::from_toml_str(&config_text)?;
    let busy_timeout = busy_override_ms.unwrap_or(config.storage.busy_timeout_ms);
    let mut attempt = 0;
    let db_result = loop {
        attempt += 1;
        let step: Result<(), lam_core::LamError> = (|| {
            let mut conn = lam_storage::open(&paths.db_file, busy_timeout)?;
            let _ = lam_storage::apply_migrations(&mut conn)?;
            lam_storage::integrity_check(&conn)?;
            let missing = lam_storage::check_schema_valid(&conn)?;
            if !missing.is_empty() {
                return Err(lam_core::LamError::storage(format!(
                    "schema missing tables: {}",
                    missing.join(",")
                )));
            }
            Ok(())
        })();
        match step {
            Ok(()) => break Ok(()),
            Err(e) if is_retryable(&e.to_string()) && attempt < max_attempts => {
                std::thread::sleep(std::time::Duration::from_millis(25 * attempt as u64));
            }
            Err(e) => break Err(e),
        }
    };
    db_result?;
    lam_storage::harden_db_files(&paths.db_file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&paths.config_file, perm);
    }
    Ok((paths, config))
}

pub fn run(home: Option<PathBuf>, as_json: bool) -> Result<(), lam_core::LamError> {
    let (paths, _) = ensure_workspace(home)?;
    if as_json {
        let output = serde_json::json!({
            "data_dir": paths.data_dir.to_string_lossy(),
            "config_file": paths.config_file.to_string_lossy(),
            "db_file": paths.db_file.to_string_lossy(),
            "status": "ok"
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("initialized {}", paths.data_dir.to_string_lossy());
    }
    Ok(())
}

fn write_if_missing(path: &std::path::Path, content: &str) -> Result<(), lam_core::LamError> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        }
    }
    if path.exists() {
        return Ok(());
    }
    let tmp = path.with_extension("toml.tmp");
    let mut file =
        std::fs::File::create(&tmp).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&tmp, perm);
    }
    file.write_all(content.as_bytes())
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    file.sync_all()
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    drop(file);
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => {
            if path.exists() {
                let _ = std::fs::remove_file(&tmp);
                Ok(())
            } else {
                Err(lam_core::LamError::io(e.to_string()))
            }
        }
    }
}
