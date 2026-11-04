use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct AssertInput {
    pub project_id: lam_core::ProjectId,
    pub scope: lam_core::Scope,
    pub kind: lam_core::MemoryKind,
    pub subject: String,
    pub predicate: String,
    pub value: serde_json::Value,
    pub env: String,
    pub status: lam_core::MemoryStatus,
    pub confidence: f64,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub actor: String,
    pub reason: Option<String>,
    pub sources: Vec<lam_core::SourceEvidence>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AssertOutcome {
    pub memory_id: lam_core::MemoryId,
    pub revision_id: lam_core::RevisionId,
    pub assertion_id: String,
    pub superseded: Vec<String>,
    pub conflict_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AssertionView {
    pub id: String,
    pub memory_id: String,
    pub revision_id: String,
    pub revision_status: String,
    pub subject: String,
    pub predicate: String,
    pub value: serde_json::Value,
    pub env: String,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub recorded_at: DateTime<Utc>,
    pub retracted_at: Option<DateTime<Utc>>,
    pub confidence: f64,
    pub under_review: bool,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConflictView {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub env: String,
    pub active_assertion_id: String,
    pub active_value: serde_json::Value,
    pub incoming_assertion_id: String,
    pub incoming_value: serde_json::Value,
    pub reason: String,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepSide {
    Incoming,
    Active,
}

impl std::str::FromStr for KeepSide {
    type Err = lam_core::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "incoming" => Ok(KeepSide::Incoming),
            "active" => Ok(KeepSide::Active),
            _ => Err(lam_core::LamError::validation(
                "E_KEEP_SIDE_INVALID",
                format!("keep must be incoming or active, got: {s}"),
            )),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    Single,
    Multi,
}

impl Cardinality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Cardinality::Single => "single",
            Cardinality::Multi => "multi",
        }
    }
}

pub fn authority_rank(
    source: &lam_core::SourceKind,
    actor: &str,
    status: lam_core::MemoryStatus,
) -> u8 {
    if status == lam_core::MemoryStatus::Proposed {
        if actor.trim_start().to_lowercase().starts_with("dream") {
            return 6;
        }
        return 5;
    }
    if is_explicit_actor(actor) {
        return 1;
    }
    match source {
        lam_core::SourceKind::User => 1,
        lam_core::SourceKind::Document => 1,
        lam_core::SourceKind::Config => 2,
        lam_core::SourceKind::Test => 3,
        lam_core::SourceKind::Session | lam_core::SourceKind::Episode => 4,
    }
}

pub fn is_explicit_actor(actor: &str) -> bool {
    matches!(actor.trim(), "user" | "cli" | "mcp")
}

pub fn cardinality_of(
    conn: &rusqlite::Connection,
    predicate: &str,
) -> Result<Cardinality, lam_core::LamError> {
    let found: Option<String> = conn
        .query_row(
            "SELECT cardinality FROM predicate_policy WHERE predicate = ?1",
            rusqlite::params![predicate.trim()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match found.as_deref() {
        Some("multi") => Ok(Cardinality::Multi),
        _ => Ok(Cardinality::Single),
    }
}

pub fn set_cardinality(
    conn: &rusqlite::Connection,
    predicate: &str,
    cardinality: Cardinality,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    let name = predicate.trim();
    if name.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_PREDICATE_INVALID",
            "predicate must not be empty".to_string(),
        ));
    }
    conn.execute(
        "INSERT INTO predicate_policy (predicate, cardinality, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(predicate) DO UPDATE SET cardinality = excluded.cardinality, updated_at = excluded.updated_at",
        rusqlite::params![name, cardinality.as_str(), now.to_rfc3339()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(())
}

fn validated_sources(
    actor: &str,
    sources: &[lam_core::SourceEvidence],
) -> Result<Vec<lam_core::SourceEvidence>, lam_core::LamError> {
    if sources.is_empty() {
        return Ok(vec![lam_core::SourceEvidence {
            source_kind: lam_core::SourceKind::User,
            source_ref: actor.to_string(),
            line_start: None,
            line_end: None,
            commit_sha: None,
            content_hash: None,
        }]);
    }
    for source in sources {
        source.validate()?;
    }
    Ok(sources.to_vec())
}

const ASSERTION_COLUMNS: &str = "a.id, m.id, r.id, r.status, a.subject, a.predicate, a.value_json, COALESCE(a.env, ''), a.valid_from, a.valid_until, a.recorded_at, a.retracted_at, a.confidence, (SELECT s.source_ref FROM memory_sources s WHERE s.revision_id = r.id ORDER BY rowid ASC LIMIT 1)";

#[derive(Debug, Clone)]
pub struct AssertionQuery<'a> {
    pub project_id: &'a lam_core::ProjectId,
    pub subject: &'a str,
    pub predicate: &'a str,
    pub env: &'a str,
    pub as_of: Option<DateTime<Utc>>,
    pub known_at: Option<DateTime<Utc>>,
    pub include_retracted: bool,
}

fn query_assertions(
    conn: &rusqlite::Connection,
    query: AssertionQuery,
) -> Result<Vec<AssertionView>, lam_core::LamError> {
    let mut sql = format!(
        "SELECT {ASSERTION_COLUMNS} FROM fact_assertions a
         JOIN memory_revisions r ON r.id = a.memory_revision_id
         JOIN memories m ON m.id = r.memory_id
         WHERE m.project_id = ?1 AND a.subject = ?2 AND a.predicate = ?3 AND COALESCE(a.env, '') = ?4
         AND (?5 IS NULL OR (a.valid_from <= ?5 AND (a.valid_until IS NULL OR ?5 < a.valid_until)))
         AND (?6 IS NULL OR (a.recorded_at <= ?6 AND (a.retracted_at IS NULL OR ?6 < a.retracted_at)))
         AND (?7 = 0 OR a.retracted_at IS NULL)"
    );
    sql.push_str(" ORDER BY a.recorded_at ASC");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let project_text = query.project_id.to_string_value();
    let as_of_text = query.as_of.map(|t| t.to_rfc3339());
    let known_text = query.known_at.map(|t| t.to_rfc3339());
    let hide_retracted: i64 = if query.include_retracted { 0 } else { 1 };
    let rows = stmt
        .query_map(
            rusqlite::params![
                project_text,
                query.subject,
                query.predicate,
                query.env,
                as_of_text,
                known_text,
                hide_retracted,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, f64>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            },
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let tuple = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let bad = || lam_core::LamError::storage("fact row has invalid typed value".to_string());
        let parse_time = |text: &str| text.parse::<DateTime<Utc>>().map_err(|_| bad());
        let value: serde_json::Value = serde_json::from_str(&tuple.6).map_err(|_| bad())?;
        out.push(AssertionView {
            id: tuple.0,
            memory_id: tuple.1,
            revision_id: tuple.2,
            revision_status: tuple.3,
            subject: tuple.4,
            predicate: tuple.5,
            value,
            env: tuple.7,
            valid_from: parse_time(&tuple.8)?,
            valid_until: tuple.9.map(|text| parse_time(&text)).transpose()?,
            recorded_at: parse_time(&tuple.10)?,
            retracted_at: tuple.11.map(|text| parse_time(&text)).transpose()?,
            confidence: tuple.12,
            under_review: false,
            source_ref: tuple.13,
        });
    }
    Ok(out)
}

pub fn active_facts(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    subject: &str,
    predicate: &str,
    env: &str,
    as_of: DateTime<Utc>,
    known_at: DateTime<Utc>,
) -> Result<Vec<AssertionView>, lam_core::LamError> {
    query_assertions(
        conn,
        AssertionQuery {
            project_id,
            subject,
            predicate,
            env,
            as_of: Some(as_of),
            known_at: Some(known_at),
            include_retracted: false,
        },
    )
}

pub fn fact_timeline(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    subject: &str,
    predicate: &str,
    env: &str,
) -> Result<Vec<AssertionView>, lam_core::LamError> {
    let mut rows = query_assertions(
        conn,
        AssertionQuery {
            project_id,
            subject,
            predicate,
            env,
            as_of: None,
            known_at: None,
            include_retracted: true,
        },
    )?;
    for row in rows.iter_mut() {
        row.under_review =
            row.retracted_at.is_some() && review_open_for(conn, &row.id).unwrap_or(false);
    }
    Ok(rows)
}

fn review_open_for(
    conn: &rusqlite::Connection,
    assertion_id: &str,
) -> Result<bool, lam_core::LamError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM conflict_reviews
             WHERE status = 'open' AND (active_assertion_id = ?1 OR incoming_assertion_id = ?1)",
            rusqlite::params![assertion_id],
            |row| row.get(0),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(count > 0)
}

fn same_value(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    left == right
}

pub fn assert_fact(
    conn: &mut rusqlite::Connection,
    input: AssertInput,
    now: DateTime<Utc>,
) -> Result<AssertOutcome, lam_core::LamError> {
    if input.subject.trim().is_empty() || input.predicate.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_FACT_RECORD_INVALID",
            "subject and predicate must not be empty".to_string(),
        ));
    }
    if !(0.0..=1.0).contains(&input.confidence) {
        return Err(lam_core::LamError::validation(
            "E_FACT_CONFIDENCE_INVALID",
            "confidence must be within 0.0 and 1.0".to_string(),
        ));
    }
    if input.status != lam_core::MemoryStatus::Verified
        && input.status != lam_core::MemoryStatus::Proposed
    {
        return Err(lam_core::LamError::validation(
            "E_FACT_STATUS_INVALID",
            "fact status must be verified or proposed".to_string(),
        ));
    }
    if let Some(until) = input.valid_until {
        if input.valid_from > until {
            return Err(lam_core::LamError::validation(
                "E_FACT_VALIDITY_INVALID",
                "valid_from must not exceed valid_until".to_string(),
            ));
        }
    }
    let value_text = serde_json::to_string(&input.value).map_err(lam_core::LamError::from)?;
    if value_text.len() > 8192 {
        return Err(lam_core::LamError::validation(
            "E_FACT_VALUE_INVALID",
            "fact value exceeds 8192 bytes".to_string(),
        ));
    }
    if let serde_json::Value::String(text) = &input.value {
        lam_core::ensure_no_secrets(text)?;
    }
    if input.actor.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_ACTOR_INVALID",
            "actor must not be empty".to_string(),
        ));
    }
    let sources = validated_sources(&input.actor, &input.sources)?;
    if let Some(key) = input.idempotency_key.clone() {
        if let Some(existing) = replay_lookup(conn, &key)? {
            return Ok(existing);
        }
    }
    let cardinality = cardinality_of(conn, &input.predicate)?;
    let actives = active_facts(
        conn,
        &input.project_id,
        &input.subject,
        &input.predicate,
        &input.env,
        input.valid_from,
        now,
    )?;
    if let Some(duplicate) = actives
        .iter()
        .find(|row| same_value(&row.value, &input.value))
    {
        return Ok(AssertOutcome {
            memory_id: duplicate
                .memory_id
                .parse()
                .map_err(|_| lam_core::LamError::storage("stored memory id invalid".to_string()))?,
            revision_id: duplicate.revision_id.parse().map_err(|_| {
                lam_core::LamError::storage("stored revision id invalid".to_string())
            })?,
            assertion_id: duplicate.id.clone(),
            superseded: Vec::new(),
            conflict_id: None,
        });
    }
    let conflicting: Vec<&AssertionView> = actives
        .iter()
        .filter(|row| row.revision_status == "verified")
        .collect();
    if cardinality == Cardinality::Single && !conflicting.is_empty() {
        return reconcile(conn, &input, &sources, &value_text, &conflicting, now);
    }
    let mut outcome = insert_assertion(conn, &input, &sources, &value_text, None, now)?;
    outcome.superseded = Vec::new();
    outcome.conflict_id = None;
    Ok(outcome)
}

fn replay_lookup(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<Option<AssertOutcome>, lam_core::LamError> {
    let found: Option<String> = conn
        .query_row(
            "SELECT new_hash FROM audit_log WHERE event = 'fact_assert' AND target = ?1 ORDER BY at ASC LIMIT 1",
            rusqlite::params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let assertion_id = match found {
        Some(id) => id,
        None => return Ok(None),
    };
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT m.id, m.current_revision FROM fact_assertions a
             JOIN memory_revisions r ON r.id = a.memory_revision_id
             JOIN memories m ON m.id = r.memory_id
             WHERE a.id = ?1",
            rusqlite::params![assertion_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match row {
        Some((memory_text, revision_text)) => {
            let memory_id = memory_text
                .parse()
                .map_err(|_| lam_core::LamError::storage("stored memory id invalid".to_string()))?;
            let revision_id = revision_text.parse().map_err(|_| {
                lam_core::LamError::storage("stored revision id invalid".to_string())
            })?;
            Ok(Some(AssertOutcome {
                memory_id,
                revision_id,
                assertion_id,
                superseded: Vec::new(),
                conflict_id: None,
            }))
        }
        None => Ok(None),
    }
}

fn insert_assertion(
    conn: &mut rusqlite::Connection,
    input: &AssertInput,
    sources: &[lam_core::SourceEvidence],
    value_text: &str,
    retracted_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<AssertOutcome, lam_core::LamError> {
    let tx = conn
        .transaction()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let memory_id = lam_core::MemoryId::new();
    let revision_id = lam_core::RevisionId::new();
    let assertion_id = lam_core::RevisionId::new().to_string();
    tx.execute(
        "INSERT INTO memories (id, project_id, scope, kind, current_revision, created_at)
         VALUES (?1, ?2, ?3, 'fact', ?4, ?5)",
        rusqlite::params![
            memory_id.to_string(),
            input.project_id.to_string_value(),
            input.scope.as_str(),
            revision_id.to_string(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO memory_revisions (id, memory_id, content, status, version, recorded_at, actor, reason)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)",
        rusqlite::params![
            revision_id.to_string(),
            memory_id.to_string(),
            value_text,
            input.status.as_str(),
            now.to_rfc3339(),
            input.actor,
            input.reason,
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for source in sources {
        tx.execute(
            "INSERT INTO memory_sources (id, revision_id, source_kind, source_ref, line_start, line_end, commit_sha, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                lam_core::RevisionId::new().to_string(),
                revision_id.to_string(),
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
        "INSERT INTO memory_fts (memory_id, content) VALUES (?1, ?2)",
        rusqlite::params![
            memory_id.to_string(),
            format!("{} {} {}", input.subject, input.predicate, value_text),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO fact_assertions (id, memory_revision_id, subject, predicate, value_json, valid_from, valid_until, recorded_at, retracted_at, confidence, env)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            assertion_id,
            revision_id.to_string(),
            input.subject.trim(),
            input.predicate.trim(),
            value_text,
            input.valid_from.to_rfc3339(),
            input.valid_until.map(|t| t.to_rfc3339()),
            now.to_rfc3339(),
            retracted_at.map(|t| t.to_rfc3339()),
            input.confidence,
            input.env,
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO audit_log (id, event, actor, target, old_hash, new_hash, at)
         VALUES (?1, 'fact_assert', ?2, ?3, NULL, ?4, ?5)",
        rusqlite::params![
            lam_core::RevisionId::new().to_string(),
            input.actor,
            input
                .idempotency_key
                .clone()
                .unwrap_or_else(|| assertion_id.clone()),
            assertion_id,
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.commit()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(AssertOutcome {
        memory_id,
        revision_id,
        assertion_id,
        superseded: Vec::new(),
        conflict_id: None,
    })
}

fn source_rank_of(
    conn: &rusqlite::Connection,
    assertion_id: &str,
) -> Result<(u8, DateTime<Utc>), lam_core::LamError> {
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT r.status, r.actor, COALESCE((SELECT s.source_kind FROM memory_sources s WHERE s.revision_id = r.id ORDER BY rowid ASC LIMIT 1), 'user'), a.valid_from
             FROM fact_assertions a
             JOIN memory_revisions r ON r.id = a.memory_revision_id
             WHERE a.id = ?1",
            rusqlite::params![assertion_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let (status_text, actor, kind_text, valid_text) = row.ok_or_else(|| {
        lam_core::LamError::validation(
            "E_FACT_NOT_FOUND",
            format!("no fact assertion {assertion_id}"),
        )
    })?;
    let status = status_text
        .parse::<lam_core::MemoryStatus>()
        .map_err(|_| lam_core::LamError::storage("stored revision status invalid".to_string()))?;
    let kind = match kind_text.as_str() {
        "document" => lam_core::SourceKind::Document,
        "session" => lam_core::SourceKind::Session,
        "episode" => lam_core::SourceKind::Episode,
        "user" => lam_core::SourceKind::User,
        "config" => lam_core::SourceKind::Config,
        _ => lam_core::SourceKind::Test,
    };
    let valid_from = valid_text
        .parse::<DateTime<Utc>>()
        .map_err(|_| lam_core::LamError::storage("stored validity invalid".to_string()))?;
    Ok((authority_rank(&kind, &actor, status), valid_from))
}

fn brief_value(value: &serde_json::Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 120 {
        text.chars().take(120).collect()
    } else {
        text
    }
}

fn reconcile(
    conn: &mut rusqlite::Connection,
    input: &AssertInput,
    sources: &[lam_core::SourceEvidence],
    value_text: &str,
    conflicting: &[&AssertionView],
    now: DateTime<Utc>,
) -> Result<AssertOutcome, lam_core::LamError> {
    let primary = sources
        .first()
        .map(|s| s.source_kind)
        .unwrap_or(lam_core::SourceKind::User);
    let new_rank = authority_rank(&primary, &input.actor, input.status);
    let mut old_ranks: Vec<(String, u8, DateTime<Utc>)> = Vec::new();
    for active in conflicting {
        let (rank, valid_from) = source_rank_of(conn, &active.id)?;
        old_ranks.push((active.id.clone(), rank, valid_from));
    }
    let strongest_old = old_ranks
        .iter()
        .map(|(_, rank, _)| *rank)
        .min()
        .unwrap_or(1);
    let newest_old = old_ranks
        .iter()
        .map(|(_, _, valid_from)| *valid_from)
        .max()
        .unwrap_or(input.valid_from);
    let explicit = is_explicit_actor(&input.actor)
        || (input.status == lam_core::MemoryStatus::Verified
            && primary == lam_core::SourceKind::Document);
    let newer = input.valid_from > newest_old;
    if new_rank < strongest_old || (explicit && new_rank == strongest_old && newer) {
        return supersede(conn, input, sources, value_text, &old_ranks, now);
    }
    let reason = format!(
        "authority {new_rank} does not overrule active rank {strongest_old}; incoming {}",
        brief_value(&input.value),
    );
    let mut outcome = insert_assertion(conn, input, sources, value_text, Some(now), now)?;
    let mut first_conflict: Option<String> = None;
    for (active_id, _, _) in &old_ranks {
        let review_id = lam_core::RevisionId::new().to_string();
        conn.execute(
            "INSERT INTO conflict_reviews (id, active_assertion_id, incoming_assertion_id, reason, status)
             VALUES (?1, ?2, ?3, ?4, 'open')",
            rusqlite::params![review_id, active_id, outcome.assertion_id, reason],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        conn.execute(
            "INSERT INTO audit_log (id, event, actor, target, old_hash, new_hash, at)
             VALUES (?1, 'conflict_open', ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                lam_core::RevisionId::new().to_string(),
                input.actor,
                review_id,
                active_id,
                outcome.assertion_id,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if first_conflict.is_none() {
            first_conflict = Some(review_id);
        }
    }
    crate::db::harden_current_db(conn);
    outcome.superseded = Vec::new();
    outcome.conflict_id = first_conflict;
    Ok(outcome)
}

fn supersede(
    conn: &mut rusqlite::Connection,
    input: &AssertInput,
    sources: &[lam_core::SourceEvidence],
    value_text: &str,
    old_ranks: &[(String, u8, DateTime<Utc>)],
    now: DateTime<Utc>,
) -> Result<AssertOutcome, lam_core::LamError> {
    let mut outcome = insert_assertion(conn, input, sources, value_text, None, now)?;
    let mut superseded = Vec::new();
    for (active_id, _, _) in old_ranks {
        conn.execute(
            "UPDATE fact_assertions SET valid_until = ?1 WHERE id = ?2",
            rusqlite::params![input.valid_from.to_rfc3339(), active_id],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        conn.execute(
            "INSERT INTO audit_log (id, event, actor, target, old_hash, new_hash, at)
             VALUES (?1, 'fact_supersede', ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                lam_core::RevisionId::new().to_string(),
                input.actor,
                active_id,
                active_id,
                outcome.assertion_id,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        superseded.push(active_id.clone());
    }
    crate::db::harden_current_db(conn);
    outcome.superseded = superseded;
    outcome.conflict_id = None;
    Ok(outcome)
}

pub fn list_conflicts(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    open_only: bool,
) -> Result<Vec<ConflictView>, lam_core::LamError> {
    let mut sql = "SELECT c.id, c.active_assertion_id, c.incoming_assertion_id, c.reason, c.status,
        a1.subject, a1.predicate, COALESCE(a1.env, ''), a1.value_json, a2.value_json
        FROM conflict_reviews c
        JOIN fact_assertions a1 ON a1.id = c.active_assertion_id
        JOIN memory_revisions r1 ON r1.id = a1.memory_revision_id
        JOIN memories m1 ON m1.id = r1.memory_id
        JOIN fact_assertions a2 ON a2.id = c.incoming_assertion_id
        WHERE m1.project_id = ?1"
        .to_string();
    if open_only {
        sql.push_str(" AND c.status = 'open'");
    }
    sql.push_str(" ORDER BY c.rowid ASC");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![project_id.to_string_value()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let tuple = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let bad = || lam_core::LamError::storage("conflict row has invalid value".to_string());
        out.push(ConflictView {
            id: tuple.0,
            active_assertion_id: tuple.1,
            incoming_assertion_id: tuple.2,
            reason: tuple.3,
            status: tuple.4,
            subject: tuple.5,
            predicate: tuple.6,
            env: tuple.7,
            active_value: serde_json::from_str(&tuple.8).map_err(|_| bad())?,
            incoming_value: serde_json::from_str(&tuple.9).map_err(|_| bad())?,
        });
    }
    Ok(out)
}

pub fn resolve_conflict(
    conn: &mut rusqlite::Connection,
    conflict_id: &str,
    keep: KeepSide,
    actor: &str,
    reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    if actor.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_ACTOR_INVALID",
            "actor must not be empty".to_string(),
        ));
    }
    let review: Option<(String, String, String)> = conn
        .query_row(
            "SELECT active_assertion_id, incoming_assertion_id, status FROM conflict_reviews WHERE id = ?1",
            rusqlite::params![conflict_id.trim()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let (active_id, incoming_id, status) = review.ok_or_else(|| {
        lam_core::LamError::validation(
            "E_CONFLICT_NOT_FOUND",
            format!("no conflict {}", conflict_id.trim()),
        )
    })?;
    if status != "open" {
        return Err(lam_core::LamError::validation(
            "E_CONFLICT_STATE",
            format!("conflict {conflict_id} is already {status}"),
        ));
    }
    let incoming_valid: Option<String> = conn
        .query_row(
            "SELECT valid_from FROM fact_assertions WHERE id = ?1",
            rusqlite::params![incoming_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let incoming_valid = incoming_valid.ok_or_else(|| {
        lam_core::LamError::validation("E_FACT_NOT_FOUND", "incoming assertion is gone".to_string())
    })?;
    let active_valid: Option<String> = conn
        .query_row(
            "SELECT valid_from FROM fact_assertions WHERE id = ?1",
            rusqlite::params![active_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let active_valid = active_valid.ok_or_else(|| {
        lam_core::LamError::validation("E_FACT_NOT_FOUND", "active assertion is gone".to_string())
    })?;
    let boundary = if incoming_valid > active_valid {
        incoming_valid.clone()
    } else {
        active_valid.clone()
    };
    match keep {
        KeepSide::Incoming => {
            conn.execute(
                "UPDATE fact_assertions SET retracted_at = NULL WHERE id = ?1",
                rusqlite::params![incoming_id],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            conn.execute(
                "UPDATE fact_assertions SET valid_until = ?1 WHERE id = ?2",
                rusqlite::params![boundary, active_id],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            set_assertion_memory_status(
                conn,
                &incoming_id,
                lam_core::MemoryStatus::Verified,
                actor,
                reason,
                now,
            )?;
        }
        KeepSide::Active => {
            set_assertion_memory_status(
                conn,
                &incoming_id,
                lam_core::MemoryStatus::Archived,
                actor,
                reason,
                now,
            )?;
        }
    }
    conn.execute(
        "UPDATE conflict_reviews SET status = 'resolved' WHERE id = ?1",
        rusqlite::params![conflict_id.trim()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO audit_log (id, event, actor, target, old_hash, new_hash, at)
         VALUES (?1, 'conflict_resolve', ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            lam_core::RevisionId::new().to_string(),
            actor,
            conflict_id.trim(),
            active_id,
            incoming_id,
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(())
}

fn set_assertion_memory_status(
    conn: &mut rusqlite::Connection,
    assertion_id: &str,
    status: lam_core::MemoryStatus,
    actor: &str,
    reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    let memory_id: Option<String> = conn
        .query_row(
            "SELECT m.id FROM fact_assertions a
             JOIN memory_revisions r ON r.id = a.memory_revision_id
             JOIN memories m ON m.id = r.memory_id
             WHERE a.id = ?1",
            rusqlite::params![assertion_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if let Some(text) = memory_id {
        if let Ok(id) = text.parse::<lam_core::MemoryId>() {
            crate::memories::set_status(conn, &id, status, actor, reason, now)?;
        }
    }
    Ok(())
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

    fn at(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, year, month, day, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    fn test_project(conn: &mut rusqlite::Connection) -> lam_core::ProjectId {
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        crate::projects::resolve_or_create(
            conn,
            leaked.path(),
            Some("temporal-proj"),
            at(2026, 10, 20),
        )
        .expect("project")
        .id
    }

    fn assert_input(project_id: lam_core::ProjectId, value: serde_json::Value) -> AssertInput {
        AssertInput {
            project_id,
            scope: lam_core::Scope::Project,
            kind: lam_core::MemoryKind::Fact,
            subject: "database.engine".to_string(),
            predicate: "uses".to_string(),
            value,
            env: String::new(),
            status: lam_core::MemoryStatus::Verified,
            confidence: 0.95,
            valid_from: at(2026, 10, 20),
            valid_until: None,
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
    fn as_of_returns_version_valid_at_that_time() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 10, 21);
        let mut first = assert_input(project_id, serde_json::json!("MySQL"));
        first.idempotency_key = Some("mysql-v1".to_string());
        assert_fact(&mut conn, first, now).expect("assert mysql");
        let mysql = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "uses",
            "",
            at(2026, 10, 21),
            now,
        )
        .expect("query");
        assert_eq!(mysql.len(), 1);
        assert_eq!(mysql[0].value, serde_json::json!("MySQL"));
        let before = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "uses",
            "",
            at(2026, 10, 19),
            now,
        )
        .expect("query before");
        assert!(before.is_empty());
    }

    #[test]
    fn explicit_newer_fact_supersedes_same_authority() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 10, 21);
        assert_fact(
            &mut conn,
            assert_input(project_id, serde_json::json!("MySQL")),
            now,
        )
        .expect("assert mysql");
        let mut second = assert_input(project_id, serde_json::json!("PostgreSQL"));
        second.valid_from = at(2026, 10, 22);
        let outcome = assert_fact(&mut conn, second, now).expect("supersede");
        assert_eq!(outcome.superseded.len(), 1);
        assert!(outcome.conflict_id.is_none());
        let current = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "uses",
            "",
            at(2026, 10, 23),
            now,
        )
        .expect("query");
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].value, serde_json::json!("PostgreSQL"));
        let history = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "uses",
            "",
            at(2026, 10, 21),
            now,
        )
        .expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].value, serde_json::json!("MySQL"));
    }

    #[test]
    fn weaker_extraction_goes_to_review_without_overwrite() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 10, 21);
        assert_fact(
            &mut conn,
            assert_input(project_id, serde_json::json!("Temporal")),
            now,
        )
        .expect("assert temporal");
        let mut candidate = assert_input(project_id, serde_json::json!("custom-cron"));
        candidate.valid_from = at(2026, 10, 22);
        candidate.status = lam_core::MemoryStatus::Proposed;
        candidate.actor = "extract:test".to_string();
        candidate.confidence = 0.4;
        let outcome = assert_fact(&mut conn, candidate, now).expect("review path");
        assert!(outcome.superseded.is_empty());
        let conflict_id = outcome.conflict_id.expect("conflict row");
        let queue = list_conflicts(&conn, &project_id, true).expect("queue");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].id, conflict_id);
        let active = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "uses",
            "",
            at(2026, 10, 23),
            now,
        )
        .expect("query");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].value, serde_json::json!("Temporal"));
        resolve_conflict(
            &mut conn,
            &conflict_id,
            KeepSide::Incoming,
            "cli",
            None,
            now,
        )
        .expect("resolve incoming");
        let after = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "uses",
            "",
            at(2026, 10, 23),
            now,
        )
        .expect("query after");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].value, serde_json::json!("custom-cron"));
        assert!(list_conflicts(&conn, &project_id, true)
            .expect("queue")
            .is_empty());
    }

    #[test]
    fn resolve_keep_active_archives_incoming() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 10, 21);
        assert_fact(
            &mut conn,
            assert_input(project_id, serde_json::json!("Temporal")),
            now,
        )
        .expect("assert temporal");
        let mut candidate = assert_input(project_id, serde_json::json!("custom-cron"));
        candidate.valid_from = at(2026, 10, 22);
        candidate.status = lam_core::MemoryStatus::Proposed;
        candidate.actor = "extract:test".to_string();
        let outcome = assert_fact(&mut conn, candidate, now).expect("review path");
        let conflict_id = outcome.conflict_id.expect("conflict row");
        resolve_conflict(
            &mut conn,
            &conflict_id,
            KeepSide::Active,
            "cli",
            Some("adr wins"),
            now,
        )
        .expect("resolve active");
        let active = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "uses",
            "",
            at(2026, 10, 23),
            now,
        )
        .expect("query");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].value, serde_json::json!("Temporal"));
    }

    #[test]
    fn equal_non_explicit_authority_goes_to_review() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 10, 21);
        let mut first = assert_input(project_id, serde_json::json!("one"));
        first.actor = "session:abc".to_string();
        first.sources = vec![lam_core::SourceEvidence {
            source_kind: lam_core::SourceKind::Session,
            source_ref: "session:abc".to_string(),
            line_start: None,
            line_end: None,
            commit_sha: None,
            content_hash: None,
        }];
        assert_fact(&mut conn, first, now).expect("first");
        let mut second = assert_input(project_id, serde_json::json!("two"));
        second.actor = "session:def".to_string();
        second.sources = vec![lam_core::SourceEvidence {
            source_kind: lam_core::SourceKind::Session,
            source_ref: "session:def".to_string(),
            line_start: None,
            line_end: None,
            commit_sha: None,
            content_hash: None,
        }];
        second.valid_from = at(2026, 10, 22);
        let outcome = assert_fact(&mut conn, second, now).expect("review path");
        assert!(outcome.conflict_id.is_some());
    }

    #[test]
    fn multi_valued_predicate_allows_parallel_facts() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 10, 21);
        let mut first = assert_input(project_id, serde_json::json!("kafka"));
        first.predicate = "depends_on".to_string();
        first.subject = "service.api".to_string();
        assert_fact(&mut conn, first, now).expect("first");
        let mut second = assert_input(project_id, serde_json::json!("postgres"));
        second.predicate = "depends_on".to_string();
        second.subject = "service.api".to_string();
        assert_fact(&mut conn, second, now).expect("second");
        let active = active_facts(
            &conn,
            &project_id,
            "service.api",
            "depends_on",
            "",
            at(2026, 10, 21),
            now,
        )
        .expect("query");
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn env_slots_are_independent() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 10, 21);
        let mut prod = assert_input(project_id, serde_json::json!("primary"));
        prod.predicate = "replicas".to_string();
        prod.env = "prod".to_string();
        assert_fact(&mut conn, prod, now).expect("prod");
        let mut staging = assert_input(project_id, serde_json::json!("single"));
        staging.predicate = "replicas".to_string();
        staging.env = "staging".to_string();
        assert_fact(&mut conn, staging, now).expect("staging");
        let prod_active = active_facts(
            &conn,
            &project_id,
            "database.engine",
            "replicas",
            "prod",
            at(2026, 10, 21),
            now,
        )
        .expect("query");
        assert_eq!(prod_active.len(), 1);
        assert_eq!(prod_active[0].value, serde_json::json!("primary"));
    }

    #[test]
    fn authority_ranks_follow_precedence() {
        use lam_core::MemoryStatus as Status;
        use lam_core::SourceKind as Kind;
        assert_eq!(
            authority_rank(&Kind::Document, "extract", Status::Verified),
            1
        );
        assert_eq!(
            authority_rank(&Kind::Config, "extract", Status::Verified),
            2
        );
        assert_eq!(authority_rank(&Kind::Test, "extract", Status::Verified), 3);
        assert_eq!(
            authority_rank(&Kind::Session, "extract", Status::Verified),
            4
        );
        assert_eq!(authority_rank(&Kind::Test, "extract", Status::Proposed), 5);
        assert_eq!(
            authority_rank(&Kind::Test, "dream-worker", Status::Proposed),
            6
        );
        assert_eq!(authority_rank(&Kind::Test, "cli", Status::Verified), 1);
    }
}
