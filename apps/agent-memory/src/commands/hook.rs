use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Subcommand)]
pub enum HookAction {
    Ingest(IngestArgs),
    Context(ContextArgs),
}

#[derive(Debug, Args)]
pub struct IngestArgs {
    #[arg(long)]
    pub host: String,
    #[arg(long)]
    pub event: String,
    #[arg(long, default_value_t = 1500)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct ContextArgs {
    #[arg(long)]
    pub host: String,
    #[arg(long)]
    pub event: String,
    #[arg(long)]
    pub task: Option<String>,
    #[arg(long, default_value_t = 1500)]
    pub timeout_ms: u64,
}

pub fn run(
    home: Option<PathBuf>,
    action: HookAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        HookAction::Ingest(args) => run_ingest(home, args),
        HookAction::Context(args) => run_context(home, args, as_json),
    }
}

fn read_stdin_capped() -> Result<Vec<u8>, lam_core::LamError> {
    use std::io::Read;
    let mut bounded = std::io::stdin().take(1_048_577);
    let mut raw = Vec::new();
    bounded
        .read_to_end(&mut raw)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    if raw.len() > 1_048_576 {
        return Err(lam_core::LamError::validation(
            "E_HOOK_INPUT_TOO_LARGE",
            "hook input exceeds 1MiB".to_string(),
        ));
    }
    Ok(raw)
}

fn parse_host(raw: &str) -> Result<lam_adapters::Host, lam_core::LamError> {
    raw.trim().parse::<lam_adapters::Host>()
}

fn hook_budget_ms(raw: u64) -> u64 {
    raw.clamp(100, 10_000)
}

fn run_ingest(home: Option<PathBuf>, args: IngestArgs) -> Result<(), lam_core::LamError> {
    let outcome: Result<(), lam_core::LamError> = (|| {
        let host = parse_host(&args.host)?;
        let raw = read_stdin_capped()?;
        let input = lam_adapters::NativeInput::parse(host, &raw)?;
        if input.event != args.event.trim() {
            eprintln!(
                "lam hook ingest notice: flag event {} differs from input {}",
                args.event.trim(),
                input.event
            );
        }
        let now = chrono::Utc::now();
        let mut event = lam_adapters::normalize(&input, now)?;
        let (paths, _) = crate::commands::init::ensure_workspace_sized(home, Some(800), 8)?;
        ingest_with_paths(&paths, &mut event, now)
    })();
    match outcome {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("lam hook ingest deferred: [{}] {}", e.code(), e);
            Ok(())
        }
    }
}

fn ingest_with_paths(
    paths: &lam_config::ResolvedPaths,
    event: &mut lam_core::NormalizedEvent,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), lam_core::LamError> {
    let text = std::fs::read_to_string(&paths.config_file)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let config = lam_config::Config::from_toml_str(&text)?;
    let mut conn = lam_storage::open(&paths.db_file, config.storage.busy_timeout_ms.min(1500))?;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let step =
            lam_storage::ingest_normalized(&mut conn, event, config.memory.episodic_ttl_days, now);
        match step {
            Ok(outcome) => {
                enqueue_followups(&mut conn, event, &outcome, now);
                run_startup_maintenance(
                    &mut conn,
                    &config,
                    Duration::from_millis(config.maintenance.startup_budget_ms.min(500)),
                );
                break;
            }
            Err(e) if lam_storage::is_retryable_message(&e.to_string()) && attempt < 3 => {
                std::thread::sleep(Duration::from_millis(100 * attempt as u64));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn run_startup_maintenance(
    conn: &mut rusqlite::Connection,
    config: &lam_config::Config,
    budget: Duration,
) {
    if !config.maintenance.run_on_startup {
        return;
    }
    let cap = config.maintenance.startup_budget_ms.clamp(1, 5000);
    let budget_ms = (budget.as_millis() as u64).min(cap);
    if budget_ms < 5 {
        return;
    }
    let mut ctx = super::maintain::pipeline_context(config);
    ctx.remote_allowed = false;
    lam_consolidate::maintain(
        conn,
        "hook-startup",
        budget_ms.max(1),
        None,
        &|conn, job| super::maintain::dispatch_job(conn, job, &ctx, None),
    );
}

fn enqueue_followups(
    conn: &mut rusqlite::Connection,
    event: &lam_core::NormalizedEvent,
    outcome: &lam_storage::IngestOutcome,
    now: chrono::DateTime<chrono::Utc>,
) {
    if !outcome.stored {
        return;
    }
    let needs_extract = matches!(
        event.event_type.as_str(),
        "session.stop" | "session.end" | "session.checkpoint" | "session.idle"
    );
    if needs_extract {
        let input_ref = format!("session:{}", event.external_session_id);
        let _ = lam_storage::enqueue(conn, "fact_extract", &input_ref, 0, now);
    }
}

fn run_context(
    home: Option<PathBuf>,
    args: ContextArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let started = Instant::now();
    let budget = Duration::from_millis(hook_budget_ms(args.timeout_ms));
    let host = match parse_host(&args.host) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("lam hook context deferred: [{}] {}", e.code(), e);
            return Ok(());
        }
    };
    let raw = match read_stdin_capped() {
        Ok(found) => found,
        Err(e) => {
            eprintln!("lam hook context deferred: [{}] {}", e.code(), e);
            return Ok(());
        }
    };
    let outcome: Result<(), lam_core::LamError> = (|| {
        let input = lam_adapters::NativeInput::parse(host, &raw)?;
        if input.event != args.event.trim() {
            eprintln!(
                "lam hook context notice: flag event {} differs from input {}",
                args.event.trim(),
                input.event
            );
        }
        let now = chrono::Utc::now();
        let event = lam_adapters::normalize(&input, now)?;
        let (paths, config) = crate::commands::init::ensure_workspace_sized(home, Some(800), 8)?;
        if started.elapsed() >= budget {
            return Err(lam_core::LamError::validation(
                "E_HOOK_TIMEOUT",
                "hook budget exhausted before retrieval".to_string(),
            ));
        }
        let mut conn = lam_storage::open(&paths.db_file, config.storage.busy_timeout_ms.min(1500))?;
        run_startup_maintenance(
            &mut conn,
            &config,
            budget
                .checked_sub(started.elapsed())
                .unwrap_or(Duration::from_millis(0)),
        );
        let project = lam_storage::resolve_ref(&mut conn, &event.cwd, now, false)?;
        let task = args.task.clone().unwrap_or_else(|| context_task(&event));
        let allowance = config.recall.auto_inject_max_tokens.min(1000);
        let response = lam_storage::build_context(&conn, &project.id, &task, allowance)?;
        let mut response = response;
        append_resume(&conn, &project.id, &mut response);
        let rendered = lam_adapters::output::render_context(host, &response);
        if as_json {
            let document = match &rendered {
                lam_adapters::output::ContextOutput::Text(text) => {
                    serde_json::json!({"context_text": text})
                }
                lam_adapters::output::ContextOutput::Json(document) => {
                    serde_json::from_str(document).unwrap_or(serde_json::json!({}))
                }
            };
            println!(
                "{}",
                serde_json::to_string(&document).unwrap_or_else(|_| "{}".to_string())
            );
        } else {
            lam_adapters::output::emit(rendered);
        }
        Ok(())
    })();
    match outcome {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("lam hook context deferred: [{}] {}", e.code(), e);
            let fallback = lam_adapters::output::empty_output(host);
            if as_json {
                match fallback {
                    lam_adapters::output::ContextOutput::Text(text) => {
                        println!("{}", serde_json::json!({"context_text": text}));
                    }
                    lam_adapters::output::ContextOutput::Json(document) => {
                        println!("{document}");
                    }
                }
            } else {
                lam_adapters::output::emit(fallback);
            }
            Ok(())
        }
    }
}

fn append_resume(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    response: &mut lam_core::ContextResponse,
) {
    let resume = match lam_storage::load_resume(conn, project_id) {
        Ok(found) => found,
        Err(_) => return,
    };
    let mut lines: Vec<String> = Vec::new();
    if let Some(state) = resume.working_state {
        lines.push("Pending work from last checkpoint:".to_string());
        match serde_json::from_str::<serde_json::Value>(&state.state_json) {
            Ok(value) => {
                for key in ["goal", "current_task", "next_steps", "blockers"] {
                    if let Some(text) = resume_field(&value, key) {
                        if !text.trim().is_empty() {
                            lines.push(format!("{key}: {}", truncate_lines(&text, 400)));
                        }
                    }
                }
            }
            Err(_) => {
                lines.push(truncate_lines(&state.state_json, 400));
            }
        }
    }
    if let Some(episode) = resume.last_episode {
        lines.push(format!(
            "Last episode [{}]: {}",
            episode.topics,
            truncate_lines(&episode.summary, 400)
        ));
    }
    if !lines.is_empty() {
        response.context.push('\n');
        response.context.push_str(&lines.join("\n"));
        response.estimated_tokens = lam_core::estimate_tokens(&response.context);
    }
}

fn resume_field(value: &serde_json::Value, key: &str) -> Option<String> {
    match value.get(key)? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|entry| entry.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ),
        _ => None,
    }
}

fn truncate_lines(text: &str, limit: usize) -> String {
    if text.chars().count() > limit {
        text.chars().take(limit).collect()
    } else {
        text.to_string()
    }
}

fn context_task(event: &lam_core::NormalizedEvent) -> String {
    if event.event_type == "prompt.submit" {
        if let Some(excerpt) = event.payload.get("prompt_excerpt").and_then(|v| v.as_str()) {
            let text = excerpt.trim();
            if !text.is_empty() {
                return text.chars().take(500).collect();
            }
        }
    }
    lam_adapters::output::SESSION_START_TASK.to_string()
}
