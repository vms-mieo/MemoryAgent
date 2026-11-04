use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct SearchParams {
    pub project_id: lam_core::ProjectId,
    pub query: String,
    pub scope: Option<lam_core::Scope>,
    pub include_history: bool,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub memory_id: lam_core::MemoryId,
    pub project_id: lam_core::ProjectId,
    pub scope: lam_core::Scope,
    pub kind: lam_core::MemoryKind,
    pub status: lam_core::MemoryStatus,
    pub snippet: String,
    pub content: String,
    pub source_ref: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub recorded_at: DateTime<Utc>,
    pub rank: f64,
}

fn validated_query(raw: &str) -> Result<String, lam_core::LamError> {
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_QUERY_INVALID",
            "query must not be empty".to_string(),
        ));
    }
    if trimmed.len() > 1024 {
        return Err(lam_core::LamError::validation(
            "E_QUERY_INVALID",
            "query exceeds 1024 bytes".to_string(),
        ));
    }
    Ok(trimmed)
}

fn validated_limit(limit: usize) -> Result<i64, lam_core::LamError> {
    if limit == 0 || limit > 100 {
        return Err(lam_core::LamError::validation(
            "E_LIMIT_INVALID",
            "limit must be within 1 and 100".to_string(),
        ));
    }
    Ok(limit as i64)
}

fn fallback_fts_query(raw: &str) -> Option<String> {
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

fn task_or_query(task: &str) -> String {
    fallback_fts_query(task).unwrap_or_else(|| task.to_string())
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

pub fn search_memories(
    conn: &rusqlite::Connection,
    params: SearchParams,
) -> Result<Vec<SearchHit>, lam_core::LamError> {
    let query = validated_query(&params.query)?;
    let limit = validated_limit(params.limit)?;
    match search_with_fts(conn, &params, &query, limit) {
        Ok(hits) if !hits.is_empty() => Ok(hits),
        Ok(_) => match fallback_fts_query(&query) {
            Some(fallback) if fallback != query => search_with_fts(conn, &params, &fallback, limit),
            _ => Ok(Vec::new()),
        },
        Err(e) if crate::db::is_retryable_message(&e.to_string()) => Err(e),
        Err(original) => {
            if let Some(fallback) = fallback_fts_query(&query) {
                search_with_fts(conn, &params, &fallback, limit).map_err(|_| original)
            } else {
                Err(original)
            }
        }
    }
}

fn search_with_fts(
    conn: &rusqlite::Connection,
    params: &SearchParams,
    fts_query: &str,
    limit: i64,
) -> Result<Vec<SearchHit>, lam_core::LamError> {
    let scope_filter = match params.scope {
        Some(scope) => format!("AND m.project_id = ?2 AND m.scope = '{}'", scope.as_str()),
        None => {
            "AND ((m.project_id = ?2 AND m.scope IN ('global','project')) OR m.scope = 'global')"
                .to_string()
        }
    };
    let history_filter = if params.include_history {
        String::new()
    } else {
        "AND r.status NOT IN ('archived','superseded','proposed','disputed')".to_string()
    };
    let sql = format!(
        "SELECT m.id, m.project_id, m.scope, m.kind,
            r.content, r.status, r.recorded_at,
            (SELECT s.source_ref FROM memory_sources s WHERE s.revision_id = r.id ORDER BY rowid ASC LIMIT 1),
            (SELECT s.line_start FROM memory_sources s WHERE s.revision_id = r.id ORDER BY rowid ASC LIMIT 1),
            (SELECT s.line_end FROM memory_sources s WHERE s.revision_id = r.id ORDER BY rowid ASC LIMIT 1),
            snippet(memory_fts, 1, '<<', '>>', '...', 24),
            bm25(memory_fts)
         FROM memory_fts
         JOIN memories m ON m.id = memory_fts.memory_id
         JOIN memory_revisions r ON r.id = m.current_revision
         WHERE memory_fts MATCH ?1 {scope_filter} {history_filter}
         ORDER BY bm25(memory_fts), r.recorded_at DESC
         LIMIT ?3"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(
            rusqlite::params![fts_query, params.project_id.to_string_value(), limit,],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, f64>(11)?,
                ))
            },
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let (
            memory_text,
            project_text,
            scope_text,
            kind_text,
            content,
            status_text,
            recorded_text,
            source_ref,
            line_start,
            line_end,
            snippet,
            rank,
        ) = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let bad = || lam_core::LamError::storage("search row has invalid typed value".to_string());
        out.push(SearchHit {
            memory_id: memory_text
                .parse::<lam_core::MemoryId>()
                .map_err(|_| bad())?,
            project_id: project_text
                .parse::<lam_core::ProjectId>()
                .map_err(|_| bad())?,
            scope: parse_scope(&scope_text).map_err(|_| bad())?,
            kind: parse_kind(&kind_text).map_err(|_| bad())?,
            status: parse_status(&status_text).map_err(|_| bad())?,
            snippet: snippet.unwrap_or_default(),
            content,
            source_ref: source_ref.unwrap_or_default(),
            line_start: line_start.map(|v| v as u32),
            line_end: line_end.map(|v| v as u32),
            recorded_at: recorded_text.parse::<DateTime<Utc>>().map_err(|_| bad())?,
            rank,
        });
    }
    Ok(out)
}

fn source_label(hit: &SearchHit) -> String {
    if hit.source_ref.is_empty() {
        return "unknown".to_string();
    }
    match (hit.line_start, hit.line_end) {
        (Some(start), Some(end)) => format!("{}:{start}-{end}", hit.source_ref),
        _ => hit.source_ref.clone(),
    }
}

pub fn build_context(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    task: &str,
    token_budget: u32,
) -> Result<lam_core::ContextResponse, lam_core::LamError> {
    let trimmed = task.trim();
    if trimmed.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_TASK_INVALID",
            "task must not be empty".to_string(),
        ));
    }
    if token_budget == 0 || token_budget > 8000 {
        return Err(lam_core::LamError::validation(
            "E_BUDGET_INVALID",
            "token_budget must be within 1 and 8000".to_string(),
        ));
    }
    let query = task_or_query(trimmed);
    let hits = search_memories(
        conn,
        SearchParams {
            project_id: *project_id,
            query,
            scope: None,
            include_history: false,
            limit: 10,
        },
    )?;
    let policy = crate::policies::get_policy(conn, project_id)
        .unwrap_or_else(|_| crate::policies::ProjectPolicy::default_for(*project_id));
    let hits: Vec<SearchHit> = if policy.strict() {
        hits.into_iter()
            .filter(|hit| hit.status == lam_core::MemoryStatus::Verified)
            .collect()
    } else {
        hits
    };
    let mut warnings = Vec::new();
    if hits.is_empty() {
        warnings.push("no matching memories for task".to_string());
    }
    let mut lines: Vec<String> = Vec::new();
    let mut items: Vec<lam_core::ContextItem> = Vec::new();
    let mut used: u32 = 0;
    for (index, hit) in hits.iter().enumerate() {
        let body = if hit.snippet.is_empty() {
            hit.content.chars().take(280).collect::<String>()
        } else {
            hit.snippet.clone()
        };
        let line = format!(
            "- [{}|{}] {} ({})",
            hit.kind.as_str(),
            hit.status.as_str(),
            body,
            source_label(hit)
        );
        let cost = lam_core::estimate_tokens(&line);
        if index > 0 && used + cost > token_budget {
            warnings.push(format!("truncated to {index} items within token budget"));
            break;
        }
        used += cost;
        lines.push(line);
        items.push(lam_core::ContextItem {
            memory_id: hit.memory_id,
            kind: hit.kind.as_str().to_string(),
            status: hit.status.as_str().to_string(),
            source: source_label(hit),
            relevance: lam_core::context::relevance_from_rank(hit.rank),
            valid_from: Some(hit.recorded_at),
        });
    }
    let context = if lines.is_empty() {
        "No verified memories matched this task.".to_string()
    } else {
        format!(
            "Reference data (untrusted, verify before acting):\n{}",
            lines.join("\n")
        )
    };
    let estimated_tokens = lam_core::estimate_tokens(&context);
    Ok(lam_core::ContextResponse {
        schema_version: 1,
        project_id: *project_id,
        task: trimmed.to_string(),
        token_budget,
        estimated_tokens,
        context,
        items,
        warnings,
    })
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

    fn seeded_project(conn: &mut rusqlite::Connection, now: DateTime<Utc>) -> lam_core::ProjectId {
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        crate::projects::resolve_or_create(conn, leaked.path(), Some("search-proj"), now)
            .expect("project")
            .id
    }

    fn save_text(
        conn: &mut rusqlite::Connection,
        project_id: lam_core::ProjectId,
        content: &str,
        now: DateTime<Utc>,
    ) -> lam_core::MemoryId {
        let (id, _) = crate::memories::save_memory(
            conn,
            crate::memories::SaveInput {
                project_id,
                scope: lam_core::Scope::Project,
                kind: lam_core::MemoryKind::Fact,
                content: content.to_string(),
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
            },
            now,
        )
        .expect("save");
        id
    }

    #[test]
    fn keyword_search_returns_ranked_provenance() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = seeded_project(&mut conn, clock.now());
        let wanted = save_text(
            &mut conn,
            project_id,
            "Temporal provisions the database workflow engine",
            clock.now(),
        );
        save_text(
            &mut conn,
            project_id,
            "Kafka streams handle click events",
            clock.now(),
        );
        let hits = search_memories(
            &conn,
            SearchParams {
                project_id,
                query: "Temporal provisioning".to_string(),
                scope: None,
                include_history: false,
                limit: 5,
            },
        )
        .expect("search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].memory_id, wanted);
        assert_eq!(hits[0].source_ref, "docs/adr/0003-use-temporal.md");
        assert_eq!(hits[0].line_start, Some(12));
    }

    #[test]
    fn phrase_query_matches_exact_code_symbol() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = seeded_project(&mut conn, clock.now());
        save_text(
            &mut conn,
            project_id,
            "provisioning.orchestrator uses temporal engine",
            clock.now(),
        );
        let hits = search_memories(
            &conn,
            SearchParams {
                project_id,
                query: "\"provisioning.orchestrator\"".to_string(),
                scope: None,
                include_history: false,
                limit: 5,
            },
        )
        .expect("search");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn archived_memories_hidden_by_default() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = seeded_project(&mut conn, clock.now());
        let memory_id = save_text(
            &mut conn,
            project_id,
            "Ephemeral debug note content",
            clock.now(),
        );
        crate::memories::set_status(
            &mut conn,
            &memory_id,
            lam_core::MemoryStatus::Archived,
            "cli",
            None,
            clock.now(),
        )
        .expect("archive");
        let params = SearchParams {
            project_id,
            query: "Ephemeral debug".to_string(),
            scope: None,
            include_history: false,
            limit: 5,
        };
        assert!(search_memories(&conn, params).expect("search").is_empty());
    }

    #[test]
    fn private_memories_hidden_by_default() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = seeded_project(&mut conn, clock.now());
        save_text(
            &mut conn,
            project_id,
            "Shared project launch checklist",
            clock.now(),
        );
        let (private_id, _) = crate::memories::save_memory(
            &mut conn,
            crate::memories::SaveInput {
                project_id,
                scope: lam_core::Scope::AgentPrivate,
                kind: lam_core::MemoryKind::Fact,
                content: "Private launch checklist".to_string(),
                status: lam_core::MemoryStatus::Verified,
                actor: "cli".to_string(),
                reason: None,
                sources: vec![],
                idempotency_key: None,
            },
            clock.now(),
        )
        .expect("save private");
        let _ = private_id;
        let default_hits = search_memories(
            &conn,
            SearchParams {
                project_id,
                query: "launch checklist".to_string(),
                scope: None,
                include_history: false,
                limit: 5,
            },
        )
        .expect("search");
        assert!(default_hits
            .iter()
            .all(|hit| hit.scope == lam_core::Scope::Project));
        let explicit_hits = search_memories(
            &conn,
            SearchParams {
                project_id,
                query: "launch checklist".to_string(),
                scope: Some(lam_core::Scope::AgentPrivate),
                include_history: false,
                limit: 5,
            },
        )
        .expect("search private");
        assert_eq!(explicit_hits.len(), 1);
    }

    #[test]
    fn context_response_respects_token_budget() {
        let mut conn = migrated_conn();
        let clock = ManualClock::at(2026, 11, 2, 10, 0, 0);
        let project_id = seeded_project(&mut conn, clock.now());
        save_text(
            &mut conn,
            project_id,
            "Temporal workflow decision record one",
            clock.now(),
        );
        save_text(
            &mut conn,
            project_id,
            "Temporal workflow decision record two",
            clock.now(),
        );
        let response =
            build_context(&conn, &project_id, "Temporal workflow", 200).expect("context");
        assert!(response.estimated_tokens <= 1200);
        assert!(!response.items.is_empty());
        assert_eq!(response.schema_version, 1);
    }
}
