use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum SessionAction {
    Resume(ResumeArgs),
}

#[derive(Debug, Args)]
pub struct ResumeArgs {
    #[arg(long)]
    pub project: String,
}

pub fn run(
    home: Option<PathBuf>,
    action: SessionAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        SessionAction::Resume(args) => run_resume(home, args, as_json),
    }
}

fn run_resume(
    home: Option<PathBuf>,
    args: ResumeArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, false)?;
    let resume = lam_storage::load_resume(&conn, &project.id)?;
    if as_json {
        let document = serde_json::json!({
            "project_id": resume.project_id.to_string(),
            "session_id": resume.session_id.map(|id| id.to_string()),
            "external_session_id": resume.external_session_id,
            "working_state": resume.working_state.map(|state| {
                serde_json::json!({
                    "task_key": state.task_key,
                    "state": serde_json::from_str::<serde_json::Value>(&state.state_json)
                        .unwrap_or(serde_json::Value::Null),
                    "updated_at": state.updated_at,
                })
            }),
            "last_episode": resume.last_episode.map(|episode| {
                serde_json::json!({
                    "id": episode.id,
                    "abstract": episode.abstract_text,
                    "summary": episode.summary,
                    "topics": episode.topics,
                })
            }),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".to_string())
        );
    } else if resume.session_id.is_none() {
        println!("no sessions recorded for project {}", project.id);
    } else {
        if let Some(state) = resume.working_state {
            println!("working state ({})", state.updated_at);
            println!("{}", state.state_json);
        }
        if let Some(episode) = resume.last_episode {
            println!("last episode [{}]", episode.topics);
            println!("{}", episode.summary);
        }
    }
    Ok(())
}
