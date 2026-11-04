use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum ProjectAction {
    List(ListArgs),
    Bind(BindArgs),
    Policy(PolicyArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {}

#[derive(Debug, Args)]
pub struct BindArgs {
    #[arg(long)]
    pub path: PathBuf,
    #[arg(long)]
    pub name: String,
}

#[derive(Debug, Args)]
pub struct PolicyArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub capture: Option<String>,
    #[arg(long)]
    pub deny: Vec<String>,
    #[arg(long)]
    pub clear_deny: bool,
    #[arg(long)]
    pub sensitivity: Option<String>,
}

pub fn run(
    home: Option<PathBuf>,
    action: ProjectAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        ProjectAction::List(_) => run_list(home, as_json),
        ProjectAction::Bind(args) => run_bind(home, args, as_json),
        ProjectAction::Policy(args) => run_policy(home, args, as_json),
    }
}

fn run_list(home: Option<PathBuf>, as_json: bool) -> Result<(), lam_core::LamError> {
    let (conn, _) = crate::commands::open_existing(&home)?;
    let projects = lam_storage::list_projects(&conn)?;
    if as_json {
        let items: Vec<serde_json::Value> = projects
            .iter()
            .map(|project| {
                serde_json::json!({
                    "id": project.id.to_string(),
                    "name": project.name,
                    "canonical_root": project.canonical_root.to_string_lossy(),
                    "identity_hash": project.identity_hash,
                    "created_at": project.created_at.to_rfc3339(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for project in &projects {
            println!(
                "{} {} {}",
                project.id,
                project.name,
                project.canonical_root.to_string_lossy()
            );
        }
    }
    Ok(())
}

fn run_bind(
    home: Option<PathBuf>,
    args: BindArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let name = args.name.trim();
    if name.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_PROJECT_NAME_INVALID",
            "project name must not be empty".to_string(),
        ));
    }
    let (paths, _) = crate::commands::init::ensure_workspace(home)?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_or_create(&mut conn, &args.path, Some(name), now)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": project.id.to_string(),
                "name": project.name,
                "canonical_root": project.canonical_root.to_string_lossy(),
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("bound {} {}", project.id, project.name);
    }
    Ok(())
}

fn run_policy(
    home: Option<PathBuf>,
    args: PolicyArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = lam_storage::resolve_ref(&mut conn, &args.project, now, true)?;
    let mut policy = lam_storage::get_policy(&conn, &project.id)?;
    if let Some(raw) = args.capture.as_deref() {
        policy.capture_enabled = match raw.trim().to_lowercase().as_str() {
            "on" | "true" | "1" => true,
            "off" | "false" | "0" => false,
            _ => {
                return Err(lam_core::LamError::validation(
                    "E_POLICY_CAPTURE_INVALID",
                    "capture must be on or off".to_string(),
                ));
            }
        };
    }
    if args.clear_deny {
        policy.deny_globs.clear();
    }
    for glob in &args.deny {
        let trimmed = glob.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !policy.deny_globs.iter().any(|existing| existing == trimmed) {
            policy.deny_globs.push(trimmed.to_string());
        }
    }
    if let Some(raw) = args.sensitivity.as_deref() {
        policy.sensitivity = raw.trim().to_lowercase();
    }
    lam_storage::set_policy(&conn, &policy)?;
    let policy = lam_storage::get_policy(&conn, &project.id)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "project": project.id.to_string(),
                "capture_enabled": policy.capture_enabled,
                "deny_globs": policy.deny_globs,
                "sensitivity": policy.sensitivity,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("project: {}", project.id);
        println!(
            "capture: {}",
            if policy.capture_enabled { "on" } else { "off" }
        );
        println!("deny: {}", policy.deny_globs.join(", "));
        println!("sensitivity: {}", policy.sensitivity);
    }
    Ok(())
}
