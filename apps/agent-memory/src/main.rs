use clap::Parser;
use clap::Subcommand;
use std::path::PathBuf;

mod commands;

#[derive(Debug, Parser)]
#[command(name = "agent-memory", version, about = "Local Agent Memory")]
struct Cli {
    #[arg(long, global = true, env = "AGENT_MEMORY_HOME")]
    home: Option<PathBuf>,
    #[arg(long, global = true, default_value_t = false)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init,
    Status,
    Doctor(commands::doctor::DoctorArgs),
    Mcp,
    Benchmark,
    Backup {
        #[command(subcommand)]
        action: Box<commands::backup::BackupAction>,
    },
    Privacy {
        #[command(subcommand)]
        action: Box<commands::privacy::PrivacyAction>,
    },
    Index {
        #[command(subcommand)]
        action: Box<commands::index::IndexAction>,
    },
    Hook {
        #[command(subcommand)]
        action: Box<commands::hook::HookAction>,
    },
    Session {
        #[command(subcommand)]
        action: Box<commands::session::SessionAction>,
    },
    Providers {
        #[command(subcommand)]
        action: Box<commands::providers::ProvidersAction>,
    },
    Integration {
        #[command(subcommand)]
        action: Box<commands::integration::IntegrationAction>,
    },
    Project {
        #[command(subcommand)]
        action: commands::project::ProjectAction,
    },
    Memory {
        #[command(subcommand)]
        action: Box<commands::memory::MemoryAction>,
    },
    Vault {
        #[command(subcommand)]
        action: Box<commands::vault::VaultAction>,
    },
    Maintain(commands::maintain::MaintainArgs),
    Schedule {
        #[command(subcommand)]
        action: commands::maintain::ScheduleAction,
    },
    Search(commands::search::SearchArgs),
    Export(commands::transfer::ExportArgs),
    Import(commands::transfer::ImportArgs),
}

fn main() {
    let cli = Cli::parse();
    let code = run(cli);
    std::process::exit(code);
}

fn run(cli: Cli) -> i32 {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let result = match cli.command {
        Commands::Init => commands::init::run(cli.home, cli.json),
        Commands::Status => commands::status::run(cli.home, cli.json),
        Commands::Doctor(args) => commands::doctor::run(cli.home, args, cli.json),
        Commands::Backup { action } => commands::backup::run(cli.home, *action, cli.json),
        Commands::Privacy { action } => commands::privacy::run(cli.home, *action, cli.json),
        Commands::Index { action } => commands::index::run(cli.home, *action, cli.json),
        Commands::Mcp => commands::mcp::run(cli.home),
        Commands::Benchmark => commands::benchmark::run(cli.home, cli.json),
        Commands::Hook { action } => commands::hook::run(cli.home, *action, cli.json),
        Commands::Session { action } => commands::session::run(cli.home, *action, cli.json),
        Commands::Providers { action } => commands::providers::run(cli.home, *action, cli.json),
        Commands::Integration { action } => commands::integration::run(cli.home, *action, cli.json),
        Commands::Project { action } => commands::project::run(cli.home, action, cli.json),
        Commands::Memory { action } => commands::memory::run(cli.home, *action, cli.json),
        Commands::Vault { action } => commands::vault::run(cli.home, *action, cli.json),
        Commands::Export(args) => commands::transfer::run_export(cli.home, args, cli.json),
        Commands::Import(args) => commands::transfer::run_import(cli.home, args, cli.json),
        Commands::Maintain(args) => commands::maintain::run_maintain(cli.home, args, cli.json),
        Commands::Schedule { action } => {
            commands::maintain::run_schedule(cli.home, action, cli.json)
        }
        Commands::Search(args) => commands::search::run(cli.home, args, cli.json),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            if cli.json {
                let body = e.to_json();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                eprintln!("error [{}]: {}", e.code(), e);
            }
            1
        }
    }
}
