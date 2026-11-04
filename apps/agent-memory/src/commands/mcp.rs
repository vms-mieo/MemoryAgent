use std::path::Path;
use std::path::PathBuf;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use rmcp::ServiceExt;

pub fn run(home: Option<PathBuf>) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    runtime.block_on(serve(paths.data_dir))
}

async fn serve(home: PathBuf) -> Result<(), lam_core::LamError> {
    let server = LamServer { home };
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    service
        .waiting()
        .await
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    Ok(())
}

#[derive(Debug, Clone)]
struct LamServer {
    home: PathBuf,
}

fn mcp_error(error: lam_core::LamError) -> McpError {
    match error {
        lam_core::LamError::Typed { code, message } => {
            McpError::invalid_params(format!("{code}: {message}"), None)
        }
        other => McpError::internal_error(other.to_string(), None),
    }
}

fn open_workspace(
    home: &Path,
) -> Result<(rusqlite::Connection, lam_config::ResolvedPaths), lam_core::LamError> {
    let paths = lam_config::resolve_paths(Some(home.to_path_buf()));
    let text = std::fs::read_to_string(&paths.config_file)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let config = lam_config::Config::from_toml_str(&text)?;
    let conn = lam_storage::open(&paths.db_file, config.storage.busy_timeout_ms)?;
    Ok((conn, paths))
}

fn resolve_existing(
    conn: &mut rusqlite::Connection,
    project_id: &Option<String>,
    project_path: &Option<String>,
) -> Result<lam_core::Project, lam_core::LamError> {
    let id_text = project_id.as_deref().map(str::trim).unwrap_or("");
    if !id_text.is_empty() {
        let parsed = id_text.parse::<lam_core::ProjectId>().map_err(|_| {
            lam_core::LamError::validation(
                "E_PROJECT_REF_INVALID",
                format!("invalid project id: {id_text}"),
            )
        })?;
        return lam_storage::lookup_by_id(conn, &parsed);
    }
    let path_text = project_path.as_deref().map(str::trim).unwrap_or("");
    if !path_text.is_empty() {
        return lam_storage::resolve_existing_by_path(conn, Path::new(path_text));
    }
    Err(lam_core::LamError::validation(
        "E_PROJECT_REQUIRED",
        "project_id or project_path is required".to_string(),
    ))
}

fn resolve_for_save(
    conn: &mut rusqlite::Connection,
    project_id: &Option<String>,
    project_path: &Option<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<lam_core::Project, lam_core::LamError> {
    let id_text = project_id.as_deref().map(str::trim).unwrap_or("");
    if !id_text.is_empty() {
        return resolve_existing(conn, project_id, &None);
    }
    let path_text = project_path.as_deref().map(str::trim).unwrap_or("");
    if !path_text.is_empty() {
        return lam_storage::resolve_or_create(conn, Path::new(path_text), None, now);
    }
    Err(lam_core::LamError::validation(
        "E_PROJECT_REQUIRED",
        "project_id or project_path is required".to_string(),
    ))
}

fn parse_source_kind(raw: &Option<String>) -> Result<lam_core::SourceKind, lam_core::LamError> {
    let text = raw.as_deref().map(str::trim).unwrap_or("document");
    match text {
        "document" => Ok(lam_core::SourceKind::Document),
        "session" => Ok(lam_core::SourceKind::Session),
        "episode" => Ok(lam_core::SourceKind::Episode),
        "user" => Ok(lam_core::SourceKind::User),
        "config" => Ok(lam_core::SourceKind::Config),
        "test" => Ok(lam_core::SourceKind::Test),
        _ => Err(lam_core::LamError::validation(
            "E_SOURCE_KIND_INVALID",
            format!("unknown source kind: {text}"),
        )),
    }
}

fn checked_limit(raw: Option<u32>) -> Result<usize, lam_core::LamError> {
    let limit = raw.unwrap_or(5) as usize;
    if limit == 0 || limit > 100 {
        return Err(lam_core::LamError::validation(
            "E_LIMIT_INVALID",
            "limit must be within 1 and 100".to_string(),
        ));
    }
    Ok(limit)
}

fn checked_budget(raw: Option<u32>) -> Result<u32, lam_core::LamError> {
    let budget = raw.unwrap_or(200);
    if budget == 0 || budget > 8000 {
        return Err(lam_core::LamError::validation(
            "E_BUDGET_INVALID",
            "token_budget must be within 1 and 8000".to_string(),
        ));
    }
    Ok(budget)
}

fn as_of_warning(raw: &Option<String>) -> Result<Option<String>, lam_core::LamError> {
    let text = raw.as_deref().map(str::trim).unwrap_or("");
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<chrono::DateTime<chrono::Utc>>().map_err(|_| {
        lam_core::LamError::validation(
            "E_AS_OF_INVALID",
            format!("as_of must be RFC3339 datetime, got: {text}"),
        )
    })?;
    Ok(Some(
        "as_of filtering lands in Phase 3; results reflect current revisions".to_string(),
    ))
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    query: String,
    project_id: Option<String>,
    project_path: Option<String>,
    scope: Option<String>,
    limit: Option<u32>,
    include_history: Option<bool>,
    as_of: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetArgs {
    memory_id: String,
    revision_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SourceArgs {
    kind: Option<String>,
    path: Option<String>,
    line_start: Option<u32>,
    line_end: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SaveArgs {
    content: String,
    kind: String,
    project_id: Option<String>,
    project_path: Option<String>,
    scope: Option<String>,
    status: Option<String>,
    sources: Option<Vec<SourceArgs>>,
    idempotency_key: Option<String>,
    actor: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ContextArgs {
    task: String,
    project_id: Option<String>,
    project_path: Option<String>,
    token_budget: Option<u32>,
    as_of: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct VaultSearchArgs {
    query: String,
    project_id: Option<String>,
    project_path: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct VaultReadArgs {
    document: String,
    project_id: Option<String>,
    project_path: Option<String>,
    max_bytes: Option<usize>,
    start_line: Option<u32>,
    end_line: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReviewArgs {
    memory_id: String,
    action: String,
    reason: Option<String>,
    actor: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ForgetArgs {
    memory_id: String,
    mode: Option<String>,
    confirm: Option<bool>,
    actor: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AssertFactArgs {
    subject: String,
    predicate: String,
    value: serde_json::Value,
    project_id: Option<String>,
    project_path: Option<String>,
    env: Option<String>,
    status: Option<String>,
    confidence: Option<f64>,
    valid_from: Option<String>,
    source_ref: Option<String>,
    source_kind: Option<String>,
    actor: Option<String>,
    reason: Option<String>,
    idempotency_key: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TimelineArgs {
    subject: String,
    predicate: String,
    project_id: Option<String>,
    project_path: Option<String>,
    env: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GraphSearchArgs {
    query: String,
    project_id: Option<String>,
    project_path: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GraphTraverseArgs {
    entity_id: String,
    project_id: Option<String>,
    project_path: Option<String>,
    depth: Option<u8>,
    as_of: Option<String>,
}

fn assert_fact_impl(home: &Path, args: AssertFactArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let now = chrono::Utc::now();
    let project = resolve_for_save(&mut conn, &args.project_id, &args.project_path, now)?;
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
            source_kind: parse_source_kind(&args.source_kind)?,
            source_ref: path.to_string(),
            line_start: None,
            line_end: None,
            commit_sha: None,
            content_hash: None,
        });
    }
    let valid_from = args
        .valid_from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<chrono::DateTime<chrono::Utc>>().map_err(|_| {
                lam_core::LamError::validation(
                    "E_TIME_INVALID",
                    format!("valid_from must be RFC3339 datetime, got: {s}"),
                )
            })
        })
        .transpose()?
        .unwrap_or(now);
    let actor = args.actor.as_deref().map(str::trim).unwrap_or("mcp");
    let actor = if actor.is_empty() { "mcp" } else { actor };
    let outcome = lam_storage::assert_fact(
        &mut conn,
        lam_storage::AssertInput {
            project_id: project.id,
            scope: lam_core::Scope::Project,
            kind: lam_core::MemoryKind::Fact,
            subject: args.subject.trim().to_string(),
            predicate: args.predicate.trim().to_string(),
            value: args.value,
            env: args.env.as_deref().map(str::trim).unwrap_or("").to_string(),
            status,
            confidence: args.confidence.unwrap_or(0.9),
            valid_from,
            valid_until: None,
            actor: actor.to_string(),
            reason: args.reason,
            sources,
            idempotency_key: args.idempotency_key,
        },
        now,
    )?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "memory_id": outcome.memory_id.to_string(),
        "revision_id": outcome.revision_id.to_string(),
        "assertion_id": outcome.assertion_id,
        "superseded": outcome.superseded,
        "conflict_id": outcome.conflict_id,
    }))
    .map_err(lam_core::LamError::from)
}

fn timeline_impl(home: &Path, args: TimelineArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let rows = lam_storage::fact_timeline(
        &conn,
        &project.id,
        args.subject.trim(),
        args.predicate.trim(),
        args.env.as_deref().map(str::trim).unwrap_or(""),
    )?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "assertion_id": row.id,
                "value": row.value,
                "env": row.env,
                "revision_status": row.revision_status,
                "valid_from": row.valid_from.to_rfc3339(),
                "valid_until": row.valid_until.map(|t| t.to_rfc3339()),
                "recorded_at": row.recorded_at.to_rfc3339(),
                "retracted_at": row.retracted_at.map(|t| t.to_rfc3339()),
                "under_review": row.under_review,
                "confidence": row.confidence,
                "source_ref": row.source_ref,
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "project_id": project.id.to_string(),
        "items": items,
    }))
    .map_err(lam_core::LamError::from)
}

fn graph_search_impl(home: &Path, args: GraphSearchArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let limit = args.limit.unwrap_or(20).clamp(1, 100) as usize;
    let rows = lam_storage::search_entities(&conn, &project.id, &args.query, limit)?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "entity_id": row.id.to_string(),
                "name": row.canonical_name,
                "type": row.entity_type,
                "aliases": row.aliases,
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "project_id": project.id.to_string(),
        "items": items,
    }))
    .map_err(lam_core::LamError::from)
}

fn graph_traverse_impl(home: &Path, args: GraphTraverseArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let now = chrono::Utc::now();
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let start = args
        .entity_id
        .trim()
        .parse::<lam_core::EntityId>()
        .map_err(|_| {
            lam_core::LamError::validation(
                "E_ENTITY_ID_INVALID",
                format!("invalid entity id: {}", args.entity_id.trim()),
            )
        })?;
    let depth = args.depth.unwrap_or(2).clamp(1, 3);
    let as_of = args
        .as_of
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<chrono::DateTime<chrono::Utc>>().map_err(|_| {
                lam_core::LamError::validation(
                    "E_TIME_INVALID",
                    format!("as_of must be RFC3339 datetime, got: {s}"),
                )
            })
        })
        .transpose()?
        .unwrap_or(now);
    let view = lam_storage::traverse(&conn, &project.id, &start, depth, as_of)?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "project_id": project.id.to_string(),
        "nodes": view.nodes.iter().map(|node| serde_json::json!({
            "entity_id": node.id.to_string(),
            "name": node.canonical_name,
            "type": node.entity_type,
            "aliases": node.aliases,
        })).collect::<Vec<_>>(),
        "edges": view.edges.iter().map(|edge| serde_json::json!({
            "edge_id": edge.id,
            "source": edge.source_name,
            "relation": edge.relation_type,
            "target": edge.target_name,
            "valid_from": edge.valid_from.to_rfc3339(),
            "valid_until": edge.valid_until.map(|t| t.to_rfc3339()),
        })).collect::<Vec<_>>(),
    }))
    .map_err(lam_core::LamError::from)
}

fn search_impl(home: &Path, args: SearchArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let scope = args
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<lam_core::Scope>())
        .transpose()?;
    let warning = as_of_warning(&args.as_of)?;
    let mut warnings: Vec<String> = Vec::new();
    if let Some(note) = warning {
        warnings.push(note);
    }
    let hits = lam_storage::search_memories(
        &conn,
        lam_storage::SearchParams {
            project_id: project.id,
            query: args.query,
            scope,
            include_history: args.include_history.unwrap_or(false),
            limit: checked_limit(args.limit)?,
        },
    )?;
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
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "project_id": project.id.to_string(),
        "items": items,
        "warnings": warnings,
    }))
    .map_err(lam_core::LamError::from)
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

fn get_impl(home: &Path, args: GetArgs) -> Result<String, lam_core::LamError> {
    let (conn, _) = open_workspace(home)?;
    let memory_id = args
        .memory_id
        .trim()
        .parse::<lam_core::MemoryId>()
        .map_err(|_| {
            lam_core::LamError::validation(
                "E_MEMORY_ID_INVALID",
                format!("invalid memory id: {}", args.memory_id.trim()),
            )
        })?;
    let revision_id = args
        .revision_id
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
    let sources: Vec<serde_json::Value> = detail
        .sources
        .iter()
        .map(|source| {
            serde_json::json!({
                "source_kind": source_kind_text(&source.source_kind),
                "source_ref": source.source_ref,
                "line_start": source.line_start,
                "line_end": source.line_end,
                "commit_sha": source.commit_sha,
                "content_hash": source.content_hash,
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "memory_id": detail.memory.id.to_string(),
        "project_id": detail.memory.project_id.to_string(),
        "scope": detail.memory.scope.as_str(),
        "kind": detail.memory.kind.as_str(),
        "status": detail.revision.status.as_str(),
        "version": detail.revision.version,
        "content": detail.revision.content,
        "actor": detail.revision.actor,
        "reason": detail.revision.reason,
        "recorded_at": detail.revision.recorded_at.to_rfc3339(),
        "sources": sources,
    }))
    .map_err(lam_core::LamError::from)
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

fn save_impl(home: &Path, args: SaveArgs) -> Result<String, lam_core::LamError> {
    if args.content.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_CONTENT_INVALID",
            "content must not be empty".to_string(),
        ));
    }
    if args.content.len() > 65536 {
        return Err(lam_core::LamError::validation(
            "E_MEMORY_CONTENT_INVALID",
            "content exceeds 65536 bytes".to_string(),
        ));
    }
    let (mut conn, _) = open_workspace(home)?;
    let now = chrono::Utc::now();
    let project = resolve_for_save(&mut conn, &args.project_id, &args.project_path, now)?;
    let kind = args.kind.trim().parse::<lam_core::MemoryKind>()?;
    let scope = args
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<lam_core::Scope>())
        .transpose()?
        .unwrap_or(lam_core::Scope::Project);
    let status = args
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<lam_core::MemoryStatus>())
        .transpose()?
        .unwrap_or(lam_core::MemoryStatus::Verified);
    let mut sources = Vec::new();
    if let Some(list) = args.sources {
        if list.len() > 32 {
            return Err(lam_core::LamError::validation(
                "E_SOURCES_INVALID",
                "sources must contain at most 32 entries".to_string(),
            ));
        }
        for entry in list {
            let path = entry.path.as_deref().map(str::trim).unwrap_or("");
            if path.is_empty() {
                return Err(lam_core::LamError::validation(
                    "E_SOURCES_INVALID",
                    "each source needs a path".to_string(),
                ));
            }
            let evidence = lam_core::SourceEvidence {
                source_kind: parse_source_kind(&entry.kind)?,
                source_ref: path.to_string(),
                line_start: entry.line_start,
                line_end: entry.line_end,
                commit_sha: None,
                content_hash: None,
            };
            evidence.validate()?;
            sources.push(evidence);
        }
    }
    let actor = args.actor.as_deref().map(str::trim).unwrap_or("mcp");
    let actor = if actor.is_empty() { "mcp" } else { actor };
    let (memory_id, revision_id) = lam_storage::save_memory(
        &mut conn,
        lam_storage::SaveInput {
            project_id: project.id,
            scope,
            kind,
            content: args.content,
            status,
            actor: actor.to_string(),
            reason: args.reason,
            sources,
            idempotency_key: args.idempotency_key,
        },
        now,
    )?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "memory_id": memory_id.to_string(),
        "revision_id": revision_id.to_string(),
        "status": status.as_str(),
    }))
    .map_err(lam_core::LamError::from)
}

fn context_impl(home: &Path, args: ContextArgs) -> Result<String, lam_core::LamError> {
    let mode = args.mode.as_deref().map(str::trim).unwrap_or("fts");
    if mode == "hybrid" {
        return hybrid_context_impl(home, args);
    }
    if mode != "fts" {
        return Err(lam_core::LamError::validation(
            "E_MODE_INVALID",
            format!("mode must be fts or hybrid, got: {mode}"),
        ));
    }
    let (mut conn, _) = open_workspace(home)?;
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let mut response = lam_storage::build_context(
        &conn,
        &project.id,
        &args.task,
        checked_budget(args.token_budget)?,
    )?;
    if let Some(note) = as_of_warning(&args.as_of)? {
        response.warnings.push(note);
    }
    serde_json::to_string(&response).map_err(lam_core::LamError::from)
}

fn hybrid_context_impl(home: &Path, args: ContextArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, paths) = open_workspace(home)?;
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let budget = checked_budget(args.token_budget)?;
    let document = crate::commands::memory::hybrid_context_json(
        &conn,
        &paths,
        &project.id,
        &args.task,
        budget,
    )?;
    serde_json::to_string(&document).map_err(lam_core::LamError::from)
}

fn review_impl(home: &Path, args: ReviewArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let memory_id = args
        .memory_id
        .trim()
        .parse::<lam_core::MemoryId>()
        .map_err(|_| {
            lam_core::LamError::validation(
                "E_MEMORY_ID_INVALID",
                format!("invalid memory id: {}", args.memory_id.trim()),
            )
        })?;
    let status = match args.action.trim() {
        "approve" => lam_core::MemoryStatus::Verified,
        "reject" => lam_core::MemoryStatus::Archived,
        "dispute" => lam_core::MemoryStatus::Disputed,
        other => {
            return Err(lam_core::LamError::validation(
                "E_REVIEW_ACTION_INVALID",
                format!("action must be approve, reject or dispute, got: {other}"),
            ));
        }
    };
    let actor = args.actor.as_deref().map(str::trim).unwrap_or("mcp");
    let actor = if actor.is_empty() { "mcp" } else { actor };
    let now = chrono::Utc::now();
    let revision_id = lam_storage::set_status(
        &mut conn,
        &memory_id,
        status,
        actor,
        args.reason.as_deref(),
        now,
    )?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "memory_id": memory_id.to_string(),
        "revision_id": revision_id.to_string(),
        "status": status.as_str(),
    }))
    .map_err(lam_core::LamError::from)
}

fn forget_impl(home: &Path, args: ForgetArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let memory_id = args
        .memory_id
        .trim()
        .parse::<lam_core::MemoryId>()
        .map_err(|_| {
            lam_core::LamError::validation(
                "E_MEMORY_ID_INVALID",
                format!("invalid memory id: {}", args.memory_id.trim()),
            )
        })?;
    let mode = args.mode.as_deref().map(str::trim).unwrap_or("archive");
    let actor = args.actor.as_deref().map(str::trim).unwrap_or("mcp");
    let actor = if actor.is_empty() { "mcp" } else { actor };
    let now = chrono::Utc::now();
    match mode {
        "archive" => {
            let revision_id = lam_storage::set_status(
                &mut conn,
                &memory_id,
                lam_core::MemoryStatus::Archived,
                actor,
                Some("forget archive"),
                now,
            )?;
            serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "memory_id": memory_id.to_string(),
                "revision_id": revision_id.to_string(),
                "mode": "archive",
            }))
            .map_err(lam_core::LamError::from)
        }
        "purge" => {
            if args.confirm != Some(true) {
                return Err(lam_core::LamError::validation(
                    "E_CONFIRM_REQUIRED",
                    "purge requires confirm true".to_string(),
                ));
            }
            lam_storage::purge_memory(&mut conn, &memory_id, actor, now)?;
            serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "memory_id": memory_id.to_string(),
                "mode": "purge",
            }))
            .map_err(lam_core::LamError::from)
        }
        other => Err(lam_core::LamError::validation(
            "E_FORGET_MODE_INVALID",
            format!("mode must be archive or purge, got: {other}"),
        )),
    }
}

fn vault_search_impl(home: &Path, args: VaultSearchArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let limit = args.limit.unwrap_or(10).clamp(1, 100) as usize;
    let hits = lam_storage::search_documents(&conn, &project.id, &args.query, limit)?;
    let items: Vec<serde_json::Value> = hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "document_id": hit.document_id,
                "path": hit.rel_path,
                "title": hit.title,
                "snippet": hit.snippet,
                "line_start": hit.line_start,
                "line_end": hit.line_end,
                "rank": hit.rank,
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "project_id": project.id.to_string(),
        "items": items,
    }))
    .map_err(lam_core::LamError::from)
}

fn vault_read_impl(home: &Path, args: VaultReadArgs) -> Result<String, lam_core::LamError> {
    let (mut conn, _) = open_workspace(home)?;
    let project = resolve_existing(&mut conn, &args.project_id, &args.project_path)?;
    let max_bytes = args.max_bytes.unwrap_or(20000).clamp(1, 1_048_576);
    let read = lam_storage::read_document(
        &conn,
        &project.id,
        &args.document,
        max_bytes,
        args.start_line.map(|v| v as usize),
        args.end_line.map(|v| v as usize),
    )?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "document_id": read.document_id,
        "path": read.rel_path,
        "title": read.title,
        "total_lines": read.total_lines,
        "start_line": read.start_line,
        "end_line": read.end_line,
        "truncated": read.truncated,
        "text": read.text,
    }))
    .map_err(lam_core::LamError::from)
}

#[tool_router]
impl LamServer {
    #[tool(description = "Search project memories with keyword FTS, ranked with provenance")]
    fn memory_search(&self, Parameters(args): Parameters<SearchArgs>) -> Result<String, McpError> {
        search_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Read one memory with its current revision and evidence")]
    fn memory_get(&self, Parameters(args): Parameters<GetArgs>) -> Result<String, McpError> {
        get_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Save a memory with provenance; replay-safe with idempotency key")]
    fn memory_save(&self, Parameters(args): Parameters<SaveArgs>) -> Result<String, McpError> {
        save_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Build token-bounded reference context for a task")]
    fn memory_context(
        &self,
        Parameters(args): Parameters<ContextArgs>,
    ) -> Result<String, McpError> {
        context_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Approve, reject, or dispute a memory by appending a revision")]
    fn memory_review(&self, Parameters(args): Parameters<ReviewArgs>) -> Result<String, McpError> {
        review_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Archive a memory or purge it with explicit confirmation")]
    fn memory_forget(&self, Parameters(args): Parameters<ForgetArgs>) -> Result<String, McpError> {
        forget_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Assert a bitemporal fact with conflict detection")]
    fn memory_assert(
        &self,
        Parameters(args): Parameters<AssertFactArgs>,
    ) -> Result<String, McpError> {
        assert_fact_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Show fact validity timeline for a subject and predicate")]
    fn memory_timeline(
        &self,
        Parameters(args): Parameters<TimelineArgs>,
    ) -> Result<String, McpError> {
        timeline_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Search knowledge graph entities in a project")]
    fn knowledge_graph_search(
        &self,
        Parameters(args): Parameters<GraphSearchArgs>,
    ) -> Result<String, McpError> {
        graph_search_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Traverse knowledge graph edges valid at a point in time")]
    fn knowledge_graph_traverse(
        &self,
        Parameters(args): Parameters<GraphTraverseArgs>,
    ) -> Result<String, McpError> {
        graph_traverse_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Search vault documents with FTS snippets and citations")]
    fn vault_search(
        &self,
        Parameters(args): Parameters<VaultSearchArgs>,
    ) -> Result<String, McpError> {
        vault_search_impl(&self.home, args).map_err(mcp_error)
    }

    #[tool(description = "Read a bounded slice of a vault document")]
    fn vault_read(&self, Parameters(args): Parameters<VaultReadArgs>) -> Result<String, McpError> {
        vault_read_impl(&self.home, args).map_err(mcp_error)
    }
}

#[tool_handler]
impl ServerHandler for LamServer {}
