use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum DedupAction {
    List(DedupListArgs),
    Resolve(DedupResolveArgs),
}

#[derive(Debug, Args)]
pub struct DedupListArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct DedupResolveArgs {
    pub suggestion_id: String,
    #[arg(long)]
    pub keep: Option<String>,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
}

pub fn run(
    home: Option<PathBuf>,
    action: DedupAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        DedupAction::List(args) => run_list(home, args, as_json),
        DedupAction::Resolve(args) => run_resolve(home, args, as_json),
    }
}

fn actor_of(raw: &Option<String>) -> String {
    let text = raw.as_deref().map(str::trim).unwrap_or("cli");
    if text.is_empty() {
        "cli".to_string()
    } else {
        text.to_string()
    }
}

fn run_list(
    home: Option<PathBuf>,
    args: DedupListArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let items = lam_consolidate::scan_project(&conn, &project.id, now)?;
    let _ = items;
    let list = lam_consolidate::list_suggestions(&conn, &project.id, !args.all)?;
    if as_json {
        let values: Vec<serde_json::Value> = list
            .iter()
            .map(|item| {
                serde_json::json!({
                    "suggestion_id": item.id,
                    "memory_a": item.memory_a_id,
                    "memory_b": item.memory_b_id,
                    "score": item.score,
                    "reason": item.reason,
                    "status": item.status,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&values).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for item in &list {
            println!(
                "{} [{}] {} <-> {} ({})",
                item.id, item.status, item.memory_a_id, item.memory_b_id, item.reason
            );
        }
    }
    Ok(())
}

fn run_resolve(
    home: Option<PathBuf>,
    args: DedupResolveArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let outcome = lam_consolidate::resolve_suggestion(
        &mut conn,
        &args.suggestion_id,
        args.keep.as_deref(),
        &actor_of(&args.actor),
        args.reason.as_deref(),
        now,
    )?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "suggestion_id": args.suggestion_id.trim(),
                "result": outcome,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("dedup {}: {outcome}", args.suggestion_id.trim());
    }
    Ok(())
}
