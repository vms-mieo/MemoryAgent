use chrono::DateTime;
use chrono::Utc;
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct IngestOutcome {
    pub stored: bool,
    pub reason: &'static str,
    pub project_id: Option<lam_core::ProjectId>,
    pub session_id: Option<lam_core::SessionId>,
    pub event_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredEpisode {
    pub id: String,
    pub abstract_text: String,
    pub summary: String,
    pub topics: String,
    pub recorded_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredWorkingState {
    pub task_key: String,
    pub state_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Resume {
    pub project_id: lam_core::ProjectId,
    pub session_id: Option<lam_core::SessionId>,
    pub working_state: Option<StoredWorkingState>,
    pub last_episode: Option<StoredEpisode>,
    pub external_session_id: Option<String>,
}

pub fn agent_kind_for_host(host: &str) -> lam_core::AgentKind {
    lam_core::AgentKind::from_str(host).unwrap_or(lam_core::AgentKind::Cli)
}

pub fn ingest_normalized(
    conn: &mut rusqlite::Connection,
    event: &mut lam_core::NormalizedEvent,
    episodic_ttl_days: u64,
    now: DateTime<Utc>,
) -> Result<IngestOutcome, lam_core::LamError> {
    let decision = lam_adapters::triage(event);
    if !decision.store {
        return Ok(IngestOutcome {
            stored: false,
            reason: decision.reason,
            project_id: None,
            session_id: None,
            event_key: None,
        });
    }
    let canonical = lam_storage_path(event)?;
    let mut project = crate::projects::resolve_or_create(conn, &canonical, None, now)?;
    project = crate::projects::lookup_by_id(conn, &project.id)?;
    let policy = crate::policies::get_policy(conn, &project.id)?;
    if !policy.capture_enabled {
        return Ok(IngestOutcome {
            stored: false,
            reason: "project_opted_out",
            project_id: Some(project.id),
            session_id: None,
            event_key: None,
        });
    }
    if let Some(denied) = denied_transcript_path(&policy, event) {
        return Ok(IngestOutcome {
            stored: false,
            reason: denied,
            project_id: Some(project.id),
            session_id: None,
            event_key: None,
        });
    }
    filter_denied_files(&policy, event);
    let session = ensure_session(conn, &project, event, now)?;
    let key = crate::projects::record_event(conn, &session.id, event)?;
    match event.event_type.as_str() {
        "session.end" => {
            store_episode(
                conn,
                &session.id,
                &episode_abstract(event),
                &episode_summary(event),
                "session-end",
                episodic_ttl_days,
                now,
            )?;
            let _ = crate::projects::end_session(conn, &session.id, now);
        }
        "session.stop" | "session.idle" => {
            store_episode(
                conn,
                &session.id,
                &episode_abstract(event),
                &episode_summary(event),
                "session-delta",
                episodic_ttl_days,
                now,
            )?;
        }
        "session.checkpoint" => {
            store_working_state(conn, &session.id, event, now)?;
            store_episode(
                conn,
                &session.id,
                &episode_abstract(event),
                &episode_summary(event),
                "checkpoint",
                episodic_ttl_days,
                now,
            )?;
        }
        _ => {}
    }
    crate::db::harden_current_db(conn);
    Ok(IngestOutcome {
        stored: true,
        reason: decision.reason,
        project_id: Some(project.id),
        session_id: Some(session.id),
        event_key: Some(key),
    })
}

fn denied_transcript_path(
    policy: &crate::policies::ProjectPolicy,
    event: &lam_core::NormalizedEvent,
) -> Option<&'static str> {
    let transcript = event.payload.get("transcript_path")?.as_str()?;
    if policy.denied(transcript) {
        Some("deny_path_transcript")
    } else {
        None
    }
}

fn filter_denied_files(
    policy: &crate::policies::ProjectPolicy,
    event: &mut lam_core::NormalizedEvent,
) {
    let Some(files) = event.payload.get_mut("files_touched") else {
        return;
    };
    let Some(list) = files.as_array_mut() else {
        return;
    };
    list.retain(|entry| {
        entry
            .as_str()
            .map(|text| !policy.denied(text))
            .unwrap_or(true)
    });
}

fn lam_storage_path(
    event: &lam_core::NormalizedEvent,
) -> Result<std::path::PathBuf, lam_core::LamError> {
    let path = std::path::Path::new(&event.cwd);
    if event.cwd.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_CWD_MISSING",
            "event carries no working directory".to_string(),
        ));
    }
    Ok(path.to_path_buf())
}

fn ensure_session(
    conn: &rusqlite::Connection,
    project: &lam_core::Project,
    event: &lam_core::NormalizedEvent,
    now: DateTime<Utc>,
) -> Result<lam_core::Session, lam_core::LamError> {
    if let Some(found) = find_session(conn, &project.id, &event.external_session_id)? {
        return Ok(found);
    }
    crate::projects::create_session(
        conn,
        &project.id,
        agent_kind_for_host(&event.host),
        Some(&event.external_session_id),
        event.git_branch.as_deref(),
        event.timestamp.min(now),
    )
}

fn find_session(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    external_session_id: &str,
) -> Result<Option<lam_core::Session>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, project_id, agent_kind, external_session_id, branch, started_at, ended_at
             FROM sessions WHERE project_id = ?1 AND external_session_id = ?2
             ORDER BY started_at DESC LIMIT 1",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let row: Option<lam_core::Session> = stmt
        .query_row(
            rusqlite::params![project_id.to_string_value(), external_session_id,],
            session_from_row,
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(row)
}

fn session_from_row(row: &rusqlite::Row) -> Result<lam_core::Session, rusqlite::Error> {
    let bad = |index: usize, name: &str| {
        rusqlite::Error::InvalidColumnType(index, name.to_string(), rusqlite::types::Type::Text)
    };
    let id_text: String = row.get(0)?;
    let project_text: String = row.get(1)?;
    let agent_text: String = row.get(2)?;
    let external: Option<String> = row.get(3)?;
    let branch: Option<String> = row.get(4)?;
    let started_text: String = row.get(5)?;
    let ended_text: Option<String> = row.get(6)?;
    let id = id_text
        .parse::<lam_core::SessionId>()
        .map_err(|_| bad(0, "id"))?;
    let project_id = project_text
        .parse::<lam_core::ProjectId>()
        .map_err(|_| bad(1, "project_id"))?;
    let agent_kind =
        lam_core::AgentKind::from_str(&agent_text).map_err(|_| bad(2, "agent_kind"))?;
    let started_at = started_text
        .parse::<DateTime<Utc>>()
        .map_err(|_| bad(5, "started_at"))?;
    let ended_at = ended_text
        .map(|text| text.parse::<DateTime<Utc>>())
        .transpose()
        .map_err(|_| bad(6, "ended_at"))?;
    Ok(lam_core::Session {
        id,
        project_id,
        agent_kind,
        external_session_id: external,
        branch,
        started_at,
        ended_at,
    })
}

fn episode_abstract(event: &lam_core::NormalizedEvent) -> String {
    for key in ["current_task", "summary", "notes", "last_message_excerpt"] {
        if let Some(text) = event.payload.get(key).and_then(excerpt_of) {
            if !text.trim().is_empty() {
                return truncate_chars(&text, 280);
            }
        }
    }
    match event.event_type.as_str() {
        "session.end" => "Session ended".to_string(),
        "session.checkpoint" => "Pre-compact checkpoint".to_string(),
        "session.idle" => "Session idle delta".to_string(),
        _ => "Session delta".to_string(),
    }
}

fn episode_summary(event: &lam_core::NormalizedEvent) -> String {
    let mut parts: Vec<String> = Vec::new();
    for key in [
        "goal",
        "current_task",
        "completed_steps",
        "next_steps",
        "blockers",
        "files_touched",
        "summary",
        "notes",
        "last_message_excerpt",
    ] {
        if let Some(value) = event.payload.get(key) {
            let text = match value {
                serde_json::Value::String(single) => single.clone(),
                serde_json::Value::Array(items) => items
                    .iter()
                    .filter_map(|entry| entry.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
                _ => continue,
            };
            if !text.trim().is_empty() {
                parts.push(format!("{key}: {}", truncate_chars(&text, 500)));
            }
        }
    }
    if parts.is_empty() {
        return format!("{} recorded", event.event_type);
    }
    parts.join("\n")
}

fn excerpt_of(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|entry| entry.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            if joined.trim().is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() > limit {
        text.chars().take(limit).collect()
    } else {
        text.to_string()
    }
}

fn store_episode(
    conn: &rusqlite::Connection,
    session_id: &lam_core::SessionId,
    abstract_text: &str,
    summary: &str,
    topics: &str,
    episodic_ttl_days: u64,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    let expires_at = now + chrono::Duration::days(episodic_ttl_days.max(1) as i64);
    let episode_id = lam_core::RevisionId::new().to_string();
    conn.execute(
        "INSERT INTO episodes (id, session_id, abstract, summary, topics, quality, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0.5, ?6)",
        rusqlite::params![
            episode_id,
            session_id.to_string(),
            abstract_text,
            summary,
            topics,
            expires_at.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO episode_fts (episode_id, abstract, summary, topics) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![episode_id, abstract_text, summary, topics],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(())
}

fn store_working_state(
    conn: &rusqlite::Connection,
    session_id: &lam_core::SessionId,
    event: &lam_core::NormalizedEvent,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    let state_json = if has_working_fields(&event.payload) {
        serde_json::to_string(&event.payload).map_err(lam_core::LamError::from)?
    } else {
        let synthesized = synthesize_working_state(conn, session_id, event)?;
        serde_json::to_string(&synthesized).map_err(lam_core::LamError::from)?
    };
    conn.execute(
        "INSERT INTO working_states (session_id, task_key, state_json, updated_at)
         VALUES (?1, 'latest', ?2, ?3)
         ON CONFLICT(session_id) DO UPDATE SET state_json = excluded.state_json, updated_at = excluded.updated_at",
        rusqlite::params![session_id.to_string(), state_json, now.to_rfc3339(),],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(())
}

fn has_working_fields(payload: &HashMap<String, serde_json::Value>) -> bool {
    ["goal", "current_task", "next_steps", "blockers"]
        .iter()
        .any(|key| {
            payload
                .get(*key)
                .and_then(|value| value.as_str())
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false)
        })
}

fn synthesize_working_state(
    conn: &rusqlite::Connection,
    session_id: &lam_core::SessionId,
    event: &lam_core::NormalizedEvent,
) -> Result<serde_json::Value, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT kind, sanitized_payload FROM session_events
             WHERE session_id = ?1 ORDER BY rowid DESC LIMIT 30",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![session_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut current_task = String::new();
    let mut recent_tools: Vec<String> = Vec::new();
    let mut files_touched: Vec<String> = Vec::new();
    let mut recent_errors: Vec<String> = Vec::new();
    for row in rows {
        let (kind, raw) = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let payload: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
        match kind.as_str() {
            "prompt.submit" => {
                if current_task.is_empty() {
                    if let Some(text) = payload
                        .get("prompt_excerpt")
                        .and_then(|value| value.as_str())
                    {
                        current_task = truncate_chars(text, 500);
                    }
                }
            }
            "tool.use" => {
                let tool = payload
                    .get("tool_name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let detail = first_path_like(payload.get("tool_input")).unwrap_or_default();
                let entry = if detail.is_empty() {
                    tool.to_string()
                } else {
                    format!("{tool} {detail}")
                };
                if recent_tools.len() < 8 && !recent_tools.contains(&entry) {
                    recent_tools.push(entry);
                }
                if let Some(path) = first_path_like(payload.get("tool_input")) {
                    if files_touched.len() < 8 && !files_touched.contains(&path) {
                        files_touched.push(path);
                    }
                }
                if let Some(response) = payload
                    .get("tool_response")
                    .and_then(|value| value.as_str())
                {
                    for line in response.lines() {
                        let lower = line.to_lowercase();
                        if recent_errors.len() >= 5 {
                            break;
                        }
                        if lower.contains("fail")
                            || lower.contains("error")
                            || lower.contains("panic")
                            || lower.contains("non-zero")
                        {
                            let excerpt: String = line.chars().take(200).collect();
                            if !excerpt.trim().is_empty() && !recent_errors.contains(&excerpt) {
                                recent_errors.push(excerpt);
                            }
                        }
                    }
                }
            }
            "session.stop" | "session.end" if current_task.is_empty() => {
                current_task = payload
                    .get("last_message_excerpt")
                    .and_then(|value| value.as_str())
                    .map(|text| truncate_chars(text, 500))
                    .unwrap_or_default();
            }
            "session.stop" | "session.end" => {}
            _ => {}
        }
    }
    if current_task.is_empty() {
        current_task = event
            .payload
            .get("last_message_excerpt")
            .and_then(|value| value.as_str())
            .map(|text| truncate_chars(text, 500))
            .unwrap_or_default();
    }
    let mut state = serde_json::Map::new();
    if let Some(trigger) = event.payload.get("trigger").cloned() {
        state.insert("trigger".to_string(), trigger);
    }
    if !current_task.trim().is_empty() {
        state.insert(
            "current_task".to_string(),
            serde_json::Value::String(current_task),
        );
    }
    if !recent_tools.is_empty() {
        state.insert("recent_tools".to_string(), serde_json::json!(recent_tools));
    }
    if !files_touched.is_empty() {
        state.insert(
            "files_touched".to_string(),
            serde_json::json!(files_touched),
        );
    }
    if !recent_errors.is_empty() {
        state.insert(
            "recent_errors".to_string(),
            serde_json::json!(recent_errors),
        );
    }
    state.insert("synthesized".to_string(), serde_json::Value::Bool(true));
    Ok(serde_json::Value::Object(state))
}

fn first_path_like(value: Option<&serde_json::Value>) -> Option<String> {
    let input = value?;
    if let Some(text) = input.as_str() {
        return path_like_text(text);
    }
    if let Some(map) = input.as_object() {
        for key in ["file", "path", "file_path", "command", "target"] {
            if let Some(text) = map.get(key).and_then(|entry| entry.as_str()) {
                if let Some(found) = path_like_text(text) {
                    return Some(found);
                }
            }
        }
        for (_, entry) in map.iter().take(8) {
            if let Some(text) = entry.as_str() {
                if let Some(found) = path_like_text(text) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn path_like_text(text: &str) -> Option<String> {
    let candidate = text.trim();
    if candidate.len() < 2 || candidate.len() > 200 || candidate.contains('\n') {
        return None;
    }
    if candidate.contains('/')
        && !candidate.starts_with("http")
        && candidate.chars().any(|c| c.is_alphanumeric())
    {
        return Some(candidate.to_string());
    }
    None
}

pub fn load_resume(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
) -> Result<Resume, lam_core::LamError> {
    let mut session_stmt = conn
        .prepare(
            "SELECT id, project_id, agent_kind, external_session_id, branch, started_at, ended_at
             FROM sessions WHERE project_id = ?1 ORDER BY started_at DESC LIMIT 1",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let session: Option<lam_core::Session> = session_stmt
        .query_row(
            rusqlite::params![project_id.to_string_value()],
            session_from_row,
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let session = match session {
        Some(found) => found,
        None => {
            return Ok(Resume {
                project_id: *project_id,
                session_id: None,
                working_state: None,
                last_episode: None,
                external_session_id: None,
            });
        }
    };
    let working_state = conn
        .query_row(
            "SELECT task_key, state_json, updated_at FROM working_states WHERE session_id = ?1",
            rusqlite::params![session.id.to_string()],
            |row| {
                Ok(StoredWorkingState {
                    task_key: row.get(0)?,
                    state_json: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let last_episode = conn
        .query_row(
            "SELECT id, abstract, summary, topics FROM episodes
             WHERE session_id = ?1 ORDER BY rowid DESC LIMIT 1",
            rusqlite::params![session.id.to_string()],
            |row| {
                Ok(StoredEpisode {
                    id: row.get(0)?,
                    abstract_text: row.get(1)?,
                    summary: row.get(2)?,
                    topics: row.get(3)?,
                    recorded_at: None,
                })
            },
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(Resume {
        project_id: *project_id,
        session_id: Some(session.id),
        working_state,
        last_episode,
        external_session_id: session.external_session_id,
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

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> DateTime<Utc> {
        "2026-11-02T00:00:00Z".parse().expect("fixed time")
    }

    fn raw_event(payload: serde_json::Value) -> lam_core::NormalizedEvent {
        serde_json::from_value(payload).expect("event json")
    }

    fn start_event(cwd: &str, session: &str) -> lam_core::NormalizedEvent {
        raw_event(serde_json::json!({
            "schema_version": 1,
            "host": "claude-code",
            "event_type": "session.start",
            "external_session_id": session,
            "event_id": format!("start-{session}"),
            "cwd": cwd,
            "git_branch": null,
            "timestamp": "2026-11-02T00:00:00Z",
            "payload": {"source": "startup"},
        }))
    }

    fn checkpoint_event(cwd: &str, session: &str) -> lam_core::NormalizedEvent {
        raw_event(serde_json::json!({
            "schema_version": 1,
            "host": "claude-code",
            "event_type": "session.checkpoint",
            "external_session_id": session,
            "event_id": format!("checkpoint-{session}"),
            "cwd": cwd,
            "git_branch": "feature/provisioning",
            "timestamp": "2026-11-02T00:01:00Z",
            "payload": {
                "trigger": "auto",
                "current_task": "Wire worker deployment config",
                "next_steps": ["Apply migration"],
                "blockers": ["Waiting on staging credentials"],
            },
        }))
    }

    #[test]
    fn ingest_start_creates_session_once() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_string_lossy().to_string();
        let first =
            ingest_normalized(&mut conn, &mut start_event(&cwd, "s-1"), 365, at()).expect("ingest");
        let second =
            ingest_normalized(&mut conn, &mut start_event(&cwd, "s-1"), 365, at()).expect("replay");
        assert!(first.stored);
        assert!(second.stored);
        assert_eq!(first.session_id, second.session_id);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 1);
    }

    #[test]
    fn transient_events_store_nothing() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_string_lossy().to_string();
        let event = raw_event(serde_json::json!({
            "schema_version": 1,
            "host": "codex",
            "event_type": "agent.notification",
            "external_session_id": "s-9",
            "event_id": "note-1",
            "cwd": cwd,
            "git_branch": null,
            "timestamp": "2026-11-02T00:00:00Z",
            "payload": {},
        }));
        let mut event = event;
        let outcome = ingest_normalized(&mut conn, &mut event, 365, at()).expect("ingest");
        assert!(!outcome.stored);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn checkpoint_round_trips_through_resume() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_string_lossy().to_string();
        ingest_normalized(&mut conn, &mut start_event(&cwd, "s-7"), 365, at()).expect("start");
        ingest_normalized(&mut conn, &mut checkpoint_event(&cwd, "s-7"), 365, at())
            .expect("checkpoint");
        let project =
            crate::projects::resolve_existing_by_path(&conn, dir.path()).expect("project");
        let resume = load_resume(&conn, &project.id).expect("resume");
        let state = resume.working_state.expect("working state");
        assert!(state.state_json.contains("Wire worker deployment config"));
        let episode = resume.last_episode.expect("episode");
        assert!(episode.summary.contains("Waiting on staging credentials"));
    }

    #[test]
    fn replayed_end_is_idempotent() {
        let mut conn = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_string_lossy().to_string();
        let mut end = raw_event(serde_json::json!({
            "schema_version": 1,
            "host": "codex",
            "event_type": "session.end",
            "external_session_id": "s-3",
            "event_id": "end-s-3",
            "cwd": cwd,
            "git_branch": null,
            "timestamp": "2026-11-02T00:05:00Z",
            "payload": {"reason": "other"},
        }));
        ingest_normalized(&mut conn, &mut end, 365, at()).expect("first");
        ingest_normalized(&mut conn, &mut end, 365, at()).expect("replay");
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
            .expect("count events");
        assert_eq!(events, 1);
    }
}
