use chrono::DateTime;
use chrono::Utc;

use crate::db::is_retryable_message;

#[derive(Debug, Clone)]
pub struct SaveInput {
    pub project_id: lam_core::ProjectId,
    pub scope: lam_core::Scope,
    pub kind: lam_core::MemoryKind,
    pub content: String,
    pub status: lam_core::MemoryStatus,
    pub actor: String,
    pub reason: Option<String>,
    pub sources: Vec<lam_core::SourceEvidence>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryDetail {
    pub memory: lam_core::Memory,
    pub revision: lam_core::MemoryRevision,
    pub sources: Vec<lam_core::SourceEvidence>,
}

fn parse_scope(text: &str) -> Result<lam_core::Scope, lam_core::LamError> {
    text.parse::<lam_core::Scope>()
}

fn parse_kind(text: &str) -> Result<lam_core::MemoryKind, lam_core::LamError> {
    text.parse::<lam_core::MemoryKind>()
}

fn parse_status(text: &str) -> Result<lam_core::MemoryStatus, lam_core::LamError> {
    text.parse::<lam_core::MemoryStatus>()
}

fn memory_from_row(row: &rusqlite::Row) -> Result<lam_core::Memory, rusqlite::Error> {
    let id_text: String = row.get(0)?;
    let project_text: String = row.get(1)?;
    let scope_text: String = row.get(2)?;
    let kind_text: String = row.get(3)?;
    let current_text: Option<String> = row.get(4)?;
    let created_text: String = row.get(5)?;
    let bad = |index: usize, name: &str| {
        rusqlite::Error::InvalidColumnType(index, name.to_string(), rusqlite::types::Type::Text)
    };
    let id = id_text
        .parse::<lam_core::MemoryId>()
        .map_err(|_| bad(0, "id"))?;
    let project_id = project_text
        .parse::<lam_core::ProjectId>()
        .map_err(|_| bad(1, "project_id"))?;
    let scope = parse_scope(&scope_text).map_err(|_| bad(2, "scope"))?;
    let kind = parse_kind(&kind_text).map_err(|_| bad(3, "kind"))?;
    let current_revision = current_text
        .map(|t| {
            t.parse::<lam_core::RevisionId>()
                .map_err(|_| bad(4, "current_revision"))
        })
        .transpose()?;
    let created_at = created_text
        .parse::<DateTime<Utc>>()
        .map_err(|_| bad(5, "created_at"))?;
    Ok(lam_core::Memory {
        id,
        project_id,
        scope,
        kind,
        current_revision,
        created_at,
    })
}

fn revision_from_row(row: &rusqlite::Row) -> Result<lam_core::MemoryRevision, rusqlite::Error> {
    let bad = |index: usize, name: &str| {
        rusqlite::Error::InvalidColumnType(index, name.to_string(), rusqlite::types::Type::Text)
    };
    let id_text: String = row.get(0)?;
    let memory_text: String = row.get(1)?;
    let content: String = row.get(2)?;
    let status_text: String = row.get(3)?;
    let version: u64 = row.get::<_, i64>(4).map(|v| v as u64)?;
    let recorded_text: String = row.get(5)?;
    let actor: String = row.get(6)?;
    let reason: Option<String> = row.get(7)?;
    let id = id_text
        .parse::<lam_core::RevisionId>()
        .map_err(|_| bad(0, "id"))?;
    let memory_id = memory_text
        .parse::<lam_core::MemoryId>()
        .map_err(|_| bad(1, "memory_id"))?;
    let status = parse_status(&status_text).map_err(|_| bad(3, "status"))?;
    let recorded_at = recorded_text
        .parse::<DateTime<Utc>>()
        .map_err(|_| bad(5, "recorded_at"))?;
    Ok(lam_core::MemoryRevision {
        id,
        memory_id,
        content,
        status,
        version,
        recorded_at,
        actor,
        reason,
    })
}

fn source_from_row(row: &rusqlite::Row) -> Result<lam_core::SourceEvidence, rusqlite::Error> {
    let bad = |index: usize, name: &str| {
        rusqlite::Error::InvalidColumnType(index, name.to_string(), rusqlite::types::Type::Text)
    };
    let kind_text: String = row.get(0)?;
    let source_ref: String = row.get(1)?;
    let line_start: Option<i64> = row.get(2)?;
    let line_end: Option<i64> = row.get(3)?;
    let commit_sha: Option<String> = row.get(4)?;
    let content_hash: Option<String> = row.get(5)?;
    let source_kind = match kind_text.as_str() {
        "document" => lam_core::SourceKind::Document,
        "session" => lam_core::SourceKind::Session,
        "episode" => lam_core::SourceKind::Episode,
        "user" => lam_core::SourceKind::User,
        "config" => lam_core::SourceKind::Config,
        "test" => lam_core::SourceKind::Test,
        _ => return Err(bad(0, "source_kind")),
    };
    Ok(lam_core::SourceEvidence {
        source_kind,
        source_ref,
        line_start: line_start.map(|v| v as u32),
        line_end: line_end.map(|v| v as u32),
        commit_sha,
        content_hash,
    })
}

fn source_kind_text(kind: &lam_core::SourceKind) -> &'static str {
    match kind {
        lam_core::SourceKind::Document => "document",
        lam_core::SourceKind::Session => "session",
        lam_core::SourceKind::Episode => "episode",
        lam_core::SourceKind::User => "user",
        lam_core::SourceKind::Config => "config",
        lam_core::SourceKind::Test => "test",
    }
}

fn with_write_retry<T>(
    conn: &mut rusqlite::Connection,
    mut operation: impl FnMut(&rusqlite::Transaction) -> Result<T, lam_core::LamError>,
) -> Result<T, lam_core::LamError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let tx = conn
            .transaction()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        match operation(&tx) {
            Ok(value) => {
                tx.commit()
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                crate::db::harden_current_db(conn);
                return Ok(value);
            }
            Err(e) if is_retryable_message(&e.to_string()) && attempt < 30 => {
                std::thread::sleep(std::time::Duration::from_millis(10 * attempt as u64));
            }
            Err(e) => return Err(e),
        }
    }
}

fn validated_sources(
    input: &SaveInput,
) -> Result<Vec<lam_core::SourceEvidence>, lam_core::LamError> {
    if input.sources.is_empty() {
        return Ok(vec![lam_core::SourceEvidence {
            source_kind: lam_core::SourceKind::User,
            source_ref: input.actor.clone(),
            line_start: None,
            line_end: None,
            commit_sha: None,
            content_hash: None,
        }]);
    }
    for source in &input.sources {
        source.validate()?;
    }
    Ok(input.sources.clone())
}

fn insert_revision_rows(
    tx: &rusqlite::Transaction,
    memory_id: &lam_core::MemoryId,
    revision: &lam_core::MemoryRevision,
    sources: &[lam_core::SourceEvidence],
) -> Result<(), lam_core::LamError> {
    tx.execute(
        "INSERT INTO memory_revisions (id, memory_id, content, status, version, recorded_at, actor, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            revision.id.to_string(),
            memory_id.to_string(),
            revision.content,
            revision.status.as_str(),
            revision.version as i64,
            revision.recorded_at.to_rfc3339(),
            revision.actor,
            revision.reason,
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for source in sources {
        tx.execute(
            "INSERT INTO memory_sources (id, revision_id, source_kind, source_ref, line_start, line_end, commit_sha, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                lam_core::RevisionId::new().to_string(),
                revision.id.to_string(),
                source_kind_text(&source.source_kind),
                source.source_ref,
                source.line_start.map(|v| v as i64),
                source.line_end.map(|v| v as i64),
                source.commit_sha,
                source.content_hash,
            ],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    }
    tx.execute(
        "DELETE FROM memory_fts WHERE memory_id = ?1",
        rusqlite::params![memory_id.to_string()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO memory_fts (memory_id, content) VALUES (?1, ?2)",
        rusqlite::params![memory_id.to_string(), revision.content],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "UPDATE memories SET current_revision = ?1 WHERE id = ?2",
        rusqlite::params![revision.id.to_string(), memory_id.to_string()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(())
}

fn audit_save(
    tx: &rusqlite::Transaction,
    event: &str,
    actor: &str,
    target: &str,
    old_hash: Option<&str>,
    new_hash: &str,
    at: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    tx.execute(
        "INSERT INTO audit_log (id, event, actor, target, old_hash, new_hash, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            lam_core::RevisionId::new().to_string(),
            event,
            actor,
            target,
            old_hash,
            new_hash,
            at.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(())
}

pub fn save_memory(
    conn: &mut rusqlite::Connection,
    input: SaveInput,
    now: DateTime<Utc>,
) -> Result<(lam_core::MemoryId, lam_core::RevisionId), lam_core::LamError> {
    lam_core::ensure_no_secrets(&input.content)?;
    if input.content.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_CONTENT_INVALID",
            "memory content must not be empty".to_string(),
        ));
    }
    if input.content.len() > 65536 {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_CONTENT_INVALID",
            "memory content exceeds 65536 bytes".to_string(),
        ));
    }
    if input.actor.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_ACTOR_INVALID",
            "actor must not be empty".to_string(),
        ));
    }
    let sources = validated_sources(&input)?;
    if let Some(key) = input.idempotency_key.clone() {
        let replay: Option<String> = conn
            .query_row(
                "SELECT new_hash FROM audit_log WHERE event = 'memory_save' AND target = ?1 ORDER BY at ASC LIMIT 1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if let Some(memory_text) = replay {
            if let Ok(memory_id) = memory_text.parse::<lam_core::MemoryId>() {
                if let Ok(detail) = get_memory(conn, &memory_id) {
                    return Ok((memory_id, detail.revision.id));
                }
            }
        }
    }
    let duplicate: Option<(String, String)> = conn
        .query_row(
            "SELECT m.id, m.current_revision FROM memories m
             JOIN memory_revisions r ON r.id = m.current_revision
             WHERE m.project_id = ?1 AND r.content = ?2 AND r.status = ?3 LIMIT 1",
            rusqlite::params![
                input.project_id.to_string_value(),
                input.content,
                input.status.as_str(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if let Some((memory_text, revision_text)) = duplicate {
        if let (Ok(memory_id), Ok(revision_id)) = (
            memory_text.parse::<lam_core::MemoryId>(),
            revision_text.parse::<lam_core::RevisionId>(),
        ) {
            return Ok((memory_id, revision_id));
        }
    }
    with_write_retry(conn, |tx| {
        let memory_id = lam_core::MemoryId::new();
        let revision_id = lam_core::RevisionId::new();
        tx.execute(
            "INSERT INTO memories (id, project_id, scope, kind, current_revision, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                memory_id.to_string(),
                input.project_id.to_string_value(),
                input.scope.as_str(),
                input.kind.as_str(),
                revision_id.to_string(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let revision = lam_core::MemoryRevision {
            id: revision_id,
            memory_id,
            content: input.content.clone(),
            status: input.status,
            version: 1,
            recorded_at: now,
            actor: input.actor.clone(),
            reason: input.reason.clone(),
        };
        revision.validate()?;
        insert_revision_rows(tx, &memory_id, &revision, &sources)?;
        audit_save(
            tx,
            "memory_save",
            &input.actor,
            &memory_id.to_string(),
            None,
            &memory_id.to_string(),
            now,
        )?;
        if let Some(key) = input.idempotency_key.clone() {
            audit_save(
                tx,
                "memory_save",
                &input.actor,
                &key,
                None,
                &memory_id.to_string(),
                now,
            )?;
        }
        Ok((memory_id, revision_id))
    })
}

pub fn get_memory(
    conn: &rusqlite::Connection,
    memory_id: &lam_core::MemoryId,
) -> Result<MemoryDetail, lam_core::LamError> {
    get_memory_at_revision(conn, memory_id, None)
}

pub fn get_memory_at_revision(
    conn: &rusqlite::Connection,
    memory_id: &lam_core::MemoryId,
    revision_id: Option<&lam_core::RevisionId>,
) -> Result<MemoryDetail, lam_core::LamError> {
    let memory: lam_core::Memory = conn
        .query_row(
            "SELECT id, project_id, scope, kind, current_revision, created_at FROM memories WHERE id = ?1",
            rusqlite::params![memory_id.to_string()],
            memory_from_row,
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .ok_or_else(|| {
            lam_core::LamError::validation(
                "E_MEMORY_NOT_FOUND",
                format!("no memory with id {memory_id}"),
            )
        })?;
    let wanted = revision_id
        .copied()
        .or(memory.current_revision)
        .ok_or_else(|| {
            lam_core::LamError::validation(
                "E_MEMORY_REVISION_MISSING",
                format!("memory {memory_id} has no current revision"),
            )
        })?;
    let revision: lam_core::MemoryRevision = conn
        .query_row(
            "SELECT id, memory_id, content, status, version, recorded_at, actor, reason
             FROM memory_revisions WHERE id = ?1 AND memory_id = ?2",
            rusqlite::params![wanted.to_string(), memory_id.to_string()],
            revision_from_row,
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .ok_or_else(|| {
            lam_core::LamError::validation(
                "E_MEMORY_REVISION_NOT_FOUND",
                format!("no revision {wanted} for memory {memory_id}"),
            )
        })?;
    let mut stmt = conn
        .prepare(
            "SELECT source_kind, source_ref, line_start, line_end, commit_sha, content_hash
             FROM memory_sources WHERE revision_id = ?1 ORDER BY rowid ASC",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![wanted.to_string()], source_from_row)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut sources = Vec::new();
    for row in rows {
        sources.push(row.map_err(|e| lam_core::LamError::storage(e.to_string()))?);
    }
    Ok(MemoryDetail {
        memory,
        revision,
        sources,
    })
}

pub fn list_revisions(
    conn: &rusqlite::Connection,
    memory_id: &lam_core::MemoryId,
) -> Result<Vec<lam_core::MemoryRevision>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, memory_id, content, status, version, recorded_at, actor, reason
             FROM memory_revisions WHERE memory_id = ?1 ORDER BY version ASC",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![memory_id.to_string()], revision_from_row)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| lam_core::LamError::storage(e.to_string()))?);
    }
    Ok(out)
}

pub fn set_status(
    conn: &mut rusqlite::Connection,
    memory_id: &lam_core::MemoryId,
    status: lam_core::MemoryStatus,
    actor: &str,
    reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<lam_core::RevisionId, lam_core::LamError> {
    if actor.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_ACTOR_INVALID",
            "actor must not be empty".to_string(),
        ));
    }
    let detail = get_memory(conn, memory_id)?;
    if detail.revision.status == status {
        return Ok(detail.revision.id);
    }
    let sources = detail.sources.clone();
    let content = detail.revision.content.clone();
    with_write_retry(conn, |tx| {
        let current: i64 = tx
            .query_row(
                "SELECT MAX(version) FROM memory_revisions WHERE memory_id = ?1",
                rusqlite::params![memory_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let revision = lam_core::MemoryRevision {
            id: lam_core::RevisionId::new(),
            memory_id: *memory_id,
            content: content.clone(),
            status,
            version: (current + 1) as u64,
            recorded_at: now,
            actor: actor.to_string(),
            reason: reason.map(|s| s.to_string()),
        };
        revision.validate()?;
        insert_revision_rows(tx, memory_id, &revision, &sources)?;
        audit_save(
            tx,
            "memory_status",
            actor,
            &memory_id.to_string(),
            Some(detail.revision.status.as_str()),
            status.as_str(),
            now,
        )?;
        Ok(revision.id)
    })
}

pub fn review_list(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
) -> Result<Vec<MemoryDetail>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT m.id FROM memories m
             JOIN memory_revisions r ON r.id = m.current_revision
             WHERE m.project_id = ?1 AND r.status IN ('proposed', 'disputed')
             ORDER BY r.recorded_at ASC",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![project_id.to_string_value()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let text = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if let Ok(id) = text.parse::<lam_core::MemoryId>() {
            out.push(get_memory(conn, &id)?);
        }
    }
    Ok(out)
}

pub fn purge_memory(
    conn: &mut rusqlite::Connection,
    memory_id: &lam_core::MemoryId,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    if actor.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_ACTOR_INVALID",
            "actor must not be empty".to_string(),
        ));
    }
    let detail = get_memory(conn, memory_id)?;
    let content_hash = blake3::hash(detail.revision.content.as_bytes())
        .to_hex()
        .to_string();
    with_write_retry(conn, |tx| {
        tx.execute(
            "DELETE FROM memory_fts WHERE memory_id = ?1",
            rusqlite::params![memory_id.to_string()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        tx.execute(
            "DELETE FROM vector_index WHERE owner_id = ?1",
            rusqlite::params![memory_id.to_string()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        tx.execute(
            "DELETE FROM memory_sources WHERE revision_id IN
             (SELECT id FROM memory_revisions WHERE memory_id = ?1)",
            rusqlite::params![memory_id.to_string()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        tx.execute(
            "DELETE FROM fact_assertions WHERE memory_revision_id IN
             (SELECT id FROM memory_revisions WHERE memory_id = ?1)",
            rusqlite::params![memory_id.to_string()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        tx.execute(
            "DELETE FROM memory_revisions WHERE memory_id = ?1",
            rusqlite::params![memory_id.to_string()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let changed = tx
            .execute(
                "DELETE FROM memories WHERE id = ?1",
                rusqlite::params![memory_id.to_string()],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if changed == 0 {
            return Err(lam_core::LamError::validation(
                "E_MEMORY_NOT_FOUND",
                format!("no memory with id {memory_id}"),
            ));
        }
        audit_save(
            tx,
            "memory_purge",
            actor,
            &memory_id.to_string(),
            Some(&content_hash),
            "purged",
            now,
        )?;
        Ok(())
    })
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

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn test_project(conn: &mut rusqlite::Connection, now: DateTime<Utc>) -> lam_core::ProjectId {
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        crate::projects::resolve_or_create(conn, leaked.path(), None, now)
            .expect("project")
            .id
    }

    fn save_input(project_id: lam_core::ProjectId) -> SaveInput {
        SaveInput {
            project_id,
            scope: lam_core::Scope::Project,
            kind: lam_core::MemoryKind::Fact,
            content: "Use Temporal for provisioning".to_string(),
            status: lam_core::MemoryStatus::Verified,
            actor: "cli".to_string(),
            reason: None,
            sources: vec![lam_core::SourceEvidence {
                source_kind: lam_core::SourceKind::Document,
                source_ref: "docs/adr/0003-use-temporal.md".to_string(),
                line_start: Some(12),
                line_end: Some(31),
                commit_sha: None,
                content_hash: None,
            }],
            idempotency_key: None,
        }
    }

    #[test]
    fn save_and_get_round_trip() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = test_project(&mut conn, clock.now());
        let mut input = save_input(project_id);
        input.content = "Use Temporal for provisioning round trip".to_string();
        let (memory_id, _) = save_memory(&mut conn, input, clock.now()).expect("save");
        let detail = get_memory(&conn, &memory_id).expect("get");
        assert_eq!(detail.revision.version, 1);
        assert_eq!(detail.sources.len(), 1);
    }

    #[test]
    fn replay_with_idempotency_key_returns_same_memory() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = test_project(&mut conn, clock.now());
        let mut first = save_input(project_id);
        first.content = "Idempotent save content".to_string();
        first.idempotency_key = Some("req-123".to_string());
        let (first_id, first_rev) = save_memory(&mut conn, first, clock.now()).expect("first save");
        let mut second = save_input(project_id);
        second.content = "Different text but same key".to_string();
        second.idempotency_key = Some("req-123".to_string());
        let (second_id, second_rev) = save_memory(&mut conn, second, clock.now()).expect("replay");
        assert_eq!(first_id, second_id);
        assert_eq!(first_rev, second_rev);
    }

    #[test]
    fn revisions_are_append_only() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = test_project(&mut conn, clock.now());
        let mut input = save_input(project_id);
        input.content = "Revision history content".to_string();
        let (memory_id, _) = save_memory(&mut conn, input, clock.now()).expect("save");
        set_status(
            &mut conn,
            &memory_id,
            lam_core::MemoryStatus::Archived,
            "cli",
            Some("superseded"),
            clock.now(),
        )
        .expect("archive");
        let revisions = list_revisions(&conn, &memory_id).expect("list");
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].version, 1);
        assert_eq!(revisions[1].version, 2);
        assert_eq!(revisions[1].status, lam_core::MemoryStatus::Archived);
        assert_eq!(revisions[0].content, revisions[1].content);
    }

    #[test]
    fn purge_removes_rows_and_keeps_audit() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = test_project(&mut conn, clock.now());
        let mut input = save_input(project_id);
        input.content = "Purge me content".to_string();
        let (memory_id, _) = save_memory(&mut conn, input, clock.now()).expect("save");
        purge_memory(&mut conn, &memory_id, "cli", clock.now()).expect("purge");
        assert!(get_memory(&conn, &memory_id).is_err());
        let fts_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_fts WHERE memory_id = ?1",
                rusqlite::params![memory_id.to_string()],
                |row| row.get(0),
            )
            .expect("count fts");
        assert_eq!(fts_count, 0);
    }

    #[test]
    fn secret_content_is_rejected() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = test_project(&mut conn, clock.now());
        let mut input = save_input(project_id);
        input.content = "token ghp_abc123def456".to_string();
        let result = save_memory(&mut conn, input, clock.now());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_SECRET_DETECTED");
    }
}
