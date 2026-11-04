use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum VaultAction {
    Register(RegisterArgs),
    List(ListArgs),
    Sync(SyncArgs),
    Search(SearchArgs),
    Read(ReadArgs),
    Links(LinksArgs),
    Unregister(UnregisterArgs),
    Reindex(ReindexArgs),
}

#[derive(Debug, Args)]
pub struct RegisterArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[arg(long)]
    pub project: String,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    #[arg(long)]
    pub project: String,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub query: String,
    #[arg(long, default_value_t = 10)]
    pub limit: u32,
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub document: String,
    #[arg(long)]
    pub max_bytes: Option<usize>,
    #[arg(long)]
    pub start_line: Option<usize>,
    #[arg(long)]
    pub end_line: Option<usize>,
}

#[derive(Debug, Args)]
pub struct LinksArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub document: String,
}

#[derive(Debug, Args)]
pub struct UnregisterArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub path: String,
}

#[derive(Debug, Args)]
pub struct ReindexArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long, default_value_t = 15000)]
    pub timeout_ms: u64,
}

pub fn run(
    home: Option<PathBuf>,
    action: VaultAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        VaultAction::Register(args) => run_register(home, args, as_json),
        VaultAction::List(args) => run_list(home, args, as_json),
        VaultAction::Sync(args) => run_sync(home, args, as_json),
        VaultAction::Search(args) => run_search(home, args, as_json),
        VaultAction::Read(args) => run_read(home, args, as_json),
        VaultAction::Links(args) => run_links(home, args, as_json),
        VaultAction::Unregister(args) => run_unregister(home, args, as_json),
        VaultAction::Reindex(args) => run_reindex(home, args, as_json),
    }
}

fn policy_from_config(
    config: &lam_config::Config,
) -> Result<lam_vault::ScanPolicy, lam_core::LamError> {
    lam_vault::ScanPolicy::new(config.vault.max_file_bytes, config.vault.follow_symlinks)
}

fn run_register(
    home: Option<PathBuf>,
    args: RegisterArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, true)?;
    let root_id = lam_storage::register_root(&conn, &project.id, &args.path, now)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"root_id": root_id}))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("registered {root_id}");
    }
    Ok(())
}

fn run_list(
    home: Option<PathBuf>,
    args: ListArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let roots = lam_storage::list_roots(&conn, &project.id)?;
    if as_json {
        let items: Vec<serde_json::Value> = roots
            .iter()
            .map(|root| {
                serde_json::json!({"root_id": root.id, "path": root.path, "created_at": root.created_at})
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for root in &roots {
            println!("{} {}", root.id, root.path);
        }
    }
    Ok(())
}

fn run_sync(
    home: Option<PathBuf>,
    args: SyncArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, config) = crate::commands::init::ensure_workspace(home)?;
    let conn = lam_storage::open(&paths.db_file, config.storage.busy_timeout_ms)?;
    let now = chrono::Utc::now();
    let mut owned = conn;
    let project = lam_storage::resolve_ref(&mut owned, &args.project, now, false)?;
    let policy = policy_from_config(&config)?;
    let report = lam_storage::sync_project(&owned, &project.id, &policy, now)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "processed": report.processed,
                "skipped": report.skipped,
                "stale": report.stale,
                "warnings": report.warnings,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "processed={} skipped={} stale={}",
            report.processed, report.skipped, report.stale
        );
        for warning in &report.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}

fn run_search(
    home: Option<PathBuf>,
    args: SearchArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let limit = args.limit.clamp(1, 100) as usize;
    let hits = lam_storage::search_documents(&conn, &project.id, &args.query, limit)?;
    if as_json {
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
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for hit in &hits {
            println!(
                "{} {} ({}:{})",
                hit.document_id, hit.title, hit.rel_path, hit.line_start
            );
            println!("  {}", hit.snippet);
        }
    }
    Ok(())
}

fn run_read(
    home: Option<PathBuf>,
    args: ReadArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, paths) = crate::commands::open_existing(&home)?;
    let max_default: usize = std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|text| lam_config::Config::from_toml_str(&text).ok())
        .map(|config| config.vault.max_read_bytes as usize)
        .unwrap_or(20000);
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let max_bytes = args.max_bytes.unwrap_or(max_default);
    let read = lam_storage::read_document(
        &conn,
        &project.id,
        &args.document,
        max_bytes,
        args.start_line,
        args.end_line,
    )?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "document_id": read.document_id,
                "path": read.rel_path,
                "title": read.title,
                "total_lines": read.total_lines,
                "start_line": read.start_line,
                "end_line": read.end_line,
                "truncated": read.truncated,
                "text": read.text,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "{} (lines {}-{} of {})",
            read.rel_path, read.start_line, read.end_line, read.total_lines
        );
        println!("{}", read.text);
        if read.truncated {
            println!("...[truncated]");
        }
    }
    Ok(())
}

fn run_links(
    home: Option<PathBuf>,
    args: LinksArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let links = lam_storage::links_of(&conn, &project.id, &args.document)?;
    if as_json {
        let render = |items: &[lam_storage::DocLink]| {
            items
                .iter()
                .map(|link| {
                    serde_json::json!({
                        "document_id": link.other_id,
                        "path": link.other_path,
                        "anchor": link.anchor,
                    })
                })
                .collect::<Vec<_>>()
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "outgoing": render(&links.outgoing),
                "backlinks": render(&links.incoming),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("outgoing:");
        for link in &links.outgoing {
            println!("  -> {}", link.other_path);
        }
        println!("backlinks:");
        for link in &links.incoming {
            println!("  <- {}", link.other_path);
        }
    }
    Ok(())
}

fn run_unregister(
    home: Option<PathBuf>,
    args: UnregisterArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let mut owned = conn;
    let project = lam_storage::resolve_ref(&mut owned, &args.project, now, false)?;
    let removed = lam_storage::unregister_root(&owned, &project.id, &args.path)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"removed": removed}))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else if removed {
        println!("unregistered {}", args.path);
    } else {
        println!("no vault root {} registered", args.path);
    }
    Ok(())
}

fn run_reindex(
    home: Option<PathBuf>,
    args: ReindexArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, config) = crate::commands::init::ensure_workspace(home)?;
    let conn = lam_storage::open(&paths.db_file, config.storage.busy_timeout_ms)?;
    if config.models.embedding.provider.as_str() == "none" {
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "embedded": 0,
                    "skipped": 0,
                    "mode": "fts-only",
                }))
                .unwrap_or_else(|_| "{}".to_string())
            );
        } else {
            println!("no embedding provider configured; staying degraded FTS-only");
        }
        return Ok(());
    }
    let now = chrono::Utc::now();
    let mut owned = conn;
    let project = lam_storage::resolve_ref(&mut owned, &args.project, now, false)?;
    let embedder = lam_providers::embedding_for(&config.models.embedding, args.timeout_ms)?;
    let roots = lam_storage::list_roots(&owned, &project.id)?;
    let mut state = ReindexState::default();
    for root in &roots {
        let documents = vault_documents(&owned, &project.id, &root.id)?;
        for (doc_id, _) in documents {
            let chunks = vault_chunks(&owned, &doc_id)?;
            let mut pending: Vec<(String, String)> = Vec::new();
            for (chunk_id, content) in chunks {
                pending.push((chunk_id, content));
                if pending.len() >= 16 {
                    state.embed_batch(&owned, &*embedder, &pending)?;
                    pending.clear();
                }
            }
            if !pending.is_empty() {
                let batch = std::mem::take(&mut pending);
                state.embed_batch(&owned, &*embedder, &batch)?;
            }
        }
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "embedded": state.embedded,
                "skipped": state.skipped,
                "model": state.model_id,
                "dimensions": state.dimensions,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "embedded={} skipped={} model={} dims={}",
            state.embedded, state.skipped, state.model_id, state.dimensions
        );
    }
    Ok(())
}

#[derive(Default)]
struct ReindexState {
    embedded: usize,
    skipped: usize,
    model_id: String,
    dimensions: usize,
    registered: bool,
}

impl ReindexState {
    fn embed_batch(
        &mut self,
        conn: &rusqlite::Connection,
        embedder: &dyn lam_providers::Embedder,
        batch: &[(String, String)],
    ) -> Result<(), lam_core::LamError> {
        if batch.is_empty() {
            return Ok(());
        }
        let texts: Vec<&str> = batch.iter().map(|(_, text)| text.as_str()).collect();
        let vectors = embedder.embed(&lam_providers::EmbedRequest { texts: &texts })?;
        if vectors.len() != batch.len() {
            return Err(lam_core::LamError::validation(
                "E_EMBED_MISMATCH",
                "embedding count does not match input batch".to_string(),
            ));
        }
        let dims = vectors.first().map(|v| v.len()).unwrap_or(0);
        if dims == 0 {
            return Err(lam_core::LamError::validation(
                "E_EMBED_INVALID",
                "embedding returned empty vectors".to_string(),
            ));
        }
        if !self.registered {
            let model = embedder.model_id();
            let (_, _) = lam_storage::ensure_model(conn, &model, dims)?;
            self.model_id = model;
            self.dimensions = dims;
            self.registered = true;
        }
        for ((chunk_id, _), vector) in batch.iter().zip(vectors.iter()) {
            if chunk_has_vector(conn, chunk_id, &self.model_id, self.dimensions)? {
                self.skipped += 1;
                continue;
            }
            lam_storage::store_vector(
                conn,
                "chunk",
                chunk_id,
                &self.model_id,
                self.dimensions,
                vector,
            )?;
            self.embedded += 1;
        }
        Ok(())
    }
}

fn chunk_has_vector(
    conn: &rusqlite::Connection,
    chunk_id: &str,
    model_id: &str,
    dimensions: usize,
) -> Result<bool, lam_core::LamError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vector_index WHERE owner_kind = 'chunk' AND owner_id = ?1 AND embedding_model = ?2 AND dimensions = ?3",
            rusqlite::params![chunk_id, model_id, dimensions as i64],
            |row| row.get(0),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(count > 0)
}

fn vault_documents(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    root_id: &str,
) -> Result<Vec<(String, String)>, lam_core::LamError> {
    conn.prepare("SELECT id, relative_path FROM documents WHERE project_id = ?1 AND root_id = ?2 AND status = 'ok'")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .query_map(
            rusqlite::params![project_id.to_string_value(), root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .collect::<Result<Vec<(String, String)>, _>>()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))
}

fn vault_chunks(
    conn: &rusqlite::Connection,
    doc_id: &str,
) -> Result<Vec<(String, String)>, lam_core::LamError> {
    conn.prepare(
        "SELECT id, content FROM document_chunks WHERE document_id = ?1 ORDER BY ordinal ASC",
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?
    .query_map(rusqlite::params![doc_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?
    .collect::<Result<Vec<(String, String)>, _>>()
    .map_err(|e| lam_core::LamError::storage(e.to_string()))
}
