use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

const AUTO: &str = "auto";
const ALL: &str = "all";

#[derive(Debug, Subcommand)]
pub enum IntegrationAction {
    Install(InstallArgs),
    Uninstall(InstallArgs),
    Status(StatusArgs),
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    #[arg(default_value = AUTO)]
    pub host: String,
    #[arg(long)]
    pub host_home: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    pub host: Option<String>,
    #[arg(long)]
    pub host_home: Option<PathBuf>,
}

pub fn run(
    home: Option<PathBuf>,
    action: IntegrationAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        IntegrationAction::Install(args) => run_install(home, args, as_json),
        IntegrationAction::Uninstall(args) => run_uninstall(home, args, as_json),
        IntegrationAction::Status(args) => run_status(home, args, as_json),
    }
}

fn parse_host(raw: &str) -> Result<lam_adapters::Host, lam_core::LamError> {
    raw.trim().parse::<lam_adapters::Host>()
}

fn host_home_of(flag: &Option<PathBuf>) -> Result<PathBuf, lam_core::LamError> {
    if let Some(dir) = flag.clone() {
        return Ok(dir);
    }
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        lam_core::LamError::validation(
            "E_HOME_MISSING",
            "HOME is not set; pass --host-home".to_string(),
        )
    })
}

fn target_for(
    home: Option<PathBuf>,
    host_home: &Option<PathBuf>,
) -> Result<lam_adapters::Target, lam_core::LamError> {
    let binary = std::env::current_exe().map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let data_home = lam_config::resolve_paths(home).data_dir;
    Ok(lam_adapters::Target {
        home: host_home_of(host_home)?,
        binary,
        data_home,
    })
}

fn hermes_target_for(
    home: Option<PathBuf>,
    host_home: &Option<PathBuf>,
) -> Result<lam_adapters::Target, lam_core::LamError> {
    let binary = std::env::current_exe().map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let data_home = lam_config::resolve_paths(home).data_dir;
    let hermes_home = if let Some(dir) = host_home.clone() {
        dir
    } else if let Ok(raw) = std::env::var("HERMES_HOME") {
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            host_home_of(&None)?.join(".hermes")
        } else {
            PathBuf::from(trimmed)
        }
    } else {
        host_home_of(&None)?.join(".hermes")
    };
    Ok(lam_adapters::Target {
        home: hermes_home,
        binary,
        data_home,
    })
}

fn target_for_host(
    home: Option<PathBuf>,
    host: lam_adapters::Host,
    host_home: &Option<PathBuf>,
) -> Result<lam_adapters::Target, lam_core::LamError> {
    match host {
        lam_adapters::Host::Hermes => hermes_target_for(home, host_home),
        _ => target_for(home, host_home),
    }
}

fn detected_hosts(host_home: &std::path::Path) -> Vec<lam_adapters::DetectedHost> {
    lam_adapters::install::detect_hosts_from_env(host_home)
}

fn parse_host_list(raw: &str) -> Result<Vec<lam_adapters::Host>, lam_core::LamError> {
    let mut hosts = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case(ALL) {
            for host in lam_adapters::Host::ALL {
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
            continue;
        }
        let host = parse_host(trimmed)?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    if hosts.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_HOST_INVALID",
            "no host selected".to_string(),
        ));
    }
    Ok(hosts)
}

fn selection(
    raw: &str,
    host_home: &std::path::Path,
) -> Result<(Vec<lam_adapters::Host>, bool), lam_core::LamError> {
    if raw.trim().eq_ignore_ascii_case(AUTO) {
        let hosts = detected_hosts(host_home)
            .iter()
            .map(|item| item.host)
            .collect();
        return Ok((hosts, true));
    }
    Ok((parse_host_list(raw)?, false))
}

fn install_host(
    host: lam_adapters::Host,
    target: &lam_adapters::Target,
) -> Result<lam_adapters::InstallReport, lam_core::LamError> {
    match host {
        lam_adapters::Host::Codex => lam_adapters::install::install_codex(target),
        lam_adapters::Host::ClaudeCode => lam_adapters::install::install_claude(target),
        lam_adapters::Host::OpenCode => lam_adapters::install::install_opencode(target),
        lam_adapters::Host::Hermes => lam_adapters::install::install_hermes(target),
    }
}

fn uninstall_host(
    host: lam_adapters::Host,
    target: &lam_adapters::Target,
) -> Result<lam_adapters::InstallReport, lam_core::LamError> {
    match host {
        lam_adapters::Host::Codex => lam_adapters::install::uninstall_codex(target),
        lam_adapters::Host::ClaudeCode => lam_adapters::install::uninstall_claude(target),
        lam_adapters::Host::OpenCode => lam_adapters::install::uninstall_opencode(target),
        lam_adapters::Host::Hermes => lam_adapters::install::uninstall_hermes(target),
    }
}

fn status_host(
    host: lam_adapters::Host,
    target: &lam_adapters::Target,
) -> Result<lam_adapters::HostStatus, lam_core::LamError> {
    match host {
        lam_adapters::Host::Codex => lam_adapters::install::status_codex(target),
        lam_adapters::Host::ClaudeCode => lam_adapters::install::status_claude(target),
        lam_adapters::Host::OpenCode => lam_adapters::install::status_opencode(target),
        lam_adapters::Host::Hermes => lam_adapters::install::status_hermes(target),
    }
}

fn report_row(report: &lam_adapters::InstallReport) -> serde_json::Value {
    serde_json::json!({
        "host": report.host,
        "changed": report.changed,
        "files_written": report.files_written,
        "backups": report.backups,
    })
}

fn print_reports(
    rows: &[serde_json::Value],
    as_json: bool,
    as_array: bool,
    verb: &str,
    absent_verb: &str,
) {
    if as_json {
        if as_array {
            println!(
                "{}",
                serde_json::to_string_pretty(rows).unwrap_or_else(|_| "[]".to_string())
            );
        } else if let Some(row) = rows.first() {
            println!(
                "{}",
                serde_json::to_string_pretty(row).unwrap_or_else(|_| "{}".to_string())
            );
        }
        return;
    }
    for row in rows {
        let host = row.get("host").and_then(|v| v.as_str()).unwrap_or("?");
        if let Some(message) = row.get("message").and_then(|v| v.as_str()) {
            let code = row
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("E_UNKNOWN");
            println!("{host}: integration failed [{code}]: {message}");
            continue;
        }
        let changed = row
            .get("changed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        println!(
            "{host}: integration {}",
            if changed { verb } else { absent_verb }
        );
    }
}

fn run_install(
    home: Option<PathBuf>,
    args: InstallArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let host_home = host_home_of(&args.host_home)?;
    let (hosts, auto) = selection(&args.host, &host_home)?;
    if hosts.is_empty() {
        if as_json {
            println!("[]");
        } else {
            println!("no agent hosts detected (looked for codex, claude-code, opencode, hermes)");
        }
        return Ok(());
    }
    let as_array = auto || hosts.len() > 1;
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for host in &hosts {
        let target = target_for_host(home.clone(), *host, &args.host_home)?;
        match install_host(*host, &target) {
            Ok(report) => rows.push(report_row(&report)),
            Err(e) if auto => rows.push(serde_json::json!({
                "host": host.as_str(),
                "changed": false,
                "error": e.code(),
                "message": e.to_string(),
            })),
            Err(e) => return Err(e),
        }
    }
    print_reports(&rows, as_json, as_array, "installed", "already installed");
    Ok(())
}

fn installed_hosts(
    home: Option<PathBuf>,
    host_home: &Option<PathBuf>,
) -> Result<Vec<lam_adapters::Host>, lam_core::LamError> {
    let mut hosts = Vec::new();
    for host in lam_adapters::Host::ALL {
        let target = target_for_host(home.clone(), host, host_home)?;
        let status = match status_host(host, &target) {
            Ok(status) => status,
            Err(e) => {
                eprintln!("warning: cannot inspect {}: {e}", host.as_str());
                continue;
            }
        };
        if status.hooks_installed || status.mcp_installed {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

fn run_uninstall(
    home: Option<PathBuf>,
    args: InstallArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (hosts, auto) = if args.host.trim().eq_ignore_ascii_case(AUTO) {
        (installed_hosts(home.clone(), &args.host_home)?, true)
    } else {
        (parse_host_list(&args.host)?, false)
    };
    if hosts.is_empty() {
        if as_json {
            println!("[]");
        } else {
            println!("no agent-memory integrations found");
        }
        return Ok(());
    }
    let as_array = auto || hosts.len() > 1;
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for host in &hosts {
        let target = target_for_host(home.clone(), *host, &args.host_home)?;
        match uninstall_host(*host, &target) {
            Ok(report) => rows.push(report_row(&report)),
            Err(e) if auto => rows.push(serde_json::json!({
                "host": host.as_str(),
                "changed": false,
                "error": e.code(),
                "message": e.to_string(),
            })),
            Err(e) => return Err(e),
        }
    }
    print_reports(&rows, as_json, as_array, "removed", "already absent");
    Ok(())
}

fn run_status(
    home: Option<PathBuf>,
    args: StatusArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let host_home = host_home_of(&args.host_home)?;
    let detected: Vec<lam_adapters::Host> = detected_hosts(&host_home)
        .iter()
        .map(|item| item.host)
        .collect();
    let wanted: Vec<lam_adapters::Host> = match args.host {
        Some(raw) if raw.trim().eq_ignore_ascii_case(AUTO) => detected.clone(),
        Some(raw) => {
            let host = parse_host(&raw)?;
            vec![host]
        }
        None => lam_adapters::Host::ALL.to_vec(),
    };
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for host in wanted {
        let target = target_for_host(home.clone(), host, &args.host_home)?;
        let status = status_host(host, &target)?;
        rows.push(serde_json::json!({
            "host": status.host,
            "detected": detected.contains(&host),
            "hooks_installed": status.hooks_installed,
            "mcp_installed": status.mcp_installed,
        }));
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
        );
    } else {
        for row in &rows {
            let host = row.get("host").and_then(|v| v.as_str()).unwrap_or("?");
            let is_detected = row
                .get("detected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let hooks = row
                .get("hooks_installed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mcp = row
                .get("mcp_installed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            println!(
                "{host}{}: hooks={}, mcp={}",
                if is_detected { " (detected)" } else { "" },
                if hooks { "yes" } else { "no" },
                if mcp { "yes" } else { "no" }
            );
        }
    }
    Ok(())
}
