use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct AssertArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub subject: String,
    #[arg(long)]
    pub predicate: String,
    #[arg(long)]
    pub value: String,
    #[arg(long, default_value_t = false)]
    pub value_json: bool,
    #[arg(long, default_value = "")]
    pub env: String,
    #[arg(long)]
    pub status: Option<String>,
    #[arg(long, default_value_t = 0.9)]
    pub confidence: f64,
    #[arg(long)]
    pub valid_from: Option<String>,
    #[arg(long)]
    pub valid_until: Option<String>,
    #[arg(long)]
    pub source_ref: Option<String>,
    #[arg(long)]
    pub source_kind: Option<String>,
    #[arg(long)]
    pub line_start: Option<u32>,
    #[arg(long)]
    pub line_end: Option<u32>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Args)]
pub struct TimelineArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub subject: String,
    #[arg(long)]
    pub predicate: String,
    #[arg(long, default_value = "")]
    pub env: String,
}

#[derive(Debug, Args)]
pub struct FactGetArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub subject: String,
    #[arg(long)]
    pub predicate: String,
    #[arg(long, default_value = "")]
    pub env: String,
    #[arg(long)]
    pub as_of: Option<String>,
}

#[derive(Debug, Args)]
pub struct ConflictsArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long, default_value_t = false)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ResolveArgs {
    pub conflict_id: String,
    #[arg(long)]
    pub keep: String,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
}

#[derive(Debug, Args)]
pub struct PolicyArgs {
    #[arg(long)]
    pub set: Option<String>,
    #[arg(long)]
    pub cardinality: Option<String>,
}

fn parse_time(
    raw: &Option<String>,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, lam_core::LamError> {
    raw.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<chrono::DateTime<chrono::Utc>>().map_err(|_| {
                lam_core::LamError::validation(
                    "E_TIME_INVALID",
                    format!("{field} must be RFC3339 datetime, got: {s}"),
                )
            })
        })
        .transpose()
}

fn parse_value(text: &str, as_json: bool) -> Result<serde_json::Value, lam_core::LamError> {
    if as_json {
        serde_json::from_str(text).map_err(|e| {
            lam_core::LamError::validation(
                "E_VALUE_INVALID",
                format!("value is not valid JSON: {e}"),
            )
        })
    } else {
        Ok(serde_json::json!(text))
    }
}

fn parse_source_kind(
    raw: &Option<String>,
) -> Result<Option<lam_core::SourceKind>, lam_core::LamError> {
    raw.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|kind| match kind {
            "document" => Ok(lam_core::SourceKind::Document),
            "session" => Ok(lam_core::SourceKind::Session),
            "episode" => Ok(lam_core::SourceKind::Episode),
            "user" => Ok(lam_core::SourceKind::User),
            "config" => Ok(lam_core::SourceKind::Config),
            "test" => Ok(lam_core::SourceKind::Test),
            _ => Err(lam_core::LamError::validation(
                "E_SOURCE_KIND_INVALID",
                format!("unknown source kind: {kind}"),
            )),
        })
        .transpose()
}

fn actor_of(raw: &Option<String>) -> String {
    let text = raw.as_deref().map(str::trim).unwrap_or("cli");
    if text.is_empty() {
        "cli".to_string()
    } else {
        text.to_string()
    }
}

pub fn run_assert(
    home: Option<PathBuf>,
    args: AssertArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, true)?;
    let status = args
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<lam_core::MemoryStatus>())
        .transpose()?
        .unwrap_or(lam_core::MemoryStatus::Verified);
    let mut sources = Vec::new();
    if let Some(path) = args
        .source_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sources.push(lam_core::SourceEvidence {
            source_kind: parse_source_kind(&args.source_kind)?
                .unwrap_or(lam_core::SourceKind::Document),
            source_ref: path.to_string(),
            line_start: args.line_start,
            line_end: args.line_end,
            commit_sha: None,
            content_hash: None,
        });
    }
    let outcome = lam_storage::assert_fact(
        &mut conn,
        lam_storage::AssertInput {
            project_id: project.id,
            scope: lam_core::Scope::Project,
            kind: lam_core::MemoryKind::Fact,
            subject: args.subject.trim().to_string(),
            predicate: args.predicate.trim().to_string(),
            value: parse_value(&args.value, args.value_json)?,
            env: args.env.trim().to_string(),
            status,
            confidence: args.confidence,
            valid_from: parse_time(&args.valid_from, "valid_from")?.unwrap_or(now),
            valid_until: parse_time(&args.valid_until, "valid_until")?,
            actor: actor_of(&args.actor),
            reason: args.reason,
            sources,
            idempotency_key: args.idempotency_key,
        },
        now,
    )?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "memory_id": outcome.memory_id.to_string(),
                "revision_id": outcome.revision_id.to_string(),
                "assertion_id": outcome.assertion_id,
                "superseded": outcome.superseded,
                "conflict_id": outcome.conflict_id,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("asserted {}", outcome.assertion_id);
        for old in &outcome.superseded {
            println!("superseded {old}");
        }
        if let Some(conflict) = &outcome.conflict_id {
            println!("conflict {conflict} opened for review");
        }
    }
    Ok(())
}

fn assertion_json(view: &lam_storage::AssertionView) -> serde_json::Value {
    serde_json::json!({
        "assertion_id": view.id,
        "memory_id": view.memory_id,
        "value": view.value,
        "env": view.env,
        "revision_status": view.revision_status,
        "valid_from": view.valid_from.to_rfc3339(),
        "valid_until": view.valid_until.map(|t| t.to_rfc3339()),
        "recorded_at": view.recorded_at.to_rfc3339(),
        "retracted_at": view.retracted_at.map(|t| t.to_rfc3339()),
        "under_review": view.under_review,
        "confidence": view.confidence,
        "source_ref": view.source_ref,
    })
}

pub fn run_timeline(
    home: Option<PathBuf>,
    args: TimelineArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let rows = lam_storage::fact_timeline(
        &conn,
        &project.id,
        args.subject.trim(),
        args.predicate.trim(),
        args.env.trim(),
    )?;
    if as_json {
        let items: Vec<serde_json::Value> = rows.iter().map(assertion_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for row in &rows {
            println!(
                "{} valid {}..{} recorded {} {}",
                row.id,
                row.valid_from.to_rfc3339(),
                row.valid_until
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string()),
                row.recorded_at.to_rfc3339(),
                if row.under_review { "[review]" } else { "" },
            );
            println!("  value: {}", row.value);
        }
    }
    Ok(())
}

pub fn run_fact_get(
    home: Option<PathBuf>,
    args: FactGetArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let as_of = parse_time(&args.as_of, "as_of")?.unwrap_or(now);
    let mut rows = lam_storage::active_facts(
        &conn,
        &project.id,
        args.subject.trim(),
        args.predicate.trim(),
        args.env.trim(),
        as_of,
        now,
    )?;
    rows.sort_by_key(|a| std::cmp::Reverse(a.recorded_at));
    let active = rows.into_iter().next();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "active": active.as_ref().map(assertion_json),
                "as_of": as_of.to_rfc3339(),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        match active {
            Some(row) => println!("active value: {}", row.value),
            None => println!("no active fact at {}", as_of.to_rfc3339()),
        }
    }
    Ok(())
}

pub fn run_conflicts(
    home: Option<PathBuf>,
    args: ConflictsArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let rows = lam_storage::list_conflicts(&conn, &project.id, !args.all)?;
    if as_json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "conflict_id": row.id,
                    "subject": row.subject,
                    "predicate": row.predicate,
                    "env": row.env,
                    "status": row.status,
                    "reason": row.reason,
                    "active_value": row.active_value,
                    "incoming_value": row.incoming_value,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for row in &rows {
            println!(
                "{} [{}] {}={} active={} incoming={} ({})",
                row.id,
                row.status,
                row.subject,
                row.predicate,
                row.active_value,
                row.incoming_value,
                row.reason,
            );
        }
    }
    Ok(())
}

pub fn run_resolve(
    home: Option<PathBuf>,
    args: ResolveArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let keep: lam_storage::KeepSide = args.keep.parse()?;
    lam_storage::resolve_conflict(
        &mut conn,
        args.conflict_id.trim(),
        keep,
        &actor_of(&args.actor),
        args.reason.as_deref(),
        now,
    )?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "conflict_id": args.conflict_id.trim(),
                "status": "resolved",
                "keep": args.keep.trim(),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "resolved {} keeping {}",
            args.conflict_id.trim(),
            args.keep.trim()
        );
    }
    Ok(())
}

pub fn run_policy(
    home: Option<PathBuf>,
    args: PolicyArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let conn = lam_storage::open(&paths.db_file, 3000)?;
    if let (Some(predicate), Some(cardinality)) = (args.set.as_deref(), args.cardinality.as_deref())
    {
        let parsed = match cardinality.trim() {
            "single" => lam_storage::Cardinality::Single,
            "multi" => lam_storage::Cardinality::Multi,
            _ => {
                return Err(lam_core::LamError::validation(
                    "E_CARDINALITY_INVALID",
                    format!(
                        "cardinality must be single or multi, got: {}",
                        cardinality.trim()
                    ),
                ));
            }
        };
        lam_storage::set_cardinality(&conn, predicate, parsed, chrono::Utc::now())?;
    }
    let mut stmt = conn
        .prepare("SELECT predicate, cardinality FROM predicate_policy ORDER BY predicate ASC")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut items = Vec::new();
    for row in rows {
        let (predicate, cardinality) =
            row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        items.push(serde_json::json!({"predicate": predicate, "cardinality": cardinality}));
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for item in &items {
            println!(
                "{}={}",
                item.get("predicate").and_then(|v| v.as_str()).unwrap_or(""),
                item.get("cardinality")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
        }
    }
    Ok(())
}
