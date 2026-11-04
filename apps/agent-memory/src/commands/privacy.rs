use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum PrivacyAction {
    Audit(PrivacyAuditArgs),
    Scrub(PrivacyScrubArgs),
}

#[derive(Debug, Args)]
pub struct PrivacyAuditArgs {
    #[arg(long)]
    pub path: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PrivacyScrubArgs {
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

pub fn run(
    home: Option<PathBuf>,
    action: PrivacyAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        PrivacyAction::Audit(args) => run_audit(home, args, as_json),
        PrivacyAction::Scrub(args) => run_scrub(home, args, as_json),
    }
}

fn run_scrub(
    home: Option<PathBuf>,
    args: PrivacyScrubArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (_, paths) = crate::commands::open_existing(&home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let report = lam_storage::scrub_stored_events(&mut conn, args.dry_run)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dry_run": args.dry_run,
                "scanned": report.scanned,
                "changed": report.changed,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else if args.dry_run {
        println!(
            "dry run: {} stored events scanned, {} would be redacted",
            report.scanned, report.changed
        );
    } else {
        println!(
            "scrubbed {} stored events, {} redacted",
            report.scanned, report.changed
        );
    }
    Ok(())
}

fn scan_bytes(bytes: &[u8]) -> Vec<(usize, String)> {
    let text = String::from_utf8_lossy(bytes);
    let mut findings = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.len() > 1_048_576 {
            continue;
        }
        if let Some(kind) = lam_core::secrets::find_secret_kind(line) {
            findings.push((number + 1, kind.to_string()));
            if findings.len() >= 100 {
                break;
            }
        }
    }
    findings
}

fn collect_targets(paths: &lam_config::ResolvedPaths, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    targets.push(paths.db_file.clone());
    targets.push(paths.config_file.clone());
    for sidecar in lam_storage::sidecar_paths(&paths.db_file) {
        targets.push(sidecar);
    }
    for sub in ["backups", "schedule"] {
        let dir = paths.data_dir.join(sub);
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    targets.push(entry.path());
                }
            }
        }
    }
    targets.extend(extra.iter().cloned());
    targets.sort();
    targets.dedup();
    targets.into_iter().filter(|path| path.is_file()).collect()
}

fn run_audit(
    home: Option<PathBuf>,
    args: PrivacyAuditArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (_, paths) = crate::commands::open_existing(&home)?;
    let targets = collect_targets(&paths, &args.path);
    let mut files_scanned = 0;
    let mut files_with_findings = Vec::new();
    let mut total_findings = 0;
    for target in &targets {
        let bytes = match std::fs::read(target) {
            Ok(found) => found,
            Err(_) => continue,
        };
        files_scanned += 1;
        let findings = scan_bytes(&bytes);
        if !findings.is_empty() {
            total_findings += findings.len();
            files_with_findings.push(serde_json::json!({
                "file": target.to_string_lossy(),
                "finding_count": findings.len(),
                "findings": findings.iter().take(10).map(|(line, kind)| {
                    serde_json::json!({"line": line, "kind": kind})
                }).collect::<Vec<_>>(),
            }));
        }
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "files_scanned": files_scanned,
                "total_findings": total_findings,
                "files_with_findings": files_with_findings,
                "remote_provider": remote_summary(&paths),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("scanned {files_scanned} files, {total_findings} secret-like findings");
        for file in &files_with_findings {
            println!(
                "flagged {}",
                file.get("file").and_then(|v| v.as_str()).unwrap_or("?")
            );
        }
        let summary = remote_summary(&paths);
        println!(
            "remote extraction: {}",
            summary
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("none")
        );
    }
    if total_findings > 0 {
        return Err(lam_core::LamError::validation(
            "E_PRIVACY_FINDINGS",
            format!("{total_findings} secret-like findings in workspace artifacts"),
        ));
    }
    Ok(())
}

fn remote_summary(paths: &lam_config::ResolvedPaths) -> serde_json::Value {
    std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|text| lam_config::Config::from_toml_str(&text).ok())
        .map(|config| {
            serde_json::json!({
                "provider": config.models.extraction.provider,
                "model": config.models.extraction.model,
                "allow_remote": config.models.extraction.allow_remote,
                "api_key_env": config.models.extraction.api_key_env,
                "enabled": config.models.extraction.provider == "openai-compatible",
            })
        })
        .unwrap_or(serde_json::json!({"enabled": false, "detail": "config unreadable"}))
}
