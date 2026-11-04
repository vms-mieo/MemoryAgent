use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct MaintainArgs {
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, default_value_t = 30_000)]
    pub budget_ms: u64,
    #[arg(long)]
    pub kind: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum ScheduleAction {
    Install(ScheduleInstallArgs),
    Uninstall(ScheduleUndoArgs),
    Status(ScheduleUndoArgs),
}

#[derive(Debug, Args)]
pub struct ScheduleInstallArgs {
    #[arg(long, default_value = "launchd")]
    pub kind: String,
    #[arg(long, default_value_t = 60)]
    pub interval_minutes: u64,
}

#[derive(Debug, Args)]
pub struct ScheduleUndoArgs {
    #[arg(long, default_value = "launchd")]
    pub kind: String,
}

pub fn run_maintain(
    home: Option<PathBuf>,
    args: MaintainArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, paths) = crate::commands::open_existing(&home)?;
    let config_text = std::fs::read_to_string(&paths.config_file)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let config = lam_config::Config::from_toml_str(&config_text)?;
    if let Some(kind) = &args.kind {
        kind.parse::<lam_storage::JobKind>()?;
    }
    let now = chrono::Utc::now();
    seed_periodic_jobs(&mut conn, &config, args.project.as_deref(), now)?;
    let ctx = pipeline_context(&config);
    let outcome = lam_consolidate::maintain(
        &mut conn,
        "cli-maintain",
        args.budget_ms.max(1),
        args.kind.as_deref(),
        &|conn, job| dispatch_job(conn, job, &ctx, args.project.as_deref()),
    );
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "processed": outcome.processed,
                "succeeded": outcome.succeeded,
                "failed": outcome.failed,
                "deferred": outcome.deferred,
                "budget_exceeded": outcome.budget_exceeded,
                "elapsed_ms": outcome.elapsed_ms,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "maintain processed={} succeeded={} failed={} deferred={} budget_exceeded={} elapsed_ms={}",
            outcome.processed,
            outcome.succeeded,
            outcome.failed,
            outcome.deferred,
            outcome.budget_exceeded,
            outcome.elapsed_ms
        );
    }
    Ok(())
}

pub(crate) fn pipeline_context(config: &lam_config::Config) -> lam_consolidate::PipelineContext {
    let mut ctx = lam_consolidate::PipelineContext::deterministic(config.memory.episodic_ttl_days)
        .with_admission(&config.admission.mode, config.admission.minimum_confidence);
    if config.models.extraction.provider.trim() != "none" {
        ctx.extractor = Some(lam_consolidate::ExtractorChoice {
            provider: config.models.extraction.provider.clone(),
            model: config.models.extraction.model.clone(),
            endpoint: config.models.extraction.endpoint.clone(),
            api_key_env: config.models.extraction.api_key_env.clone(),
            allow_remote: config.models.extraction.allow_remote,
            timeout_ms: config.models.extraction.timeout_ms.clamp(500, 300_000),
            session_header: config.models.extraction.session_header.clone(),
        });
    }
    ctx
}

fn seed_periodic_jobs(
    conn: &mut rusqlite::Connection,
    config: &lam_config::Config,
    project_ref: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), lam_core::LamError> {
    let _ = lam_storage::enqueue(conn, "vacuum_cleanup", "global", 0, now);
    let scope_project = match project_ref {
        Some(reference) => {
            let project = lam_storage::resolve_ref(&mut *conn, reference, now, false)?;
            Some(project.id)
        }
        None => None,
    };
    if let Some(project_id) = scope_project {
        let input_ref = format!("project:{}", project_id.to_string_value());
        let _ = lam_storage::enqueue(conn, "dedup_scan", &input_ref, 0, now);
        if config.dreaming.enabled {
            let clusters = lam_consolidate::find_eligible_clusters(
                conn,
                config.dreaming.min_new_episodes.max(2) as usize,
                config.dreaming.min_idle_minutes as i64,
                now,
            )?;
            for cluster in clusters
                .iter()
                .filter(|c| c.project_id == project_id)
                .take(8)
            {
                let input = format!(
                    "dream:{}:{}:{}",
                    project_id.to_string_value(),
                    cluster.topic,
                    cluster.max_rowid
                );
                let _ = lam_storage::enqueue(conn, "dream_batch", &input, 0, now);
            }
        }
    }
    Ok(())
}

fn project_for_session(
    conn: &rusqlite::Connection,
    session_ref: &str,
) -> Result<Option<lam_core::ProjectId>, lam_core::LamError> {
    let row: Option<String> = conn
        .query_row(
            "SELECT project_id FROM sessions WHERE external_session_id = ?1 OR id = ?1",
            rusqlite::params![session_ref],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match row {
        Some(text) => Ok(Some(text.parse::<lam_core::ProjectId>().map_err(|_| {
            lam_core::LamError::storage("stored project id invalid".to_string())
        })?)),
        None => Ok(None),
    }
}

pub(crate) fn dispatch_job(
    conn: &mut rusqlite::Connection,
    job: &lam_storage::Job,
    ctx: &lam_consolidate::PipelineContext,
    project_ref: Option<&str>,
) -> Result<String, lam_core::LamError> {
    let now = chrono::Utc::now();
    match job.kind.as_str() {
        "episode_summarize" => lam_consolidate::handle_episode_summarize(conn, job, ctx, now),
        "fact_extract" => {
            let session_ref = job
                .input_ref
                .strip_prefix("session:")
                .unwrap_or(job.input_ref.as_str());
            let project_id = match project_for_session(conn, session_ref)? {
                Some(found) => found,
                None => match project_ref {
                    Some(reference) => lam_storage::resolve_ref(conn, reference, now, false)?.id,
                    None => {
                        return Err(lam_core::LamError::validation(
                            "E_JOB_PROJECT_UNKNOWN",
                            format!("no session {} and no --project given", session_ref),
                        ));
                    }
                },
            };
            let outcome =
                lam_consolidate::handle_fact_extract(conn, job, ctx, now, &project_id, "maintain")?;
            Ok(format!(
                "candidates={} asserted={} deferred={}",
                outcome.candidates, outcome.asserted, outcome.deferred
            ))
        }
        "dedup_scan" => {
            let project_id = match project_ref {
                Some(reference) => lam_storage::resolve_ref(conn, reference, now, false)?.id,
                None => {
                    return Err(lam_core::LamError::validation(
                        "E_JOB_PROJECT_UNKNOWN",
                        "dedup_scan needs --project".to_string(),
                    ));
                }
            };
            let created = lam_consolidate::scan_project(conn, &project_id, now)?;
            Ok(format!("suggestions={created}"))
        }
        "dream_batch" => dispatch_dream(conn, job, now),
        "vacuum_cleanup" => dispatch_vacuum(conn, now),
        "reindex" => dispatch_reindex(conn),
        other => Err(lam_core::LamError::validation(
            "E_JOB_DEFERRED",
            format!("job kind {other} is planned for a later phase; left pending"),
        )),
    }
}

fn dispatch_dream(
    conn: &mut rusqlite::Connection,
    job: &lam_storage::Job,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, lam_core::LamError> {
    let tail = job.input_ref.strip_prefix("dream:").unwrap_or("");
    let mut parts = tail.rsplitn(2, ':');
    let _max_text = parts.next().unwrap_or("");
    let head = parts.next().unwrap_or("");
    let mut head_parts = head.splitn(2, ':');
    let project_text = head_parts.next().unwrap_or("");
    let topic = head_parts.next().unwrap_or("");
    if project_text.is_empty() || topic.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_JOB_REF_INVALID",
            format!("malformed dream_batch ref: {}", job.input_ref),
        ));
    }
    let project_id: lam_core::ProjectId = project_text.parse().map_err(|_| {
        lam_core::LamError::validation(
            "E_JOB_REF_INVALID",
            format!("bad project in dream_batch ref: {}", job.input_ref),
        )
    })?;
    let clusters = lam_consolidate::find_eligible_clusters(conn, 2, 0, now)?;
    let cluster = clusters
        .iter()
        .find(|c| c.project_id == project_id && c.topic == topic)
        .cloned();
    let Some(cluster) = cluster else {
        return Ok("stale cluster; nothing to dream".to_string());
    };
    match lam_consolidate::synthesize(conn, &project_id, &cluster, now) {
        Ok(outcome) => Ok(format!(
            "dream {} supports={} {}",
            outcome.status,
            outcome.support_links.len(),
            outcome.reason
        )),
        Err(e) if e.code() == "E_DREAM_DEFERRED" => Ok(format!("deferred: {e}")),
        Err(e) => Err(e),
    }
}

fn dispatch_vacuum(
    conn: &mut rusqlite::Connection,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, lam_core::LamError> {
    let expired: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE expires_at < ?1",
            rusqlite::params![now.to_rfc3339()],
            |row| row.get(0),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "DELETE FROM episode_fts WHERE episode_id IN (SELECT id FROM episodes WHERE expires_at < ?1)",
        rusqlite::params![now.to_rfc3339()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "DELETE FROM episodes WHERE expires_at < ?1",
        rusqlite::params![now.to_rfc3339()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let cutoff = now - chrono::Duration::days(7);
    conn.execute(
        "DELETE FROM maintenance_jobs WHERE state IN ('done', 'cancelled', 'dead') AND due_at < ?1",
        rusqlite::params![cutoff.to_rfc3339()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    lam_storage::harden_current_db(conn);
    Ok(format!("vacuumed episodes={expired}"))
}

fn dispatch_reindex(conn: &mut rusqlite::Connection) -> Result<String, lam_core::LamError> {
    conn.execute("DELETE FROM episode_fts", [])
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO episode_fts (episode_id, abstract, summary, topics)
         SELECT id, abstract, summary, topics FROM episodes",
        [],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    lam_storage::harden_current_db(conn);
    Ok(format!("reindexed episodes={count}"))
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

pub fn run_schedule(
    home: Option<PathBuf>,
    action: ScheduleAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let paths = lam_config::resolve_paths(home);
    match action {
        ScheduleAction::Install(args) => install_schedule(&paths, &args, as_json),
        ScheduleAction::Uninstall(args) => uninstall_schedule(&paths, &args.kind, as_json),
        ScheduleAction::Status(args) => schedule_status(&paths, &args.kind, as_json),
    }
}

fn schedule_dir(paths: &lam_config::ResolvedPaths) -> PathBuf {
    paths.data_dir.join("schedule")
}

fn schedule_files(paths: &lam_config::ResolvedPaths, kind: &str) -> Vec<PathBuf> {
    let dir = schedule_dir(paths);
    match kind.trim().to_lowercase().as_str() {
        "launchd" => vec![dir.join("com.lam.agent-memory.plist")],
        "systemd" => vec![
            dir.join("lam-maintain.service"),
            dir.join("lam-maintain.timer"),
        ],
        _ => vec![dir.join("lam-maintain.cron")],
    }
}

fn install_schedule(
    paths: &lam_config::ResolvedPaths,
    args: &ScheduleInstallArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let kind = args.kind.trim().to_lowercase();
    if !["launchd", "systemd", "cron"].contains(&kind.as_str()) {
        return Err(lam_core::LamError::validation(
            "E_SCHEDULE_KIND_INVALID",
            "schedule kind must be launchd, systemd or cron".to_string(),
        ));
    }
    let minutes = args.interval_minutes.clamp(5, 1440);
    let exe = std::env::current_exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "agent-memory".to_string());
    let dir = schedule_dir(paths);
    std::fs::create_dir_all(&dir).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let home_arg = paths.data_dir.to_string_lossy().to_string();
    let written = match kind.as_str() {
        "launchd" => {
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n<key>Label</key><string>com.lam.agent-memory</string>\n<key>ProgramArguments</key><array><string>{exe}</string><string>--home</string><string>{home_arg}</string><string>maintain</string><string>--budget-ms</string><string>30000</string></array>\n<key>StartInterval</key><integer>{}</integer>\n</dict>\n</plist>\n",
                minutes * 60
            );
            let target = dir.join("com.lam.agent-memory.plist");
            std::fs::write(&target, body).map_err(|e| lam_core::LamError::io(e.to_string()))?;
            vec![target]
        }
        "systemd" => {
            let service = format!(
                "[Unit]\nDescription=Local agent memory maintenance\n[Service]\nType=oneshot\nExecStart={exe} --home {home_arg} maintain --budget-ms 30000\n"
            );
            let timer = format!(
                "[Unit]\nDescription=Run local agent memory maintenance\n[Timer]\nOnUnitActiveSec={minutes}min\nOnBootSec={minutes}min\n[Install]\nWantedBy=timers.target\n"
            );
            let service_path = dir.join("lam-maintain.service");
            let timer_path = dir.join("lam-maintain.timer");
            std::fs::write(&service_path, service)
                .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            std::fs::write(&timer_path, timer)
                .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            vec![service_path, timer_path]
        }
        _ => {
            let line =
                format!("*/{minutes} * * * * {exe} --home {home_arg} maintain --budget-ms 30000\n");
            let target = dir.join("lam-maintain.cron");
            std::fs::write(&target, &line).map_err(|e| lam_core::LamError::io(e.to_string()))?;
            vec![target]
        }
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "kind": kind,
                "interval_minutes": minutes,
                "files": written,
                "note": "opt-in schedule files under workspace; copy to LaunchAgents/systemd user dir or crontab to activate",
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        for file in &written {
            println!("wrote {}", file.display());
        }
        println!("opt-in only: copy the file to LaunchAgents/systemd user dir or crontab to activate; nothing was installed on this machine");
    }
    Ok(())
}

fn uninstall_schedule(
    paths: &lam_config::ResolvedPaths,
    kind: &str,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let mut removed = 0;
    for file in schedule_files(paths, kind) {
        if file.exists() {
            std::fs::remove_file(&file).map_err(|e| lam_core::LamError::io(e.to_string()))?;
            removed += 1;
        }
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "kind": kind.trim().to_lowercase(),
                "removed": removed,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("removed {removed} schedule file(s)");
    }
    Ok(())
}

fn schedule_status(
    paths: &lam_config::ResolvedPaths,
    kind: &str,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let files = schedule_files(paths, kind);
    let existing: Vec<PathBuf> = files.into_iter().filter(|path| path.exists()).collect();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "kind": kind.trim().to_lowercase(),
                "installed": !existing.is_empty(),
                "files": existing,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else if existing.is_empty() {
        println!("no {} schedule installed", kind.trim().to_lowercase());
    } else {
        for file in &existing {
            println!("installed: {}", file.display());
        }
    }
    Ok(())
}
