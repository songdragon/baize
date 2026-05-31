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

enum CliAction {
    RunTui,
    RunDaemon,
    Print(String),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match plan_cli_action(cli.command)? {
        CliAction::RunTui => baize_tui::run(),
        CliAction::RunDaemon => {
            let config = baize_config::load_or_default()?;
            baize_daemon::run(config).await
        }
        CliAction::Print(output) => {
            print!("{output}");
            Ok(())
        }
    }
}

fn plan_cli_action(command: Option<Command>) -> Result<CliAction> {
    match command.unwrap_or(Command::Tui) {
        Command::Tui => Ok(CliAction::RunTui),
        Command::Daemon => Ok(CliAction::RunDaemon),
        Command::Config { command } => handle_config(command).map(CliAction::Print),
        Command::Status { path } => status_output(path).map(CliAction::Print),
        Command::Doctor => doctor_output().map(CliAction::Print),
        Command::Providers => providers_output().map(CliAction::Print),
        Command::Validate { provider } => validate_output(provider).map(CliAction::Print),
    }
}

fn status_output(path: String) -> Result<String> {
    let status = baize_workspace::inspect(path)?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&status)?))
}

fn doctor_output() -> Result<String> {
    let providers = baize_adapters::default_provider_profiles();
    let health = providers
        .iter()
        .map(baize_adapters::check_provider)
        .collect::<Vec<_>>();
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({ "providers": health }))?
    ))
}

fn providers_output() -> Result<String> {
    let providers = baize_adapters::default_provider_profiles();
    Ok(format!("{}\n", serde_json::to_string_pretty(&providers)?))
}

fn validate_output(provider: Option<String>) -> Result<String> {
    let providers = baize_adapters::default_provider_profiles();
    if let Some(provider) = provider {
        let Some(profile) = providers.iter().find(|profile| profile.id.0 == provider) else {
            anyhow::bail!("unknown provider: {provider}");
        };
        let validation = baize_adapters::validate_provider(profile);
        Ok(format!("{}\n", serde_json::to_string_pretty(&validation)?))
    } else {
        let validations = baize_adapters::validate_all_providers();
        Ok(format!("{}\n", serde_json::to_string_pretty(&validations)?))
    }
}

fn handle_config(command: ConfigCommand) -> Result<String> {
    match command {
        ConfigCommand::Path => Ok(format!(
            "{}\n",
            baize_config::default_config_path().display()
        )),
        ConfigCommand::Show => {
            let config = baize_config::load_or_default()?;
            Ok(format!("{}\n", toml::to_string_pretty(&config)?))
        }
        ConfigCommand::Init { force } => {
            let path = baize_config::init_default_config(force)?;
            Ok(format!("created {}\n", path.display()))
        }
        ConfigCommand::Validate => {
            let config = baize_config::load_or_default()?;
            let validation = baize_config::validate_config(&config);
            let output = format!("{}\n", serde_json::to_string_pretty(&validation)?);
            if !validation.valid {
                anyhow::bail!("config validation failed");
            }
            Ok(output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_to_tui_action() {
        let action = plan_cli_action(None).expect("action");

        assert!(matches!(action, CliAction::RunTui));
    }

    #[test]
    fn daemon_command_maps_to_daemon_action() {
        let action = plan_cli_action(Some(Command::Daemon)).expect("action");

        assert!(matches!(action, CliAction::RunDaemon));
    }

    #[test]
    fn config_path_prints_default_path() {
        let output = handle_config(ConfigCommand::Path).expect("config path");

        assert!(output.contains("baize"));
        assert!(output.ends_with('\n'));
    }

    #[test]
    fn providers_output_is_json_array() {
        let output = providers_output().expect("providers");
        let providers: serde_json::Value = serde_json::from_str(&output).expect("json");

        assert_eq!(providers[0]["id"], "codex");
        assert_eq!(providers[1]["id"], "gemini");
    }

    #[test]
    fn validate_unknown_provider_returns_error() {
        let error = validate_output(Some("missing".to_string())).expect_err("error");

        assert!(error.to_string().contains("unknown provider: missing"));
    }

    #[test]
    fn status_output_inspects_directory() {
        let temp = tempfile::tempdir().expect("temp dir");
        let output = status_output(temp.path().display().to_string()).expect("status");
        let status: serde_json::Value = serde_json::from_str(&output).expect("json");
        let expected_root = temp.path().canonicalize().expect("canonical temp dir");

        assert_eq!(status["root"], expected_root.display().to_string());
        assert_eq!(status["dirty"], false);
    }
}
