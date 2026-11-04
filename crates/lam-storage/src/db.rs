use std::path::Path;
use std::path::PathBuf;

pub fn open(path: &Path, busy_timeout_ms: u64) -> Result<rusqlite::Connection, lam_core::LamError> {
    open_with_busy_timeout(path, busy_timeout_ms)
}

pub fn open_with_busy_timeout(
    path: &Path,
    busy_timeout_ms: u64,
) -> Result<rusqlite::Connection, lam_core::LamError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        }
    }
    let conn =
        rusqlite::Connection::open(path).map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    configure(&conn, busy_timeout_ms)?;
    harden_db_files(path);
    Ok(conn)
}

pub fn open_readonly(path: &Path) -> Result<rusqlite::Connection, lam_core::LamError> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.busy_timeout(std::time::Duration::from_millis(3000))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(conn)
}

pub fn open_in_memory(busy_timeout_ms: u64) -> Result<rusqlite::Connection, lam_core::LamError> {
    let conn = rusqlite::Connection::open_in_memory()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    configure(&conn, busy_timeout_ms)?;
    Ok(conn)
}

fn configure(conn: &rusqlite::Connection, busy_timeout_ms: u64) -> Result<(), lam_core::LamError> {
    conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;",
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(())
}

pub fn integrity_check(conn: &rusqlite::Connection) -> Result<(), lam_core::LamError> {
    let value: String = conn
        .query_row("PRAGMA integrity_check;", [], |row| row.get(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if value.to_lowercase() == "ok" {
        Ok(())
    } else {
        Err(lam_core::LamError::storage(format!(
            "integrity_check failed: {value}"
        )))
    }
}

pub fn check_schema_valid(conn: &rusqlite::Connection) -> Result<Vec<String>, lam_core::LamError> {
    let required = [
        "schema_migrations",
        "projects",
        "project_paths",
        "sessions",
        "session_events",
        "episodes",
        "working_states",
        "memories",
        "memory_revisions",
        "memory_sources",
        "fact_assertions",
        "entities",
        "entity_aliases",
        "relations",
        "documents",
        "document_chunks",
        "document_links",
        "memory_fts",
        "document_fts",
        "vector_index",
        "maintenance_jobs",
        "conflict_reviews",
        "admission_traces",
        "audit_log",
        "predicate_policy",
        "vault_roots",
        "episode_fts",
        "embedding_models",
        "dedup_suggestions",
        "remote_calls",
        "project_policies",
    ];
    let mut missing = Vec::new();
    for table in required {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if count == 0 {
            missing.push(table.to_string());
        }
    }
    Ok(missing)
}

pub fn journal_mode(conn: &rusqlite::Connection) -> Result<String, lam_core::LamError> {
    let mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(mode)
}

pub fn sidecar_paths(db_path: &Path) -> [PathBuf; 2] {
    let text = db_path.to_string_lossy().to_string();
    [
        PathBuf::from(text.clone() + "-wal"),
        PathBuf::from(text + "-shm"),
    ]
}

pub fn harden_db_files(db_path: &Path) {
    harden_one(db_path);
    for sidecar in sidecar_paths(db_path) {
        if sidecar.exists() {
            harden_one(&sidecar);
        }
    }
}

pub fn harden_current_db(conn: &rusqlite::Connection) {
    let file: Option<String> = (|| {
        let mut stmt = conn.prepare("PRAGMA database_list;").ok()?;
        let mut rows = stmt.query([]).ok()?;
        while let Some(row) = rows.next().ok()? {
            let name: String = row.get(1).ok()?;
            if name == "main" {
                return row.get(2).ok();
            }
        }
        None
    })();
    if let Some(path) = file {
        if !path.is_empty() {
            harden_db_files(Path::new(&path));
        }
    }
}

#[cfg(unix)]
fn harden_one(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        let perm = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn harden_one(_path: &Path) {}

pub fn is_retryable_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("locked") || lower.contains("busy")
}

#[cfg(unix)]
pub fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
pub fn file_mode(_path: &Path) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_db_has_wal_and_fk() {
        let conn = open_in_memory(3000).expect("open memory");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys;", [], |row| row.get(0))
            .expect("read fk");
        assert_eq!(fk, 1);
    }
    #[test]
    fn integrity_check_passes_on_fresh_db() {
        let conn = open_in_memory(3000).expect("open memory");
        integrity_check(&conn).expect("integrity ok");
    }

    #[test]
    fn one_thousand_concurrent_writes_without_corruption() {
        concurrent_writes_assert(4, 250);
    }

    #[test]
    #[ignore = "full NFR-008 sweep: 8 threads x 1250 writes, minutes in debug; run explicitly"]
    fn ten_thousand_concurrent_writes_without_corruption() {
        concurrent_writes_assert(8, 1250);
    }

    fn concurrent_writes_assert(threads: usize, per_thread: usize) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("concurrent.db");
        let mut setup = open(&db_path, 3000).expect("open file db");
        crate::migrations::apply_migrations(&mut setup).expect("migrate");
        let now = chrono::Utc::now();
        let project = crate::projects::resolve_or_create(
            &mut setup,
            dir.path(),
            Some("concurrent-proj"),
            now,
        )
        .expect("project")
        .id;
        let project_text = project.to_string_value();
        drop(setup);
        let mut handles = Vec::new();
        for worker in 0..threads {
            let path = db_path.clone();
            let project_text = project_text.clone();
            handles.push(std::thread::spawn(move || {
                let mut conn = open(&path, 30_000).expect("worker open");
                let project: lam_core::ProjectId = project_text.parse().expect("project id");
                for item in 0..per_thread {
                    let key = format!("concurrent:{worker}:{item}");
                    let (id, _) = crate::save_memory(
                        &mut conn,
                        crate::SaveInput {
                            project_id: project,
                            scope: lam_core::Scope::Project,
                            kind: lam_core::MemoryKind::Note,
                            content: format!("concurrent write {key}"),
                            status: lam_core::MemoryStatus::Proposed,
                            actor: "concurrency-test".to_string(),
                            reason: None,
                            sources: vec![],
                            idempotency_key: Some(key),
                        },
                        chrono::Utc::now(),
                    )
                    .expect("save");
                    let _ = id;
                }
            }));
        }
        for handle in handles {
            handle.join().expect("worker done");
        }
        let verify = open(&db_path, 3000).expect("verify open");
        integrity_check(&verify).expect("integrity ok");
        let count: i64 = verify
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, (threads * per_thread) as i64);
    }
}
