use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "baize")]
#[command(about = "Workspace-native agent supervisor")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Tui,
    Daemon,
    Status {
        #[arg(default_value = ".")]
        path: String,
    },
    Doctor,
    Providers,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => baize_tui::run(),
        Command::Daemon => {
            let config = baize_config::load_or_default()?;
            baize_daemon::run(config).await
        }
        Command::Status { path } => {
            let status = baize_workspace::inspect(path)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Command::Doctor => {
            let providers = baize_adapters::default_provider_profiles();
            let health = providers
                .iter()
                .map(baize_adapters::check_provider)
                .collect::<Vec<_>>();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "providers": health }))?
            );
            Ok(())
        }
        Command::Providers => {
            let providers = baize_adapters::default_provider_profiles();
            println!("{}", serde_json::to_string_pretty(&providers)?);
            Ok(())
        }
    }
}
