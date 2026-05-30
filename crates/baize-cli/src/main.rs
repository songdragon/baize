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
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Status {
        #[arg(default_value = ".")]
        path: String,
    },
    Doctor,
    Providers,
    Validate {
        provider: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Path,
    Show,
    Init {
        #[arg(long)]
        force: bool,
    },
    Validate,
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
        Command::Config { command } => handle_config(command),
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
        Command::Validate { provider } => {
            let providers = baize_adapters::default_provider_profiles();
            if let Some(provider) = provider {
                let Some(profile) = providers.iter().find(|profile| profile.id.0 == provider)
                else {
                    anyhow::bail!("unknown provider: {provider}");
                };
                let validation = baize_adapters::validate_provider(profile);
                println!("{}", serde_json::to_string_pretty(&validation)?);
            } else {
                let validations = baize_adapters::validate_all_providers();
                println!("{}", serde_json::to_string_pretty(&validations)?);
            }
            Ok(())
        }
    }
}

fn handle_config(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Path => {
            println!("{}", baize_config::default_config_path().display());
            Ok(())
        }
        ConfigCommand::Show => {
            let config = baize_config::load_or_default()?;
            println!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
        ConfigCommand::Init { force } => {
            let path = baize_config::init_default_config(force)?;
            println!("created {}", path.display());
            Ok(())
        }
        ConfigCommand::Validate => {
            let config = baize_config::load_or_default()?;
            let validation = baize_config::validate_config(&config);
            println!("{}", serde_json::to_string_pretty(&validation)?);
            if !validation.valid {
                anyhow::bail!("config validation failed");
            }
            Ok(())
        }
    }
}
