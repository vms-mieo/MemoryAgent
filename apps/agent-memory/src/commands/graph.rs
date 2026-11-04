use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum GraphAction {
    Search(GraphSearchArgs),
    Traverse(GraphTraverseArgs),
    EntityAdd(EntityAddArgs),
    EntityAlias(EntityAliasArgs),
    EdgeAdd(EdgeAddArgs),
}

#[derive(Debug, Args)]
pub struct GraphSearchArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub query: String,
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
}

#[derive(Debug, Args)]
pub struct GraphTraverseArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub entity: String,
    #[arg(long, default_value_t = 2)]
    pub depth: u8,
    #[arg(long)]
    pub as_of: Option<String>,
}

#[derive(Debug, Args)]
pub struct EntityAddArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub kind: String,
}

#[derive(Debug, Args)]
pub struct EntityAliasArgs {
    #[arg(long)]
    pub project: String,
    pub id: String,
    #[arg(long)]
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct EdgeAddArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub source: String,
    #[arg(long)]
    pub relation: String,
    #[arg(long)]
    pub target: String,
    #[arg(long)]
    pub valid_from: Option<String>,
    #[arg(long)]
    pub valid_until: Option<String>,
}

pub fn run(
    home: Option<PathBuf>,
    action: GraphAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        GraphAction::Search(args) => run_search(home, args, as_json),
        GraphAction::Traverse(args) => run_traverse(home, args, as_json),
        GraphAction::EntityAdd(args) => run_entity_add(home, args, as_json),
        GraphAction::EntityAlias(args) => run_entity_alias(home, args, as_json),
        GraphAction::EdgeAdd(args) => run_edge_add(home, args, as_json),
    }
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

fn run_search(
    home: Option<PathBuf>,
    args: GraphSearchArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let limit = args.limit.clamp(1, 100) as usize;
    let rows = lam_storage::search_entities(&conn, &project.id, &args.query, limit)?;
    if as_json {
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
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for row in &rows {
            println!("{} [{}] {}", row.id, row.entity_type, row.canonical_name);
        }
    }
    Ok(())
}

fn run_traverse(
    home: Option<PathBuf>,
    args: GraphTraverseArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let start = args
        .entity
        .trim()
        .parse::<lam_core::EntityId>()
        .map_err(|_| {
            lam_core::LamError::validation(
                "E_ENTITY_ID_INVALID",
                format!("invalid entity id: {}", args.entity.trim()),
            )
        })?;
    let as_of = parse_time(&args.as_of, "as_of")?.unwrap_or(now);
    let view = lam_storage::traverse(&conn, &project.id, &start, args.depth, as_of)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
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
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        for edge in &view.edges {
            println!(
                "{} -[{}]-> {} ({}..{})",
                edge.source_name,
                edge.relation_type,
                edge.target_name,
                edge.valid_from.to_rfc3339(),
                edge.valid_until
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
    }
    Ok(())
}

fn run_entity_add(
    home: Option<PathBuf>,
    args: EntityAddArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, true)?;
    let id = lam_storage::upsert_entity(&conn, &project.id, &args.name, &args.kind, None)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"entity_id": id.to_string()}))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("entity {id}");
    }
    Ok(())
}

fn run_entity_alias(
    home: Option<PathBuf>,
    args: EntityAliasArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let id = args.id.trim().parse::<lam_core::EntityId>().map_err(|_| {
        lam_core::LamError::validation(
            "E_ENTITY_ID_INVALID",
            format!("invalid entity id: {}", args.id.trim()),
        )
    })?;
    lam_storage::attach_alias(&conn, &project.id, &id, &args.alias)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "entity_id": id.to_string(),
                "alias": args.alias.trim(),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("alias {} -> {id}", args.alias.trim());
    }
    Ok(())
}

fn run_edge_add(
    home: Option<PathBuf>,
    args: EdgeAddArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let parse_id = |raw: &str| {
        raw.trim().parse::<lam_core::EntityId>().map_err(|_| {
            lam_core::LamError::validation(
                "E_ENTITY_ID_INVALID",
                format!("invalid entity id: {}", raw.trim()),
            )
        })
    };
    let id = lam_storage::add_edge(
        &conn,
        lam_storage::EdgeInput {
            project_id: &project.id,
            source_id: &parse_id(&args.source)?,
            relation_type: &args.relation,
            target_id: &parse_id(&args.target)?,
            valid_from: parse_time(&args.valid_from, "valid_from")?.unwrap_or(now),
            valid_until: parse_time(&args.valid_until, "valid_until")?,
            source_ref: None,
            now,
        },
    )?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"edge_id": id.to_string()}))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("edge {id}");
    }
    Ok(())
}
