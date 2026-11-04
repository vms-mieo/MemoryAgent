use std::path::PathBuf;

pub fn run(home: Option<PathBuf>, as_json: bool) -> Result<(), lam_core::LamError> {
    let paths = lam_config::resolve_paths(home);
    let data_exists = paths.data_dir.exists();
    let config_exists = paths.config_file.exists();
    let db_exists = paths.db_file.exists();
    let config_state = if config_exists {
        match std::fs::read_to_string(&paths.config_file) {
            Ok(text) => match lam_config::Config::from_toml_str(&text) {
                Ok(_) => "valid",
                Err(_) => "invalid",
            },
            Err(_) => "unreadable",
        }
    } else {
        "missing"
    };
    let (db_state, schema_version, table_count) = if db_exists {
        match lam_storage::open(&paths.db_file, 3000) {
            Ok(conn) => {
                match lam_storage::applied_versions(&conn) {
                    Ok(versions) => {
                        let count: i64 = conn
                        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','virtual')", [], |row| row.get(0))
                        .unwrap_or(0);
                        (
                            if versions.is_empty() { "empty" } else { "ok" },
                            versions.last().copied(),
                            count,
                        )
                    }
                    Err(_) => ("error", None, 0),
                }
            }
            Err(_) => ("error", None, 0),
        }
    } else {
        ("missing", None, 0)
    };
    let remote = std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|text| lam_config::Config::from_toml_str(&text).ok())
        .map(|config| {
            let provider = config.models.extraction.provider.clone();
            let enabled = provider == "openai-compatible";
            serde_json::json!({
                "provider": provider,
                "model": config.models.extraction.model,
                "endpoint_host": remote_host(&config.models.extraction.endpoint),
                "allow_remote": config.models.extraction.allow_remote,
                "api_key_env": config.models.extraction.api_key_env,
                "api_key_set": is_key_set(&config.models.extraction.api_key_env),
                "enabled": enabled,
            })
        })
        .unwrap_or(serde_json::json!({"enabled": false, "detail": "config unreadable"}));
    if as_json {
        let output = serde_json::json!({
            "data_dir": paths.data_dir.to_string_lossy(),
            "config_file": paths.config_file.to_string_lossy(),
            "db_file": paths.db_file.to_string_lossy(),
            "data_exists": data_exists,
            "config": config_state,
            "db": db_state,
            "schema_version": schema_version,
            "table_count": table_count,
            "remote_provider": remote,
            "remote_calls": remote_calls_total(&paths.db_file)
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("data_dir: {}", paths.data_dir.to_string_lossy());
        println!(
            "config: {config_state} ({})",
            paths.config_file.to_string_lossy()
        );
        println!("db: {db_state} ({})", paths.db_file.to_string_lossy());
        if let Some(v) = schema_version {
            println!("schema_version: {v}");
        }
        println!("tables: {table_count}");
        println!(
            "remote_provider: {} (allow_remote: {})",
            remote
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            remote
                .get("allow_remote")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        );
        println!("remote_calls: {}", remote_calls_total(&paths.db_file));
    }
    Ok(())
}

fn remote_host(raw: &str) -> String {
    let text = raw.trim();
    let without_scheme = match text.find("://") {
        Some(position) => &text[position + 3..],
        None => text,
    };
    without_scheme.split('/').next().unwrap_or("").to_string()
}

fn is_key_set(holder: &str) -> bool {
    let name = holder.trim();
    !name.is_empty()
        && std::env::var(name)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
}

fn remote_calls_total(db_file: &std::path::Path) -> i64 {
    lam_storage::open_readonly(db_file)
        .ok()
        .and_then(|conn| lam_storage::remote_call_count(&conn).ok())
        .unwrap_or(0)
}
