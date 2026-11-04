use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct SearchArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub query: String,
    #[arg(long, default_value_t = 5)]
    pub limit: u32,
}

pub fn run(
    home: Option<PathBuf>,
    args: SearchArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let limit = (args.limit as usize).clamp(1, 20);
    let grouped = lam_retrieval::multi_search(&conn, &project.id, &args.query, limit, now)?;
    if as_json {
        let render = |items: &[lam_retrieval::UnifiedHit]| {
            items
                .iter()
                .map(|hit| {
                    serde_json::json!({
                        "id": hit.id,
                        "kind": hit.kind.as_str(),
                        "title": hit.title,
                        "snippet": hit.snippet,
                        "source": hit.source_ref,
                        "rank": hit.rank,
                    })
                })
                .collect::<Vec<_>>()
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "memories": render(&grouped.memories),
                "episodes": render(&grouped.episodes),
                "vault": render(&grouped.vault),
                "graph": render(&grouped.graph),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("== memories ==");
        for hit in &grouped.memories {
            println!("- {} ({})", hit.title, hit.source_ref);
        }
        println!("== episodes ==");
        for hit in &grouped.episodes {
            println!("- {} ({})", hit.title, hit.source_ref);
        }
        println!("== vault ==");
        for hit in &grouped.vault {
            println!("- {} ({})", hit.title, hit.source_ref);
        }
        println!("== graph ==");
        for hit in &grouped.graph {
            println!("- {} ({})", hit.title, hit.source_ref);
        }
    }
    Ok(())
}
