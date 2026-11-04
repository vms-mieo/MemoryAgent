use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[arg(long, default_value_t = false)]
    pub repair: bool,
}

pub fn run(
    home: Option<PathBuf>,
    args: DoctorArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let paths = lam_config::resolve_paths(home);
    let mut checks: Vec<serde_json::Value> = Vec::new();
    let mut ok = true;
    let data_exists = paths.data_dir.exists();
    checks.push(serde_json::json!({"name": "data_dir", "ok": data_exists, "path": paths.data_dir.to_string_lossy()}));
    if !data_exists {
        ok = false;
    }
    let config_result = if paths.config_file.exists() {
        match std::fs::read_to_string(&paths.config_file) {
            Ok(text) => match lam_config::Config::from_toml_str(&text) {
                Ok(config) => {
                    if config.privacy.allow_network {
                        Some("allow_network must be false".to_string())
                    } else {
                        None
                    }
                }
                Err(e) => Some(e.to_string()),
            },
            Err(e) => Some(e.to_string()),
        }
    } else {
        Some("config file missing, run init".to_string())
    };
    let config_ok = config_result.is_none();
    checks.push(serde_json::json!({
        "name": "config",
        "ok": config_ok,
        "detail": config_result.unwrap_or_else(|| "valid".to_string())
    }));
    if !config_ok {
        ok = false;
    }
    let busy_timeout = std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|t| lam_config::Config::from_toml_str(&t).ok())
        .map(|c| c.storage.busy_timeout_ms)
        .unwrap_or(3000);
    for check in permission_checks(&paths) {
        if !check.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            ok = false;
        }
        checks.push(check);
    }
    if paths.db_file.exists() {
        if args.repair {
            for fix in repair_workspace(&paths) {
                checks.push(fix);
            }
        }
        match lam_storage::open(&paths.db_file, busy_timeout) {
            Ok(conn) => {
                let integrity_ok = lam_storage::integrity_check(&conn).is_ok();
                checks.push(serde_json::json!({"name": "db_integrity", "ok": integrity_ok}));
                if !integrity_ok {
                    ok = false;
                }
                match lam_storage::check_schema_valid(&conn) {
                    Ok(missing) => {
                        let schema_ok = missing.is_empty();
                        checks.push(serde_json::json!({"name": "db_schema", "ok": schema_ok, "missing": missing}));
                        if !schema_ok {
                            ok = false;
                        }
                    }
                    Err(e) => {
                        checks.push(serde_json::json!({"name": "db_schema", "ok": false, "detail": e.to_string()}));
                        ok = false;
                    }
                }
                let mode =
                    lam_storage::db::journal_mode(&conn).unwrap_or_else(|_| "unknown".to_string());
                let wal_ok = mode.to_lowercase() == "wal";
                checks.push(serde_json::json!({"name": "db_wal", "ok": wal_ok, "mode": mode}));
                if !wal_ok {
                    ok = false;
                }
                let fk: i64 = conn
                    .query_row("PRAGMA foreign_keys;", [], |row| row.get(0))
                    .unwrap_or(0);
                let fk_ok = fk == 1;
                checks.push(serde_json::json!({"name": "db_foreign_keys", "ok": fk_ok}));
                if !fk_ok {
                    ok = false;
                }
            }
            Err(e) => {
                checks.push(
                    serde_json::json!({"name": "db_open", "ok": false, "detail": e.to_string()}),
                );
                ok = false;
            }
        }
    } else {
        checks.push(serde_json::json!({"name": "db", "ok": false, "detail": "database file missing, run init"}));
        ok = false;
    }
    if as_json {
        let output = serde_json::json!({"ok": ok, "checks": checks});
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        for check in &checks {
            let name = check
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let passed = check.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            let mark = if passed { "ok" } else { "FAIL" };
            println!("{mark} {name}");
        }
        if !ok {
            println!("doctor found issues, run agent-memory init to repair workspace");
        }
    }
    if ok {
        Ok(())
    } else {
        Err(lam_core::LamError::validation(
            "E_DOCTOR_FAILED",
            "doctor checks failed".to_string(),
        ))
    }
}

fn repair_workspace(paths: &lam_config::ResolvedPaths) -> Vec<serde_json::Value> {
    let mut fixes = Vec::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let targets = [
            (&paths.data_dir, 0o700),
            (&paths.config_file, 0o600),
            (&paths.db_file, 0o600),
        ];
        for (path, mode) in targets {
            if path.exists() {
                let result = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
                fixes.push(serde_json::json!({
                    "name": format!("repair_perms_{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")),
                    "ok": result.is_ok(),
                }));
                for sidecar in lam_storage::sidecar_paths(&paths.db_file) {
                    if sidecar.exists() {
                        let side_result = std::fs::set_permissions(
                            &sidecar,
                            std::fs::Permissions::from_mode(0o600),
                        );
                        fixes.push(serde_json::json!({"name": "repair_perms_sidecar", "ok": side_result.is_ok()}));
                    }
                }
            }
        }
    }
    if let Ok(conn) = lam_storage::open(&paths.db_file, 3000) {
        let checkpointed = conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .is_ok();
        fixes.push(serde_json::json!({"name": "repair_wal_checkpoint", "ok": checkpointed}));
    }
    fixes
}

fn permission_checks(paths: &lam_config::ResolvedPaths) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    out.push(mode_check(
        "data_dir_perms",
        &paths.data_dir,
        0o700,
        paths.data_dir.exists(),
    ));
    out.push(mode_check(
        "config_perms",
        &paths.config_file,
        0o600,
        paths.config_file.exists(),
    ));
    out.push(mode_check(
        "db_perms",
        &paths.db_file,
        0o600,
        paths.db_file.exists(),
    ));
    for sidecar in lam_storage::sidecar_paths(&paths.db_file) {
        if sidecar.exists() {
            let name = sidecar
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("db_sidecar");
            let label = if name.ends_with("-wal") {
                "db_wal_perms"
            } else {
                "db_shm_perms"
            };
            out.push(mode_check(label, &sidecar, 0o600, true));
        }
    }
    out
}

fn mode_check(
    name: &str,
    path: &std::path::Path,
    expected: u32,
    must_exist: bool,
) -> serde_json::Value {
    if !path.exists() {
        return serde_json::json!({"name": name, "ok": !must_exist, "detail": "file missing"});
    }
    match lam_storage::file_mode(path) {
        Some(mode) => {
            let pass = mode == expected;
            serde_json::json!({"name": name, "ok": pass, "mode": format!("{mode:o}"), "expected": format!("{expected:o}")})
        }
        None => {
            serde_json::json!({"name": name, "ok": true, "detail": "permission check applies on unix"})
        }
    }
}
