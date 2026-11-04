pub mod backup;
pub mod benchmark;
pub mod dedup;
pub mod doctor;
pub mod facts;
pub mod graph;
pub mod hook;
pub mod index;
pub mod init;
pub mod integration;
pub mod maintain;
pub mod mcp;
pub mod memory;
pub mod privacy;
pub mod project;
pub mod providers;
pub mod search;
pub mod session;
pub mod status;
pub mod transfer;
pub mod vault;

pub fn open_existing(
    home: &Option<std::path::PathBuf>,
) -> Result<(rusqlite::Connection, lam_config::ResolvedPaths), lam_core::LamError> {
    let paths = lam_config::resolve_paths(home.clone());
    if !paths.config_file.exists() || !paths.db_file.exists() {
        return Err(lam_core::LamError::validation(
            "E_NOT_INITIALIZED",
            "workspace is not initialized, run agent-memory init".to_string(),
        ));
    }
    let text = std::fs::read_to_string(&paths.config_file)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let config = lam_config::Config::from_toml_str(&text)?;
    let conn = lam_storage::open(&paths.db_file, config.storage.busy_timeout_ms)?;
    Ok((conn, paths))
}
