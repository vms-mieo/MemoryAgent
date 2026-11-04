use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct DedupSuggestion {
    pub id: String,
    pub memory_a_id: String,
    pub memory_b_id: String,
    pub score: f64,
    pub reason: String,
    pub status: String,
}

pub fn normalize_content(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn content_fingerprint(text: &str) -> String {
    blake3::hash(format!("lam-dedup:v1:{}", normalize_content(text)).as_bytes())
        .to_hex()
        .to_string()
}

pub fn scan_project(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    now: DateTime<Utc>,
) -> Result<usize, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT m.id, r.content FROM memories m
             JOIN memory_revisions r ON r.id = m.current_revision
             WHERE m.project_id = ?1 AND r.status NOT IN ('archived', 'superseded')",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![project_id.to_string_value()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut by_hash: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for row in rows {
        let (id, content) = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        by_hash
            .entry(content_fingerprint(&content))
            .or_default()
            .push(id);
    }
    let mut created = 0;
    for members in by_hash.values() {
        if members.len() < 2 {
            continue;
        }
        let mut sorted = members.clone();
        sorted.sort();
        for pair in sorted.windows(2) {
            let (first, second) = (pair[0].clone(), pair[1].clone());
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM dedup_suggestions
                     WHERE project_id = ?1 AND status = 'open'
                       AND ((memory_a_id = ?2 AND memory_b_id = ?3) OR (memory_a_id = ?3 AND memory_b_id = ?2))",
                    rusqlite::params![project_id.to_string_value(), first, second],
                    |row| row.get(0),
                )
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            if exists > 0 {
                continue;
            }
            let id = format!(
                "dedup-{}",
                &blake3::hash(format!("lam-dedup:v1:{}:{first}:{second}", project_id).as_bytes())
                    .to_hex()[..16]
            );
            conn.execute(
                "INSERT INTO dedup_suggestions (id, project_id, memory_a_id, memory_b_id, score, reason, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1.0, 'identical normalized content', 'open', ?5)",
                rusqlite::params![id, project_id.to_string_value(), first, second, now.to_rfc3339()],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            created += 1;
        }
    }
    lam_storage::harden_current_db(conn);
    Ok(created)
}

pub fn list_suggestions(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    open_only: bool,
) -> Result<Vec<DedupSuggestion>, lam_core::LamError> {
    let mut sql = "SELECT id, memory_a_id, memory_b_id, score, reason, status FROM dedup_suggestions WHERE project_id = ?1".to_string();
    if open_only {
        sql.push_str(" AND status = 'open'");
    }
    sql.push_str(" ORDER BY created_at ASC");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![project_id.to_string_value()], |row| {
            Ok(DedupSuggestion {
                id: row.get(0)?,
                memory_a_id: row.get(1)?,
                memory_b_id: row.get(2)?,
                score: row.get(3)?,
                reason: row.get(4)?,
                status: row.get(5)?,
            })
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| lam_core::LamError::storage(e.to_string()))?);
    }
    Ok(out)
}

pub fn resolve_suggestion(
    conn: &mut rusqlite::Connection,
    suggestion_id: &str,
    keep_memory_id: Option<&str>,
    actor: &str,
    reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<String, lam_core::LamError> {
    if actor.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_ACTOR_INVALID",
            "actor must not be empty".to_string(),
        ));
    }
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT project_id, memory_a_id, memory_b_id, status FROM dedup_suggestions WHERE id = ?1",
            rusqlite::params![suggestion_id.trim()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let (_, memory_a, memory_b, status) = row.ok_or_else(|| {
        lam_core::LamError::validation(
            "E_DEDUP_NOT_FOUND",
            format!("no dedup suggestion {}", suggestion_id.trim()),
        )
    })?;
    if status != "open" {
        return Err(lam_core::LamError::validation(
            "E_DEDUP_STATE",
            format!(
                "dedup suggestion {} is already {status}",
                suggestion_id.trim()
            ),
        ));
    }
    let outcome = match keep_memory_id.map(str::trim) {
        None | Some("") => {
            conn.execute(
                "UPDATE dedup_suggestions SET status = 'dismissed' WHERE id = ?1",
                rusqlite::params![suggestion_id.trim()],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            "dismissed".to_string()
        }
        Some(keep) => {
            let keep_id: lam_core::MemoryId = keep.parse().map_err(|_| {
                lam_core::LamError::validation(
                    "E_MEMORY_ID_INVALID",
                    format!("invalid memory id: {keep}"),
                )
            })?;
            let loser = if keep_id.to_string() == memory_a {
                memory_b.clone()
            } else if keep_id.to_string() == memory_b {
                memory_a.clone()
            } else {
                return Err(lam_core::LamError::validation(
                    "E_DEDUP_KEEP_INVALID",
                    "kept memory must be one of the pair".to_string(),
                ));
            };
            let loser_id: lam_core::MemoryId = loser
                .parse()
                .map_err(|_| lam_core::LamError::storage("stored memory id invalid".to_string()))?;
            let fallback;
            let reason_ref = match reason {
                Some(text) => text,
                None => {
                    fallback = format!("duplicate of {keep}");
                    &fallback
                }
            };
            lam_storage::set_status(
                conn,
                &loser_id,
                lam_core::MemoryStatus::Archived,
                actor,
                Some(reason_ref),
                now,
            )?;
            conn.execute(
                "UPDATE dedup_suggestions SET status = 'merged' WHERE id = ?1",
                rusqlite::params![suggestion_id.trim()],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            format!("merged keeping {keep}")
        }
    };
    lam_storage::harden_current_db(conn);
    Ok(outcome)
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

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = lam_storage::open_in_memory(3000).expect("open memory");
        lam_storage::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    fn save_text(
        conn: &mut rusqlite::Connection,
        project: &lam_core::ProjectId,
        content: &str,
    ) -> lam_core::MemoryId {
        let (id, _) = lam_storage::save_memory(
            conn,
            lam_storage::SaveInput {
                project_id: *project,
                scope: lam_core::Scope::Project,
                kind: lam_core::MemoryKind::Note,
                content: content.to_string(),
                status: lam_core::MemoryStatus::Verified,
                actor: "cli".to_string(),
                reason: None,
                sources: vec![],
                idempotency_key: None,
            },
            at(),
        )
        .expect("save");
        id
    }

    #[test]
    fn identical_content_produces_one_suggestion() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        let project =
            lam_storage::resolve_or_create(&mut conn, leaked.path(), Some("dup-proj"), at())
                .expect("project")
                .id;
        save_text(&mut conn, &project, "Restart cadence is weekly on Fridays");
        save_text(
            &mut conn,
            &project,
            "  restart   CADENCE is weekly   on fridays  ",
        );
        save_text(&mut conn, &project, "Completely unrelated note here");
        let created = scan_project(&conn, &project, at()).expect("scan");
        assert_eq!(created, 1);
        let again = scan_project(&conn, &project, at()).expect("rescan");
        assert_eq!(again, 0);
        let open = list_suggestions(&conn, &project, true).expect("list");
        assert_eq!(open.len(), 1);
    }

    #[test]
    fn merge_archives_loser_and_keeps_winner() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        let project =
            lam_storage::resolve_or_create(&mut conn, leaked.path(), Some("merge-proj"), at())
                .expect("project")
                .id;
        let winner = save_text(&mut conn, &project, "Keep this canonical note");
        let _loser = save_text(&mut conn, &project, "keep THIS canonical NOTE");
        scan_project(&conn, &project, at()).expect("scan");
        let open = list_suggestions(&conn, &project, true).expect("list");
        assert_eq!(open.len(), 1);
        let outcome = resolve_suggestion(
            &mut conn,
            &open[0].id,
            Some(&winner.to_string()),
            "cli",
            None,
            at(),
        )
        .expect("merge");
        assert!(outcome.starts_with("merged"));
        assert!(list_suggestions(&conn, &project, true)
            .expect("list")
            .is_empty());
    }

    #[test]
    fn dismiss_leaves_memories_alone() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        let project =
            lam_storage::resolve_or_create(&mut conn, leaked.path(), Some("dismiss-proj"), at())
                .expect("project")
                .id;
        save_text(&mut conn, &project, "Same text here");
        save_text(&mut conn, &project, "same TEXT here");
        scan_project(&conn, &project, at()).expect("scan");
        let open = list_suggestions(&conn, &project, true).expect("list");
        resolve_suggestion(&mut conn, &open[0].id, None, "cli", None, at()).expect("dismiss");
        assert!(list_suggestions(&conn, &project, true)
            .expect("list")
            .is_empty());
        assert_eq!(
            list_suggestions(&conn, &project, false).expect("all").len(),
            1
        );
    }
}
