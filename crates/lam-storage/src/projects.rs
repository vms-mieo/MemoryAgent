use chrono::DateTime;
use chrono::Utc;
use std::path::Path;
use std::path::PathBuf;

use crate::db::is_retryable_message;

pub fn normalize_path(input: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(input) {
        return canonical;
    }
    let absolute = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(input)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn identity_hash_for(canonical: &Path) -> String {
    blake3::hash(canonical.to_string_lossy().as_bytes())
        .to_hex()
        .to_string()
}

fn project_from_row(row: &rusqlite::Row) -> Result<lam_core::Project, rusqlite::Error> {
    let id_text: String = row.get(0)?;
    let name: String = row.get(1)?;
    let root_text: String = row.get(2)?;
    let identity_hash: String = row.get(3)?;
    let created_text: String = row.get(4)?;
    let id = id_text.parse::<lam_core::ProjectId>().map_err(|_| {
        rusqlite::Error::InvalidColumnType(0, "id".to_string(), rusqlite::types::Type::Text)
    })?;
    let created_at = created_text.parse::<DateTime<Utc>>().map_err(|_| {
        rusqlite::Error::InvalidColumnType(4, "created_at".to_string(), rusqlite::types::Type::Text)
    })?;
    Ok(lam_core::Project {
        id,
        name,
        canonical_root: PathBuf::from(root_text),
        identity_hash,
        created_at,
    })
}

fn default_name_for(canonical: &Path) -> String {
    canonical
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or("unnamed-project")
        .to_string()
}

pub fn resolve_or_create(
    conn: &mut rusqlite::Connection,
    path: &Path,
    name_override: Option<&str>,
    now: DateTime<Utc>,
) -> Result<lam_core::Project, lam_core::LamError> {
    let canonical = normalize_path(path);
    let root_text = canonical.to_string_lossy().to_string();
    let mut attempt = 0;
    loop {
        attempt += 1;
        let step: Result<lam_core::Project, lam_core::LamError> = (|| {
            let existing: Option<lam_core::Project> = conn
                .query_row(
                    "SELECT id, name, canonical_root, identity_hash, created_at FROM projects WHERE canonical_root = ?1",
                    rusqlite::params![root_text],
                    project_from_row,
                )
                .optional()
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            if let Some(mut project) = existing {
                if let Some(wanted) = name_override {
                    let wanted = wanted.trim();
                    if !wanted.is_empty() && wanted != project.name {
                        if wanted.len() > 256 {
                            return Err(lam_core::LamError::validation(
                                "E_PROJECT_NAME_INVALID",
                                "project name exceeds 256 chars".to_string(),
                            ));
                        }
                        conn.execute(
                            "UPDATE projects SET name = ?1 WHERE id = ?2",
                            rusqlite::params![wanted, project.id.to_string_value()],
                        )
                        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                        project.name = wanted.to_string();
                    }
                }
                record_path_seen(conn, &project.id, &canonical, now)?;
                return Ok(project);
            }
            let name = name_override
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_name_for(&canonical));
            if name.len() > 256 {
                return Err(lam_core::LamError::validation(
                    "E_PROJECT_NAME_INVALID",
                    "project name exceeds 256 chars".to_string(),
                ));
            }
            let project = lam_core::Project {
                id: lam_core::ProjectId::new(),
                name,
                canonical_root: canonical.clone(),
                identity_hash: identity_hash_for(&canonical),
                created_at: now,
            };
            project.validate()?;
            conn.execute(
                "INSERT INTO projects (id, name, canonical_root, identity_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    project.id.to_string_value(),
                    project.name,
                    root_text,
                    project.identity_hash,
                    project.created_at.to_rfc3339(),
                ],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            record_path_seen(conn, &project.id, &canonical, now)?;
            crate::db::harden_current_db(conn);
            Ok(project)
        })();
        match step {
            Ok(project) => return Ok(project),
            Err(e) if is_retryable_message(&e.to_string()) && attempt < 30 => {
                std::thread::sleep(std::time::Duration::from_millis(10 * attempt as u64));
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn resolve_existing_by_path(
    conn: &rusqlite::Connection,
    path: &Path,
) -> Result<lam_core::Project, lam_core::LamError> {
    let canonical = normalize_path(path);
    let root_text = canonical.to_string_lossy().to_string();
    conn.query_row(
        "SELECT id, name, canonical_root, identity_hash, created_at FROM projects WHERE canonical_root = ?1",
        rusqlite::params![root_text],
        project_from_row,
    )
    .optional()
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?
    .ok_or_else(|| {
        lam_core::LamError::validation(
            "E_PROJECT_NOT_FOUND",
            format!("no project registered for path {root_text}"),
        )
    })
}

pub fn lookup_by_id(
    conn: &rusqlite::Connection,
    id: &lam_core::ProjectId,
) -> Result<lam_core::Project, lam_core::LamError> {
    conn.query_row(
        "SELECT id, name, canonical_root, identity_hash, created_at FROM projects WHERE id = ?1",
        rusqlite::params![id.to_string_value()],
        project_from_row,
    )
    .optional()
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?
    .ok_or_else(|| {
        lam_core::LamError::validation("E_PROJECT_NOT_FOUND", format!("no project with id {id}"))
    })
}

pub fn resolve_ref(
    conn: &mut rusqlite::Connection,
    reference: &str,
    now: DateTime<Utc>,
    create_if_missing: bool,
) -> Result<lam_core::Project, lam_core::LamError> {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_PROJECT_REF_INVALID",
            "project reference must not be empty".to_string(),
        ));
    }
    if let Ok(id) = trimmed.parse::<lam_core::ProjectId>() {
        return lookup_by_id(conn, &id);
    }
    if create_if_missing {
        resolve_or_create(conn, Path::new(trimmed), None, now)
    } else {
        resolve_existing_by_path(conn, Path::new(trimmed))
    }
}

pub fn list_projects(
    conn: &rusqlite::Connection,
) -> Result<Vec<lam_core::Project>, lam_core::LamError> {
    let mut stmt = conn
        .prepare("SELECT id, name, canonical_root, identity_hash, created_at FROM projects ORDER BY created_at ASC")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map([], project_from_row)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| lam_core::LamError::storage(e.to_string()))?);
    }
    Ok(out)
}

pub fn record_path_seen(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    path: &Path,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    let text = normalize_path(path).to_string_lossy().to_string();
    let stamp = now.to_rfc3339();
    conn.execute(
        "INSERT INTO project_paths (project_id, path, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(project_id, path) DO UPDATE SET last_seen = excluded.last_seen",
        rusqlite::params![project_id.to_string_value(), text, stamp],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(())
}

pub fn create_session(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    agent_kind: lam_core::AgentKind,
    external_session_id: Option<&str>,
    branch: Option<&str>,
    started_at: DateTime<Utc>,
) -> Result<lam_core::Session, lam_core::LamError> {
    lookup_by_id(conn, project_id)?;
    let session = lam_core::Session {
        id: lam_core::SessionId::new(),
        project_id: *project_id,
        agent_kind,
        external_session_id: external_session_id.map(|s| s.to_string()),
        branch: branch.map(|s| s.to_string()),
        started_at,
        ended_at: None,
    };
    conn.execute(
        "INSERT INTO sessions (id, project_id, agent_kind, external_session_id, branch, started_at, ended_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        rusqlite::params![
            session.id.to_string(),
            project_id.to_string_value(),
            agent_kind.as_str(),
            session.external_session_id,
            session.branch,
            started_at.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(session)
}

pub fn end_session(
    conn: &rusqlite::Connection,
    session_id: &lam_core::SessionId,
    ended_at: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    let changed = conn
        .execute(
            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
            rusqlite::params![ended_at.to_rfc3339(), session_id.to_string()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if changed == 0 {
        return Err(lam_core::LamError::validation(
            "E_SESSION_NOT_FOUND",
            format!("no session with id {session_id}"),
        ));
    }
    Ok(())
}

pub fn record_event(
    conn: &rusqlite::Connection,
    session_id: &lam_core::SessionId,
    event: &lam_core::NormalizedEvent,
) -> Result<String, lam_core::LamError> {
    event.validate()?;
    let key = event.idempotency_key();
    let payload = serde_json::to_string(&event.payload).map_err(lam_core::LamError::from)?;
    conn.execute(
        "INSERT INTO session_events (id, session_id, event_key, kind, sanitized_payload, created_at)
         VALUES (?1, ?2, ?1, ?3, ?4, ?5) ON CONFLICT(event_key) DO NOTHING",
        rusqlite::params![
            key,
            session_id.to_string(),
            event.event_type,
            payload,
            event.timestamp.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(key)
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lam_core::Clock;
    use lam_core::ManualClock;

    fn memory_conn() -> rusqlite::Connection {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    #[test]
    fn resolve_creates_then_reuses_project() {
        let mut conn = memory_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let dir = tempfile::tempdir().expect("tempdir");
        let first = resolve_or_create(&mut conn, dir.path(), None, clock.now()).expect("create");
        let second = resolve_or_create(&mut conn, dir.path(), None, clock.now()).expect("reuse");
        assert_eq!(first.id, second.id);
        assert_eq!(list_projects(&conn).expect("list").len(), 1);
    }

    #[test]
    fn rename_move_keeps_identity_through_rebind() {
        let mut conn = memory_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let dir = tempfile::tempdir().expect("tempdir");
        let created = resolve_or_create(&mut conn, dir.path(), Some("cloud-platform"), clock.now())
            .expect("create");
        let rebound =
            resolve_or_create(&mut conn, dir.path(), Some("renamed"), clock.now()).expect("rebind");
        assert_eq!(created.id, rebound.id);
        assert_eq!(rebound.name, "renamed");
    }

    #[test]
    fn session_and_event_round_trip() {
        let mut conn = memory_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let dir = tempfile::tempdir().expect("tempdir");
        let project = resolve_or_create(&mut conn, dir.path(), None, clock.now()).expect("project");
        let session = create_session(
            &conn,
            &project.id,
            lam_core::AgentKind::ClaudeCode,
            Some("ext-1"),
            Some("main"),
            clock.now(),
        )
        .expect("session");
        let bundle = lam_core::generate_bundle(9, 1, &clock);
        let event = bundle.events.into_iter().next().expect("event");
        let first = record_event(&conn, &session.id, &event).expect("record");
        let second = record_event(&conn, &session.id, &event).expect("replay");
        assert_eq!(first, second);
        end_session(&conn, &session.id, clock.now()).expect("end");
    }

    #[test]
    fn missing_project_reference_errors() {
        let mut conn = memory_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let result = resolve_ref(
            &mut conn,
            "/tmp/lam-definitely-missing-xyz",
            clock.now(),
            false,
        );
        assert!(result.is_err());
    }
}
