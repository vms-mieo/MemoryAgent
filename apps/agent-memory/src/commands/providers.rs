use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum ProvidersAction {
    Extract(ExtractArgs),
}

#[derive(Debug, Args)]
pub struct ExtractArgs {
    #[arg(long)]
    pub text: String,
    #[arg(long)]
    pub source_ref: String,
    #[arg(long, default_value_t = 15000)]
    pub timeout_ms: u64,
}

pub fn run(
    home: Option<PathBuf>,
    action: ProvidersAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        ProvidersAction::Extract(args) => run_extract(home, args, as_json),
    }
}

fn run_extract(
    home: Option<PathBuf>,
    args: ExtractArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (paths, config) = crate::commands::init::ensure_workspace(home)?;
    let _ = paths;
    let extractor = lam_providers::extractor_for(&config.models.extraction, args.timeout_ms)?;
    let session_id = lam_providers::stable_session_id("cli", &args.source_ref);
    let found = extractor.extract(&lam_providers::ExtractInput {
        text: args.text,
        source_ref: args.source_ref,
        session_id: Some(session_id),
    })?;
    if as_json {
        let items: Vec<serde_json::Value> = found
            .iter()
            .map(|candidate| {
                serde_json::json!({
                    "kind": candidate.kind,
                    "statement": candidate.statement,
                    "confidence": candidate.confidence,
                    "epistemic_status": candidate.epistemic_status,
                    "sources": candidate.sources,
                    "entities": candidate.entities,
                    "recommended_action": candidate.recommended_action,
                    "mapped_status": "proposed",
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for candidate in &found {
            println!(
                "- [{}] {} ({})",
                candidate.kind, candidate.statement, candidate.epistemic_status
            );
        }
        if found.is_empty() {
            eprintln!("no candidates: provider {}", extractor.name());
        }
    }
    Ok(())
}
