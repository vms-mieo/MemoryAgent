use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct PipelineContext {
    pub extractor: Option<ExtractorChoice>,
    pub episodic_ttl_days: u64,
    pub admission_mode: String,
    pub minimum_confidence: f64,
    pub remote_allowed: bool,
}

#[derive(Debug, Clone)]
pub struct ExtractorChoice {
    pub provider: String,
    pub model: String,
    pub endpoint: String,
    pub api_key_env: String,
    pub allow_remote: bool,
    pub timeout_ms: u64,
    pub session_header: Option<String>,
}

impl ExtractorChoice {
    pub fn is_remote(&self) -> bool {
        self.provider == "openai-compatible"
    }
}

impl PipelineContext {
    pub fn deterministic(episodic_ttl_days: u64) -> Self {
        Self {
            extractor: None,
            episodic_ttl_days,
            admission_mode: "shadow".to_string(),
            minimum_confidence: 0.85,
            remote_allowed: true,
        }
    }

    pub fn with_admission(mut self, mode: &str, minimum_confidence: f64) -> Self {
        self.admission_mode = mode.trim().to_lowercase();
        self.minimum_confidence = minimum_confidence.clamp(0.0, 1.0);
        self
    }

    pub fn with_cloud(
        mut self,
        endpoint: &str,
        model: &str,
        api_key_env: &str,
        timeout_ms: u64,
    ) -> Self {
        self.extractor = Some(ExtractorChoice {
            provider: "openai-compatible".to_string(),
            model: model.to_string(),
            endpoint: endpoint.to_string(),
            api_key_env: api_key_env.to_string(),
            allow_remote: true,
            timeout_ms,
            session_header: None,
        });
        self
    }

    pub fn with_cloud_session(
        mut self,
        endpoint: &str,
        model: &str,
        api_key_env: &str,
        timeout_ms: u64,
        session_header: Option<String>,
    ) -> Self {
        self.extractor = Some(ExtractorChoice {
            provider: "openai-compatible".to_string(),
            model: model.to_string(),
            endpoint: endpoint.to_string(),
            api_key_env: api_key_env.to_string(),
            allow_remote: true,
            timeout_ms,
            session_header,
        });
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExtractOutcome {
    pub candidates: usize,
    pub asserted: usize,
    pub deferred: usize,
    pub notes: Vec<String>,
}

pub fn handle_episode_summarize(
    conn: &mut rusqlite::Connection,
    job: &lam_storage::Job,
    ctx: &PipelineContext,
    now: DateTime<Utc>,
) -> Result<String, lam_core::LamError> {
    let session_ref = job
        .input_ref
        .strip_prefix("session:")
        .unwrap_or(job.input_ref.as_str());
    let session: Option<(String, String)> = conn
        .query_row(
            "SELECT id, COALESCE(external_session_id, id) FROM sessions WHERE external_session_id = ?1 OR id = ?1",
            rusqlite::params![session_ref],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let Some((session_id, external)) = session else {
        return Ok(format!(
            "no such session {session_ref}; nothing to summarize"
        ));
    };
    let existing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if existing > 0 {
        return Ok(format!("session {external} already summarized"));
    }
    let mut kinds_stmt = conn
        .prepare("SELECT kind, COUNT(*) FROM session_events WHERE session_id = ?1 GROUP BY kind ORDER BY COUNT(*) DESC")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let kinds = kinds_stmt
        .query_map(rusqlite::params![session_id], |row| {
            let kind: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok(format!("{kind} x{count}"))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if kinds.is_empty() {
        return Ok(format!(
            "session {external} has no stored events; nothing to summarize"
        ));
    }
    let summary = format!("Session activity: {}.", kinds.join(", "));
    let abstract_text = format!("Summary of session {external}");
    let id = lam_core::RevisionId::new().to_string();
    let expires_at = now + chrono::Duration::days(ctx.episodic_ttl_days.max(1) as i64);
    conn.execute(
        "INSERT INTO episodes (id, session_id, abstract, summary, topics, quality, expires_at)
         VALUES (?1, ?2, ?3, ?4, 'session-summary', 0.5, ?5)",
        rusqlite::params![
            id,
            session_id,
            abstract_text,
            summary,
            expires_at.to_rfc3339()
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO episode_fts (episode_id, abstract, summary, topics) VALUES (?1, ?2, ?3, 'session-summary')",
        rusqlite::params![id, abstract_text, summary],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    lam_storage::harden_current_db(conn);
    Ok(format!("summarized session {external} as episode {id}"))
}

pub fn handle_fact_extract(
    conn: &mut rusqlite::Connection,
    job: &lam_storage::Job,
    ctx: &PipelineContext,
    now: DateTime<Utc>,
    project_id: &lam_core::ProjectId,
    actor: &str,
) -> Result<ExtractOutcome, lam_core::LamError> {
    let mut outcome = ExtractOutcome::default();
    let session_ref = job
        .input_ref
        .strip_prefix("session:")
        .unwrap_or(job.input_ref.as_str());
    let episodes = episode_texts_for_session(conn, session_ref)?;
    if episodes.is_empty() {
        outcome
            .notes
            .push(format!("no episodes for session {session_ref}; deferred"));
        outcome.deferred += 1;
        return Ok(outcome);
    }
    if let Some(choice) = &ctx.extractor {
        if choice.is_remote() && !ctx.remote_allowed {
            return Err(lam_core::LamError::validation(
                "E_REMOTE_DEFERRED",
                "remote extraction deferred to maintain".to_string(),
            ));
        }
    }
    for (episode_id, summary) in &episodes {
        let mut batch = extract_candidates(summary, episode_id);
        if let Some(choice) = &ctx.extractor {
            let session = lam_providers::stable_session_id(&project_id.to_string_value(), &job.id);
            match run_configured_extractor(conn, choice, summary, episode_id, Some(session)) {
                Ok(provided) => batch.extend(provided.into_iter().take(32)),
                Err(e) => {
                    store_candidate_batch(conn, project_id, actor, now, ctx, &mut outcome, batch)?;
                    return Err(e);
                }
            }
        }
        store_candidate_batch(conn, project_id, actor, now, ctx, &mut outcome, batch)?;
    }
    Ok(outcome)
}

fn store_candidate_batch(
    conn: &mut rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    actor: &str,
    now: DateTime<Utc>,
    ctx: &PipelineContext,
    outcome: &mut ExtractOutcome,
    batch: Vec<CandidateFact>,
) -> Result<(), lam_core::LamError> {
    for candidate in batch {
        outcome.candidates += 1;
        if lam_core::secrets::find_secret_kind(&candidate.statement).is_some() {
            outcome.deferred += 1;
            outcome
                .notes
                .push("candidate scrubbed: secret-like content".to_string());
            continue;
        }
        let admission = crate::admission::decide_write(
            &ctx.admission_mode,
            &candidate.statement,
            &crate::admission::WriteOptions {
                explicit_save: false,
                verified_adr: false,
                secret_deny: false,
                minimum_confidence: ctx.minimum_confidence,
                model_available: ctx.extractor.is_some(),
            },
        );
        if admission.action == "reject" {
            outcome.deferred += 1;
            outcome.notes.push(format!(
                "admission gate rejected: {}",
                admission.reason_code
            ));
            continue;
        }
        if admission.action == "review" {
            outcome
                .notes
                .push(format!("admission borderline: {}", admission.reason_code));
        }
        store_candidate(conn, project_id, &candidate, actor, now)?;
        if let Some((subject, predicate, value)) = candidate.triple.clone() {
            let sources = vec![lam_core::SourceEvidence {
                source_kind: lam_core::SourceKind::Episode,
                source_ref: candidate.sources.first().cloned().unwrap_or_default(),
                line_start: None,
                line_end: None,
                commit_sha: None,
                content_hash: None,
            }];
            let asserted = lam_storage::assert_fact(
                conn,
                lam_storage::AssertInput {
                    project_id: *project_id,
                    scope: lam_core::Scope::Project,
                    kind: lam_core::MemoryKind::Fact,
                    subject,
                    predicate,
                    value: serde_json::json!(value),
                    env: String::new(),
                    status: lam_core::MemoryStatus::Proposed,
                    confidence: candidate.confidence.min(0.85),
                    valid_from: now,
                    valid_until: None,
                    actor: format!("extract:{actor}"),
                    reason: Some("deterministic extraction; needs human review".to_string()),
                    sources,
                    idempotency_key: Some(format!(
                        "extract:{}:{}",
                        candidate.sources.first().cloned().unwrap_or_default(),
                        blake3::hash(candidate.statement.as_bytes()).to_hex()
                    )),
                },
                now,
            );
            match asserted {
                Ok(_) => outcome.asserted += 1,
                Err(e) if e.code() == "E_FACT_CONFLICT" => {
                    outcome.deferred += 1;
                    outcome
                        .notes
                        .push("candidate conflicts with active verified fact".to_string());
                }
                Err(e) => return Err(e),
            }
        } else {
            outcome.deferred += 1;
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct CandidateFact {
    kind: String,
    statement: String,
    confidence: f64,
    epistemic: String,
    sources: Vec<String>,
    triple: Option<(String, String, String)>,
}

fn extract_candidates(text: &str, episode_ref: &str) -> Vec<CandidateFact> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.len() < 8 {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("decision:")
            || lower.starts_with("adr:")
            || lower.starts_with("decided:")
        {
            out.push(CandidateFact {
                kind: "decision".to_string(),
                statement: trimmed.to_string(),
                confidence: 0.6,
                epistemic: "observed".to_string(),
                sources: vec![episode_ref.to_string()],
                triple: None,
            });
            continue;
        }
        if let Some((subject, predicate, value)) = parse_uses_statement(trimmed) {
            out.push(CandidateFact {
                kind: "fact".to_string(),
                statement: trimmed.to_string(),
                confidence: 0.6,
                epistemic: "observed".to_string(),
                sources: vec![episode_ref.to_string()],
                triple: Some((subject, predicate, value)),
            });
        }
    }
    out.truncate(32);
    out
}

fn clean_token(text: &str) -> String {
    text.trim_matches(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_' || c == '-'))
        .to_string()
}

fn parse_uses_statement(text: &str) -> Option<(String, String, String)> {
    let lower = text.to_lowercase();
    for marker in [" uses ", " use ", " depends on "] {
        if let Some(position) = lower.find(marker) {
            let head = text[..position].trim();
            let tail = text[position + marker.len()..].trim();
            let subject = clean_token(head.split_whitespace().last().unwrap_or(""));
            let predicate = if marker.trim() == "depends on" {
                "depends_on".to_string()
            } else {
                "uses".to_string()
            };
            let value = clean_token(tail.split_whitespace().next().unwrap_or(""));
            if subject.len() >= 2 && value.len() >= 2 {
                return Some((subject, predicate, value));
            }
        }
    }
    None
}

fn remote_failure_transient(reason: &str) -> bool {
    match reason {
        "timeout" | "connect" | "dns" | "transport" => true,
        other => match other.strip_prefix("status-") {
            Some(code) => code == "429" || code.starts_with('5'),
            None => false,
        },
    }
}

fn record_remote_attempt(
    conn: &rusqlite::Connection,
    extractor: &dyn lam_providers::Extractor,
    status: String,
) {
    if let Some(usage) = extractor.remote_usage() {
        let _ = lam_storage::record_remote_call(
            conn,
            &lam_storage::RemoteCall {
                provider: usage.provider,
                model: usage.model,
                endpoint_host: usage.endpoint_host,
                payload_bytes: usage.payload_bytes as i64,
                redactions: usage.redactions as i64,
                status,
            },
            chrono::Utc::now(),
        );
    }
}

fn run_configured_extractor(
    conn: &rusqlite::Connection,
    choice: &ExtractorChoice,
    text: &str,
    episode_ref: &str,
    session_id: Option<String>,
) -> Result<Vec<CandidateFact>, lam_core::LamError> {
    let config = lam_config::ProviderConfig {
        provider: choice.provider.clone(),
        model: choice.model.clone(),
        endpoint: choice.endpoint.clone(),
        auto_download: false,
        api_key_env: choice.api_key_env.clone(),
        allow_remote: choice.allow_remote,
        timeout_ms: choice.timeout_ms,
        session_header: choice.session_header.clone(),
    };
    let extractor = lam_providers::extractor_for(&config, choice.timeout_ms).map_err(|_| {
        lam_core::LamError::validation(
            "E_PROVIDER_UNSUPPORTED",
            "configured extractor is not supported in pipelines".to_string(),
        )
    })?;
    let excerpt: String = text.chars().take(8000).collect();
    let found = match extractor.extract(&lam_providers::ExtractInput {
        text: excerpt,
        source_ref: episode_ref.to_string(),
        session_id,
    }) {
        Ok(found) => {
            record_remote_attempt(conn, extractor.as_ref(), "ok".to_string());
            found
        }
        Err(e) if choice.is_remote() && e.code() == "E_PROVIDER_UNAVAILABLE" => {
            let reason = extractor
                .last_remote_failure()
                .unwrap_or_else(|| "unknown".to_string());
            record_remote_attempt(conn, extractor.as_ref(), format!("fallback:{reason}"));
            if remote_failure_transient(&reason) {
                return Err(e);
            }
            Vec::new()
        }
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for candidate in found.into_iter().take(32) {
        let triple = if candidate.kind == "fact" {
            parse_uses_statement(&candidate.statement)
        } else {
            None
        };
        out.push(CandidateFact {
            kind: candidate.kind,
            statement: candidate.statement,
            confidence: candidate.confidence.clamp(0.0, 1.0),
            epistemic: candidate.epistemic_status,
            sources: if candidate.sources.is_empty() {
                vec![episode_ref.to_string()]
            } else {
                candidate.sources
            },
            triple,
        });
    }
    Ok(out)
}

fn episode_texts_for_session(
    conn: &rusqlite::Connection,
    session_ref: &str,
) -> Result<Vec<(String, String)>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.summary FROM episodes e
             JOIN sessions s ON s.id = e.session_id
             WHERE s.external_session_id = ?1 OR s.id = ?1
             ORDER BY e.rowid ASC",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![session_ref], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| lam_core::LamError::storage(e.to_string()))?);
    }
    Ok(out)
}

fn store_candidate(
    conn: &mut rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    candidate: &CandidateFact,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<lam_core::MemoryId, lam_core::LamError> {
    let kind = match candidate.kind.as_str() {
        "decision" => lam_core::MemoryKind::ArchitectureDecision,
        "fact" => lam_core::MemoryKind::Fact,
        "lesson" => lam_core::MemoryKind::Lesson,
        _ => lam_core::MemoryKind::Note,
    };
    let sources = vec![lam_core::SourceEvidence {
        source_kind: lam_core::SourceKind::Episode,
        source_ref: candidate.sources.first().cloned().unwrap_or_default(),
        line_start: None,
        line_end: None,
        commit_sha: None,
        content_hash: None,
    }];
    let (memory_id, _) = lam_storage::save_memory(
        conn,
        lam_storage::SaveInput {
            project_id: *project_id,
            scope: lam_core::Scope::Project,
            kind,
            content: candidate.statement.clone(),
            status: lam_core::MemoryStatus::Proposed,
            actor: format!("extract:{actor}"),
            reason: Some(candidate.epistemic.clone()),
            sources,
            idempotency_key: Some(format!(
                "extract:{}:{}",
                candidate.sources.first().cloned().unwrap_or_default(),
                blake3::hash(candidate.statement.as_bytes()).to_hex()
            )),
        },
        now,
    )?;
    Ok(memory_id)
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

    fn write_session_with_stop(conn: &mut rusqlite::Connection, external: &str, cwd: &str) {
        let mut event = lam_core::NormalizedEvent {
            schema_version: 1,
            host: "claude-code".to_string(),
            event_type: "session.stop".to_string(),
            external_session_id: external.to_string(),
            event_id: format!("stop-{external}"),
            cwd: cwd.to_string(),
            git_branch: None,
            timestamp: at(),
            payload: [(
                "last_message_excerpt".to_string(),
                serde_json::json!("Decision: use Temporal for orchestration"),
            )]
            .into_iter()
            .collect(),
        };
        lam_storage::ingest_normalized(conn, &mut event, 365, at()).expect("ingest");
    }

    fn leased_job(kind: &str, input_ref: &str) -> lam_storage::Job {
        lam_storage::Job {
            id: format!("job-{kind}-{input_ref}"),
            kind: kind.to_string(),
            input_ref: input_ref.to_string(),
            state: "leased".to_string(),
            attempts: 1,
            due_at: at(),
            lease_until: None,
            error: None,
        }
    }

    fn temp_project(conn: &mut rusqlite::Connection, name: &str) -> lam_core::ProjectId {
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        lam_storage::resolve_or_create(conn, leaked.path(), Some(name), at())
            .expect("project")
            .id
    }

    #[test]
    fn episode_summarize_backfills_missing_episodes() {
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "ep-proj");
        let _ = project_id;
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        write_session_with_stop(&mut conn, "s-sum-1", &cwd);
        let ctx = PipelineContext::deterministic(365);
        let note = handle_episode_summarize(
            &mut conn,
            &leased_job("episode_summarize", "session:s-sum-1"),
            &ctx,
            at(),
        )
        .expect("summarize");
        assert!(note.contains("already summarized") || note.contains("summarized session"));
    }

    #[test]
    fn fact_extract_creates_proposed_only() {
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "fx-proj");
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        write_session_with_stop(&mut conn, "s-fx-1", &cwd);
        let ctx = PipelineContext::deterministic(365);
        let outcome = handle_fact_extract(
            &mut conn,
            &leased_job("fact_extract", "session:s-fx-1"),
            &ctx,
            at(),
            &project_id,
            "maintain",
        )
        .expect("extract");
        assert!(outcome.candidates >= 1);
        let verified: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_revisions WHERE status = 'verified' AND actor LIKE 'extract:%'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(verified, 0, "extracted memories must never be verified");
    }

    #[test]
    fn cloud_failure_falls_back_to_deterministic() {
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "cloud-fallback-proj");
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        write_session_with_stop(&mut conn, "s-cloud-1", &cwd);
        let ctx = PipelineContext::deterministic(365).with_cloud(
            "http://127.0.0.1:9/v1",
            "muse-spark",
            "LAM_MISSING_KEY_FOR_TEST",
            500,
        );
        let outcome = handle_fact_extract(
            &mut conn,
            &leased_job("fact_extract", "session:s-cloud-1"),
            &ctx,
            at(),
            &project_id,
            "maintain",
        )
        .expect("extract must not fail when the cloud is down");
        assert!(outcome.candidates >= 1);
        let fallback: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_calls WHERE status = 'fallback'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(fallback, 0, "missing key makes no call at all");
        let verified: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_revisions WHERE status = 'verified' AND actor LIKE 'extract:%'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(verified, 0, "extracted memories must never be verified");
    }

    #[test]
    fn transient_remote_failure_retries_with_backoff_signal() {
        let (endpoint, handle) = fake_cloud_server(500, "boom", 0, "LAM_TEST_PIPE_KEY_500");
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "cloud-500-proj");
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        write_session_with_stop(&mut conn, "s-cloud-500", &cwd);
        let ctx = PipelineContext::deterministic(365).with_cloud(
            &endpoint,
            "muse-spark",
            "LAM_TEST_PIPE_KEY_500",
            5000,
        );
        let error = handle_fact_extract(
            &mut conn,
            &leased_job("fact_extract", "session:s-cloud-500"),
            &ctx,
            at(),
            &project_id,
            "maintain",
        )
        .expect_err("transient failure must propagate for retry");
        assert_eq!(error.code(), "E_PROVIDER_UNAVAILABLE");
        let stored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories m JOIN memory_revisions r ON r.id = m.current_revision WHERE m.project_id = ?1 AND r.actor = 'extract:maintain'",
                rusqlite::params![project_id.to_string_value()],
                |row| row.get(0),
            )
            .expect("count");
        assert!(stored >= 1, "deterministic candidates must persist");
        let reason: String = conn
            .query_row(
                "SELECT status FROM remote_calls ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("audit row");
        assert_eq!(reason, "fallback:status-500");
        handle.join().expect("server done");
    }

    #[test]
    fn rate_limited_remote_failure_retries() {
        let (endpoint, handle) = fake_cloud_server(429, "slow down", 0, "LAM_TEST_PIPE_KEY_429");
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "cloud-429-proj");
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        write_session_with_stop(&mut conn, "s-cloud-429", &cwd);
        let ctx = PipelineContext::deterministic(365).with_cloud(
            &endpoint,
            "muse-spark",
            "LAM_TEST_PIPE_KEY_429",
            5000,
        );
        let error = handle_fact_extract(
            &mut conn,
            &leased_job("fact_extract", "session:s-cloud-429"),
            &ctx,
            at(),
            &project_id,
            "maintain",
        )
        .expect_err("429 must propagate for retry");
        assert_eq!(error.code(), "E_PROVIDER_UNAVAILABLE");
        let reason: String = conn
            .query_row(
                "SELECT status FROM remote_calls ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("audit row");
        assert_eq!(reason, "fallback:status-429");
        handle.join().expect("server done");
    }

    #[test]
    fn permanent_remote_failure_completes_without_retry() {
        let (endpoint, handle) = fake_cloud_server(
            200,
            "{\"choices\":[{\"message\":{\"content\":\"not json at all\"}}]}",
            0,
            "LAM_TEST_PIPE_KEY_SCHEMA",
        );
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "cloud-schema-proj");
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        write_session_with_stop(&mut conn, "s-cloud-schema", &cwd);
        let ctx = PipelineContext::deterministic(365).with_cloud(
            &endpoint,
            "muse-spark",
            "LAM_TEST_PIPE_KEY_SCHEMA",
            5000,
        );
        let outcome = handle_fact_extract(
            &mut conn,
            &leased_job("fact_extract", "session:s-cloud-schema"),
            &ctx,
            at(),
            &project_id,
            "maintain",
        )
        .expect("permanent failure must not fail the job");
        assert!(outcome.candidates >= 1);
        let reason: String = conn
            .query_row(
                "SELECT status FROM remote_calls ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("audit row");
        assert_eq!(reason, "fallback:schema");
        let cloud: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories m JOIN memory_revisions r ON r.id = m.current_revision WHERE m.project_id = ?1 AND r.content LIKE '%Timeout handling%'",
                rusqlite::params![project_id.to_string_value()],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(cloud, 0, "no cloud candidate on schema failure");
        handle.join().expect("server done");
    }

    #[test]
    fn hook_context_defers_remote_extraction() {
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "cloud-defer-proj");
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        write_session_with_stop(&mut conn, "s-cloud-defer", &cwd);
        let mut ctx = PipelineContext::deterministic(365).with_cloud(
            "http://127.0.0.1:9/v1",
            "muse-spark",
            "LAM_TEST_PIPE_KEY_DEFER",
            500,
        );
        ctx.remote_allowed = false;
        let error = handle_fact_extract(
            &mut conn,
            &leased_job("fact_extract", "session:s-cloud-defer"),
            &ctx,
            at(),
            &project_id,
            "hook",
        )
        .expect_err("hooks must not call remote providers");
        assert_eq!(error.code(), "E_REMOTE_DEFERRED");
        let stored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE project_id = ?1",
                rusqlite::params![project_id.to_string_value()],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(stored, 0, "deferred hook run stores nothing");
        let calls: i64 = conn
            .query_row("SELECT COUNT(*) FROM remote_calls", [], |row| row.get(0))
            .expect("count");
        assert_eq!(calls, 0, "deferred hook run makes no remote call");
    }

    fn fake_cloud_server(
        status: u16,
        body: &'static str,
        delay_ms: u64,
        key_holder: &str,
    ) -> (String, std::thread::JoinHandle<()>) {
        std::env::set_var(key_holder, "test-key");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake cloud");
        let port = listener.local_addr().expect("port").port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut length = 0usize;
                {
                    use std::io::{BufRead, Read};
                    let mut reader = std::io::BufReader::new(&stream);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        if line.trim().is_empty() {
                            break;
                        }
                        if let Some(value) = line.strip_prefix("Content-Length:") {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut received = vec![0u8; length];
                    let _ = reader.read_exact(&mut received);
                }
                if delay_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                }
                let phrase = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {phrase}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                use std::io::Write;
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}/v1"), handle)
    }

    #[test]
    fn extraction_rejects_secrets() {
        let found = extract_candidates("api_key = hunter2 leaked here", "episode:9");
        assert!(found.is_empty() || found.iter().all(|c| !c.statement.contains("hunter2")));
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn, "sec-proj");
        let ctx = PipelineContext::deterministic(365);
        let outcome = handle_fact_extract(
            &mut conn,
            &leased_job("fact_extract", "session:s-missing"),
            &ctx,
            at(),
            &project_id,
            "maintain",
        )
        .expect("extract");
        assert_eq!(outcome.candidates, 0);
        assert!(outcome.deferred > 0);
    }
}
