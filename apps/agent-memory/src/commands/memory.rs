use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

use super::dedup;
use super::facts;
use super::graph;

#[derive(Debug, Subcommand)]
pub enum MemoryAction {
    Add(AddArgs),
    Search(SearchArgs),
    Show(ShowArgs),
    Context(ContextArgs),
    Review(ReviewArgs),
    Approve(StateArgs),
    Archive(StateArgs),
    Forget(ForgetArgs),
    Assert(facts::AssertArgs),
    Timeline(facts::TimelineArgs),
    FactGet(facts::FactGetArgs),
    Conflicts(facts::ConflictsArgs),
    Resolve(facts::ResolveArgs),
    PredicatePolicy(facts::PolicyArgs),
    Dedup {
        #[command(subcommand)]
        action: Box<dedup::DedupAction>,
    },
    Graph {
        #[command(subcommand)]
        action: Box<graph::GraphAction>,
    },
}

#[derive(Debug, Args)]
pub struct AddArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub kind: String,
    #[arg(long)]
    pub text: String,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long)]
    pub status: Option<String>,
    #[arg(long)]
    pub source_ref: Option<String>,
    #[arg(long)]
    pub source_kind: Option<String>,
    #[arg(long)]
    pub line_start: Option<u32>,
    #[arg(long)]
    pub line_end: Option<u32>,
    #[arg(long)]
    pub idempotency_key: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub query: String,
    #[arg(long)]
    pub limit: Option<u32>,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long)]
    pub include_history: bool,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub id: String,
    #[arg(long)]
    pub revision: Option<String>,
}

#[derive(Debug, Args)]
pub struct ContextArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub task: String,
    #[arg(long, default_value_t = 200)]
    pub token_budget: u32,
    #[arg(long, default_value_t = false)]
    pub hybrid: bool,
}

#[derive(Debug, Args)]
pub struct ReviewArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub list: bool,
}

#[derive(Debug, Args)]
pub struct StateArgs {
    pub id: String,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
}

#[derive(Debug, Args)]
pub struct ForgetArgs {
    pub id: String,
    #[arg(long)]
    pub purge: bool,
    #[arg(long)]
    pub actor: Option<String>,
}

pub fn run(
    home: Option<PathBuf>,
    action: MemoryAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        MemoryAction::Add(args) => run_add(home, args, as_json),
        MemoryAction::Search(args) => run_search(home, args, as_json),
        MemoryAction::Show(args) => run_show(home, args, as_json),
        MemoryAction::Context(args) => run_context(home, args, as_json),
        MemoryAction::Review(args) => run_review(home, args, as_json),
        MemoryAction::Approve(args) => {
            run_set_status(home, args, lam_core::MemoryStatus::Verified, as_json)
        }
        MemoryAction::Archive(args) => {
            run_set_status(home, args, lam_core::MemoryStatus::Archived, as_json)
        }
        MemoryAction::Forget(args) => run_forget(home, args, as_json),
        MemoryAction::Assert(args) => facts::run_assert(home, args, as_json),
        MemoryAction::Timeline(args) => facts::run_timeline(home, args, as_json),
        MemoryAction::FactGet(args) => facts::run_fact_get(home, args, as_json),
        MemoryAction::Conflicts(args) => facts::run_conflicts(home, args, as_json),
        MemoryAction::Resolve(args) => facts::run_resolve(home, args, as_json),
        MemoryAction::PredicatePolicy(args) => facts::run_policy(home, args, as_json),
        MemoryAction::Dedup { action } => dedup::run(home, *action, as_json),
        MemoryAction::Graph { action } => graph::run(home, *action, as_json),
    }
}

fn parse_scope(raw: &Option<String>) -> Result<Option<lam_core::Scope>, lam_core::LamError> {
    raw.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<lam_core::Scope>())
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

fn run_add(home: Option<PathBuf>, args: AddArgs, as_json: bool) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, true)?;
    let kind = args.kind.trim().parse::<lam_core::MemoryKind>()?;
    let scope = parse_scope(&args.scope)?.unwrap_or(lam_core::Scope::Project);
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
        let kind_text = args
            .source_kind
            .as_deref()
            .map(str::trim)
            .unwrap_or("document");
        let source_kind = match kind_text {
            "document" => lam_core::SourceKind::Document,
            "session" => lam_core::SourceKind::Session,
            "episode" => lam_core::SourceKind::Episode,
            "user" => lam_core::SourceKind::User,
            "config" => lam_core::SourceKind::Config,
            "test" => lam_core::SourceKind::Test,
            _ => {
                return Err(lam_core::LamError::validation(
                    "E_SOURCE_KIND_INVALID",
                    format!("unknown source kind: {kind_text}"),
                ));
            }
        };
        let evidence = lam_core::SourceEvidence {
            source_kind,
            source_ref: path.to_string(),
            line_start: args.line_start,
            line_end: args.line_end,
            commit_sha: None,
            content_hash: None,
        };
        evidence.validate()?;
        sources.push(evidence);
    }
    let (memory_id, revision_id) = lam_storage::save_memory(
        &mut conn,
        lam_storage::SaveInput {
            project_id: project.id,
            scope,
            kind,
            content: args.text,
            status,
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
                "memory_id": memory_id.to_string(),
                "revision_id": revision_id.to_string(),
                "status": status.as_str(),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("saved {memory_id} revision {revision_id}");
    }
    Ok(())
}

fn run_search(
    home: Option<PathBuf>,
    args: SearchArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, paths) = crate::commands::open_existing(&home)?;
    let project = lam_storage::resolve_ref(&mut conn, &args.project, chrono::Utc::now(), false)?;
    let limit = args.limit.unwrap_or(5) as usize;
    let hits = lam_storage::search_memories(
        &conn,
        lam_storage::SearchParams {
            project_id: project.id,
            query: args.query,
            scope: parse_scope(&args.scope)?,
            include_history: args.include_history,
            limit,
        },
    )?;
    let hits = apply_read_admission(&conn, &paths, &hits);
    if as_json {
        let items: Vec<serde_json::Value> = hits
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "memory_id": hit.memory_id.to_string(),
                    "kind": hit.kind.as_str(),
                    "status": hit.status.as_str(),
                    "snippet": hit.snippet,
                    "source": source_label(hit),
                    "relevance": lam_core::context::relevance_from_rank(hit.rank),
                    "recorded_at": hit.recorded_at.to_rfc3339(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for hit in &hits {
            println!(
                "{} [{}|{}] {} ({})",
                hit.memory_id,
                hit.kind.as_str(),
                hit.status.as_str(),
                hit.snippet,
                source_label(hit)
            );
        }
    }
    Ok(())
}

fn apply_read_admission(
    conn: &rusqlite::Connection,
    paths: &lam_config::ResolvedPaths,
    hits: &[lam_storage::SearchHit],
) -> Vec<lam_storage::SearchHit> {
    let config = std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|text| lam_config::Config::from_toml_str(&text).ok());
    let Some(config) = config else {
        return hits.to_vec();
    };
    if config.admission.mode.trim().to_lowercase() != "gate" {
        return hits.to_vec();
    }
    let scored: Vec<lam_consolidate::ScoredItem> = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            let (score, _) = lam_consolidate::score_content(&hit.snippet);
            lam_consolidate::ScoredItem { index, score }
        })
        .collect();
    let kept = lam_consolidate::decide_read_scores(
        &config.admission.mode,
        &scored,
        config.admission.minimum_confidence,
        false,
        200,
    );
    let decision = lam_consolidate::decide_write(
        &config.admission.mode,
        "read seam filter",
        &lam_consolidate::WriteOptions {
            minimum_confidence: config.admission.minimum_confidence,
            ..Default::default()
        },
    );
    let _ = lam_consolidate::record_trace(conn, "read", &decision, true, chrono::Utc::now());
    let mut out = Vec::with_capacity(kept.len());
    for index in kept {
        if let Some(hit) = hits.get(index) {
            out.push(hit.clone());
        }
    }
    out
}

fn source_label(hit: &lam_storage::SearchHit) -> String {
    if hit.source_ref.is_empty() {
        return "unknown".to_string();
    }
    match (hit.line_start, hit.line_end) {
        (Some(start), Some(end)) => format!("{}:{start}-{end}", hit.source_ref),
        _ => hit.source_ref.clone(),
    }
}

fn run_show(
    home: Option<PathBuf>,
    args: ShowArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (conn, _) = crate::commands::open_existing(&home)?;
    let memory_id = args.id.trim().parse::<lam_core::MemoryId>().map_err(|_| {
        lam_core::LamError::validation(
            "E_MEMORY_ID_INVALID",
            format!("invalid memory id: {}", args.id.trim()),
        )
    })?;
    let revision_id = args
        .revision
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<lam_core::RevisionId>().map_err(|_| {
                lam_core::LamError::validation(
                    "E_REVISION_ID_INVALID",
                    format!("invalid revision id: {s}"),
                )
            })
        })
        .transpose()?;
    let detail = lam_storage::get_memory_at_revision(&conn, &memory_id, revision_id.as_ref())?;
    if as_json {
        let sources: Vec<serde_json::Value> = detail
            .sources
            .iter()
            .map(|source| {
                serde_json::json!({
                    "source_ref": source.source_ref,
                    "line_start": source.line_start,
                    "line_end": source.line_end,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "memory_id": detail.memory.id.to_string(),
                "scope": detail.memory.scope.as_str(),
                "kind": detail.memory.kind.as_str(),
                "status": detail.revision.status.as_str(),
                "version": detail.revision.version,
                "content": detail.revision.content,
                "sources": sources,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "{} [{}|{}] v{}",
            detail.memory.id,
            detail.memory.kind.as_str(),
            detail.revision.status.as_str(),
            detail.revision.version
        );
        println!("{}", detail.revision.content);
        for source in &detail.sources {
            println!("source: {}", source.source_ref);
        }
    }
    Ok(())
}

fn run_context(
    home: Option<PathBuf>,
    args: ContextArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, paths) = crate::commands::open_existing(&home)?;
    let project = lam_storage::resolve_ref(&mut conn, &args.project, chrono::Utc::now(), false)?;
    if args.hybrid {
        return run_hybrid_context(
            &conn,
            &paths,
            &project.id,
            &args.task,
            args.token_budget,
            as_json,
        );
    }
    let response = lam_storage::build_context(&conn, &project.id, &args.task, args.token_budget)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("{}", response.context);
        if !response.warnings.is_empty() {
            for warning in &response.warnings {
                println!("warning: {warning}");
            }
        }
    }
    Ok(())
}

fn load_embedding_config(
    paths: &lam_config::ResolvedPaths,
) -> Result<lam_config::Config, lam_core::LamError> {
    let text = std::fs::read_to_string(&paths.config_file)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    lam_config::Config::from_toml_str(&text)
}

fn hybrid_query_vector(
    config: &lam_config::Config,
    task: &str,
) -> (Option<(Vec<f32>, String)>, Vec<String>) {
    if config.models.embedding.provider.as_str() == "none" {
        return (
            None,
            vec!["vector search unavailable; FTS-only".to_string()],
        );
    }
    let embedder = match lam_providers::embedding_for(&config.models.embedding, 5000) {
        Err(e) => {
            return (
                None,
                vec![format!("vector search unavailable: {}", e.code())],
            )
        }
        Ok(found) => found,
    };
    let request = lam_providers::EmbedRequest { texts: &[task] };
    match embedder.embed(&request) {
        Err(e) => (
            None,
            vec![format!("vector search unavailable: {}", e.code())],
        ),
        Ok(vectors) => {
            if vectors.len() != 1 || vectors[0].is_empty() {
                (
                    None,
                    vec!["vector search unavailable: empty embedding".to_string()],
                )
            } else {
                let model = embedder.model_id();
                (
                    Some((vectors.into_iter().next().expect("one vector"), model)),
                    Vec::new(),
                )
            }
        }
    }
}

pub(crate) fn hybrid_context_json(
    conn: &rusqlite::Connection,
    paths: &lam_config::ResolvedPaths,
    project_id: &lam_core::ProjectId,
    task: &str,
    token_budget: u32,
) -> Result<serde_json::Value, lam_core::LamError> {
    let config = load_embedding_config(paths)?;
    let budget = token_budget.clamp(1, 8000);
    let (vector, mut warnings) = hybrid_query_vector(&config, task);
    let vector_spec = vector
        .as_ref()
        .map(|(values, model)| lam_retrieval::HybridVector {
            query: values,
            model_id: model,
            dimensions: values.len(),
            top_k: 10,
        });
    let now = chrono::Utc::now();
    let result = lam_retrieval::hybrid_search(conn, project_id, task, 10, vector_spec, now)?;
    warnings.extend(result.vector_warnings.iter().cloned());
    let mut context = lam_retrieval::build_hybrid_context(task, result.fused, budget, 10)?;
    context.warnings.extend(warnings);
    Ok(serde_json::json!({
        "schema_version": 1,
        "task": context.task,
        "token_budget": context.token_budget,
        "estimated_tokens": context.estimated_tokens,
        "context": context.text,
        "items": context.items,
        "warnings": context.warnings,
        "vector_model": result.vector_model,
    }))
}

pub(crate) fn run_hybrid_context(
    conn: &rusqlite::Connection,
    paths: &lam_config::ResolvedPaths,
    project_id: &lam_core::ProjectId,
    task: &str,
    token_budget: u32,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let document = hybrid_context_json(conn, paths, project_id, task, token_budget)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        if let Some(text) = document.get("context").and_then(|v| v.as_str()) {
            println!("{text}");
        }
        if let Some(warnings) = document.get("warnings").and_then(|v| v.as_array()) {
            for warning in warnings {
                if let Some(text) = warning.as_str() {
                    println!("warning: {text}");
                }
            }
        }
    }
    Ok(())
}

fn run_review(
    home: Option<PathBuf>,
    args: ReviewArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    if !args.list {
        return Err(lam_core::LamError::validation(
            "E_ARGS_INVALID",
            "memory review requires --list".to_string(),
        ));
    }
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let project = lam_storage::resolve_ref(&mut conn, &args.project, chrono::Utc::now(), false)?;
    let items = lam_storage::review_list(&conn, &project.id)?;
    if as_json {
        let values: Vec<serde_json::Value> = items
            .iter()
            .map(|detail| {
                serde_json::json!({
                    "memory_id": detail.memory.id.to_string(),
                    "kind": detail.memory.kind.as_str(),
                    "status": detail.revision.status.as_str(),
                    "content": detail.revision.content,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&values).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for detail in &items {
            println!(
                "{} [{}] {}",
                detail.memory.id,
                detail.revision.status.as_str(),
                detail.revision.content
            );
        }
    }
    Ok(())
}

fn run_set_status(
    home: Option<PathBuf>,
    args: StateArgs,
    status: lam_core::MemoryStatus,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let memory_id = args.id.trim().parse::<lam_core::MemoryId>().map_err(|_| {
        lam_core::LamError::validation(
            "E_MEMORY_ID_INVALID",
            format!("invalid memory id: {}", args.id.trim()),
        )
    })?;
    let now = chrono::Utc::now();
    let revision_id = lam_storage::set_status(
        &mut conn,
        &memory_id,
        status,
        &actor_of(&args.actor),
        args.reason.as_deref(),
        now,
    )?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "memory_id": memory_id.to_string(),
                "revision_id": revision_id.to_string(),
                "status": status.as_str(),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("updated {memory_id} to {}", status.as_str());
    }
    Ok(())
}

fn run_forget(
    home: Option<PathBuf>,
    args: ForgetArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let memory_id = args.id.trim().parse::<lam_core::MemoryId>().map_err(|_| {
        lam_core::LamError::validation(
            "E_MEMORY_ID_INVALID",
            format!("invalid memory id: {}", args.id.trim()),
        )
    })?;
    let now = chrono::Utc::now();
    let actor = actor_of(&args.actor);
    let mode = if args.purge {
        lam_storage::purge_memory(&mut conn, &memory_id, &actor, now)?;
        "purge"
    } else {
        lam_storage::set_status(
            &mut conn,
            &memory_id,
            lam_core::MemoryStatus::Archived,
            &actor,
            Some("forget archive"),
            now,
        )?;
        "archive"
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "memory_id": memory_id.to_string(),
                "mode": mode,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("forgot {memory_id} mode {mode}");
        if args.purge {
            println!("note: backups taken before this purge may still retain the purged data");
        }
    }
    Ok(())
}
