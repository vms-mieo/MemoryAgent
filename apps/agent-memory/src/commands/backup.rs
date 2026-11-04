use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum BackupAction {
    Create(BackupCreateArgs),
    Restore(BackupRestoreArgs),
}

#[derive(Debug, Args)]
pub struct BackupCreateArgs {
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct BackupRestoreArgs {
    #[arg(long)]
    pub input: PathBuf,
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

pub fn run(
    home: Option<PathBuf>,
    action: BackupAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        BackupAction::Create(args) => run_create(home, args, as_json),
        BackupAction::Restore(args) => run_restore(home, args, as_json),
    }
}

fn backup_dir(paths: &lam_config::ResolvedPaths) -> PathBuf {
    paths.data_dir.join("backups")
}

fn default_output(paths: &lam_config::ResolvedPaths) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    backup_dir(paths).join(format!("lam-backup-{stamp}.db"))
}

fn file_sha256(path: &std::path::Path) -> Result<String, lam_core::LamError> {
    let bytes = std::fs::read(path).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn table_counts(conn: &rusqlite::Connection) -> Result<serde_json::Value, lam_core::LamError> {
    let mut counts = serde_json::Map::new();
    for table in [
        "memories",
        "memory_revisions",
        "episodes",
        "sessions",
        "fact_assertions",
        "documents",
        "document_chunks",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        counts.insert(table.to_string(), serde_json::json!(count));
    }
    Ok(serde_json::Value::Object(counts))
}

fn run_create(
    home: Option<PathBuf>,
    args: BackupCreateArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (_, paths) = crate::commands::open_existing(&home)?;
    let target = args.output.unwrap_or_else(|| default_output(&paths));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    }
    let busy = std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|t| lam_config::Config::from_toml_str(&t).ok())
        .map(|c| c.storage.busy_timeout_ms)
        .unwrap_or(3000);
    let conn = lam_storage::open(&paths.db_file, busy)?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let quoted = target.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{quoted}';"))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let versions = lam_storage::applied_versions(&conn)?;
    let counts = table_counts(&conn)?;
    drop(conn);
    let manifest = serde_json::json!({
        "tool": "agent-memory backup",
        "created_at": chrono::Utc::now().to_rfc3339(),
        "schema_versions": versions,
        "counts": counts,
        "sha256_blake3": file_sha256(&target)?,
    });
    let manifest_path = target.with_extension("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string()),
    )
    .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    lam_storage::harden_db_files(&target);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&manifest_path, perm);
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "output": target,
                "manifest": manifest_path,
                "schema_versions": versions,
                "counts": counts,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("backup wrote {}", target.display());
        println!("manifest wrote {}", manifest_path.display());
    }
    Ok(())
}

fn run_restore(
    home: Option<PathBuf>,
    args: BackupRestoreArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    if !args.force {
        return Err(lam_core::LamError::validation(
            "E_BACKUP_FORCE_REQUIRED",
            "restore replaces the live database; rerun with --force to confirm".to_string(),
        ));
    }
    if !args.input.exists() {
        return Err(lam_core::LamError::validation(
            "E_BACKUP_MISSING",
            format!("no backup file at {}", args.input.display()),
        ));
    }
    let manifest_path = args.input.with_extension("manifest.json");
    if manifest_path.exists() {
        let raw = std::fs::read_to_string(&manifest_path)
            .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        let manifest: serde_json::Value =
            serde_json::from_str(&raw).map_err(lam_core::LamError::from)?;
        let expected = manifest
            .get("sha256_blake3")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !expected.is_empty() && file_sha256(&args.input)? != expected {
            return Err(lam_core::LamError::validation(
                "E_BACKUP_CORRUPT",
                "backup hash does not match its manifest".to_string(),
            ));
        }
    }
    let probe = lam_storage::open_readonly(&args.input)?;
    lam_storage::integrity_check(&probe)?;
    let missing = lam_storage::check_schema_valid(&probe)?;
    if !missing.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_BACKUP_SCHEMA",
            format!("backup is missing tables: {}", missing.join(", ")),
        ));
    }
    let counts = table_counts(&probe)?;
    drop(probe);
    let (_, paths) = crate::commands::open_existing(&home)?;
    let live = lam_storage::open(&paths.db_file, 3000)?;
    live.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    drop(live);
    for sidecar in lam_storage::sidecar_paths(&paths.db_file) {
        let _ = std::fs::remove_file(&sidecar);
    }
    std::fs::copy(&args.input, &paths.db_file)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    lam_storage::harden_db_files(&paths.db_file);
    let mut migrated = lam_storage::open(&paths.db_file, 3000)?;
    lam_storage::apply_migrations(&mut migrated)?;
    drop(migrated);
    let verify = lam_storage::open(&paths.db_file, 3000)?;
    lam_storage::integrity_check(&verify)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "restored": paths.db_file,
                "counts": counts,
                "note": "older backups may retain memories purged after the backup was taken",
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("restore replaced {}", paths.db_file.display());
        println!("note: older backups may retain memories purged after the backup was taken");
    }
    Ok(())
}
