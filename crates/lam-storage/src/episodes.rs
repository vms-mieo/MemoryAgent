use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct EpisodeHit {
    pub episode_id: String,
    pub session_id: String,
    pub abstract_text: String,
    pub snippet: String,
    pub topics: String,
    pub rank: f64,
}

fn fallback_query(raw: &str) -> Option<String> {
    let tokens: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(10)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" OR "))
    }
}

pub fn search_episodes(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    query: &str,
    limit: usize,
    now: DateTime<Utc>,
) -> Result<Vec<EpisodeHit>, lam_core::LamError> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_QUERY_INVALID",
            "query must not be empty".to_string(),
        ));
    }
    if limit == 0 || limit > 100 {
        return Err(lam_core::LamError::validation(
            "E_LIMIT_INVALID",
            "limit must be within 1 and 100".to_string(),
        ));
    }
    match search_episodes_fts(conn, project_id, trimmed, limit as i64, now) {
        Ok(hits) if !hits.is_empty() => Ok(hits),
        Ok(_) => match fallback_query(trimmed) {
            Some(fallback) if fallback != trimmed => {
                search_episodes_fts(conn, project_id, &fallback, limit as i64, now)
            }
            _ => Ok(Vec::new()),
        },
        Err(e) if crate::db::is_retryable_message(&e.to_string()) => Err(e),
        Err(original) => {
            if let Some(fallback) = fallback_query(trimmed) {
                search_episodes_fts(conn, project_id, &fallback, limit as i64, now)
                    .map_err(|_| original)
            } else {
                Err(original)
            }
        }
    }
}

fn search_episodes_fts(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    fts_query: &str,
    limit: i64,
    now: DateTime<Utc>,
) -> Result<Vec<EpisodeHit>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.session_id, e.abstract, e.topics,
                    snippet(episode_fts, 2, '<<', '>>', '...', 24),
                    bm25(episode_fts)
             FROM episode_fts
             JOIN episodes e ON e.id = episode_fts.episode_id
             JOIN sessions s ON s.id = e.session_id
             WHERE episode_fts MATCH ?1 AND s.project_id = ?2
               AND (e.expires_at IS NULL OR e.expires_at > ?3)
             ORDER BY bm25(episode_fts)
             LIMIT ?4",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                fts_query,
                project_id.to_string_value(),
                now.to_rfc3339(),
                limit,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, f64>(5)?,
                ))
            },
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let tuple = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        out.push(EpisodeHit {
            episode_id: tuple.0,
            session_id: tuple.1,
            abstract_text: tuple.2,
            snippet: tuple.4.unwrap_or_default(),
            topics: tuple.3,
            rank: tuple.5,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    #[test]
    fn episode_search_finds_checkpoint_summaries() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        let project_id =
            crate::projects::resolve_or_create(&mut conn, leaked.path(), Some("ep-proj"), at())
                .expect("project")
                .id;
        let raw = serde_json::json!({
            "schema_version": 1,
            "host": "claude-code",
            "event_type": "session.checkpoint",
            "external_session_id": "s-ep-1",
            "event_id": "checkpoint-s-ep-1",
            "cwd": leaked.path().to_string_lossy(),
            "git_branch": null,
            "timestamp": "2026-11-02T09:00:00Z",
            "payload": {
                "trigger": "auto",
                "current_task": "Wire worker deployment config",
                "blockers": ["Waiting on staging credentials"],
            },
        });
        let mut event: lam_core::NormalizedEvent = serde_json::from_value(raw).expect("event");
        crate::capture::ingest_normalized(&mut conn, &mut event, 365, at()).expect("ingest");
        let hits =
            search_episodes(&conn, &project_id, "worker deployment", 5, at()).expect("search");
        assert!(!hits.is_empty());
        assert!(hits[0].snippet.contains("worker") || hits[0].abstract_text.contains("Wire"));
    }

    #[test]
    fn expired_episodes_excluded() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        let project_id =
            crate::projects::resolve_or_create(&mut conn, leaked.path(), Some("ep-exp"), at())
                .expect("project")
                .id;
        let session = crate::projects::create_session(
            &conn,
            &project_id,
            lam_core::AgentKind::Cli,
            Some("s-old"),
            None,
            at(),
        )
        .expect("session");
        conn.execute(
            "INSERT INTO episodes (id, session_id, abstract, summary, topics, quality, expires_at)
             VALUES ('ep-old-1', ?1, 'Old abstract', 'Old summary text', 'checkpoint', 0.5, '2026-11-01T09:00:00+00:00')",
            rusqlite::params![session.id.to_string()],
        )
        .expect("insert");
        conn.execute(
            "INSERT INTO episode_fts (episode_id, abstract, summary, topics) VALUES ('ep-old-1', 'Old abstract', 'Old summary text', 'checkpoint')",
            [],
        )
        .expect("fts");
        let hits = search_episodes(&conn, &project_id, "Old summary", 5, at()).expect("search");
        assert!(hits.is_empty());
    }
}
