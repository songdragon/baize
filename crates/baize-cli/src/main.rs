use anyhow::Result;
use baize_adapters::{ProviderDiagnostic, ProviderReadiness};
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
    Smoke {
        provider: String,
        #[arg(long)]
        run_prompt: bool,
        #[arg(long, default_value = "Return exactly: baize-smoke")]
        prompt: String,
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
        #[arg(long, default_value = ".")]
        path: String,
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
        Command::Smoke {
            provider,
            run_prompt,
            prompt,
            timeout_seconds,
            path,
        } => {
            smoke_output(provider, run_prompt, prompt, timeout_seconds, path).map(CliAction::Print)
        }
    }
}

fn status_output(path: String) -> Result<String> {
    let status = baize_workspace::inspect(path)?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&status)?))
}

fn doctor_output() -> Result<String> {
    let diagnostics = baize_adapters::diagnose_all_providers();
    let config = baize_config::load_or_default()?;
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&doctor_report(&diagnostics, &config))?
    ))
}

fn doctor_report(
    diagnostics: &[ProviderDiagnostic],
    config: &baize_config::BaizeConfig,
) -> serde_json::Value {
    let ready_prompt_providers = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.prompt_runtime_supported && diagnostic.readiness == ProviderReadiness::Ready
        })
        .map(|diagnostic| diagnostic.provider_id.0.clone())
        .collect::<Vec<_>>();
    let blocked_prompt_providers = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.prompt_runtime_supported && diagnostic.readiness != ProviderReadiness::Ready
        })
        .map(|diagnostic| diagnostic.provider_id.0.clone())
        .collect::<Vec<_>>();

    json!({
        "coding_ready": !ready_prompt_providers.is_empty(),
        "ready_prompt_providers": ready_prompt_providers,
        "blocked_prompt_providers": blocked_prompt_providers,
        "runtime": {
            "command_policy": config.workspace.command_policy,
            "checkpoint_policy": config.workspace.checkpoint_policy,
            "routing": {
                "sticky_window_minutes": config.routing.sticky_window_minutes,
                "quota_switch_threshold_percent": config.routing.quota_switch_threshold_percent,
                "failure_threshold_count": config.routing.failure_threshold_count,
            }
        },
        "providers": diagnostics,
    })
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

fn smoke_output(
    provider: String,
    run_prompt: bool,
    prompt: String,
    timeout_seconds: u64,
    path: String,
) -> Result<String> {
    let providers = baize_adapters::default_provider_profiles();
    let Some(profile) = providers.iter().find(|profile| profile.id.0 == provider) else {
        anyhow::bail!("unknown provider: {provider}");
    };
    let report = baize_adapters::smoke_provider(
        profile,
        baize_adapters::ProviderSmokeOptions {
            cwd: path.into(),
            run_prompt,
            prompt,
            timeout_seconds: Some(timeout_seconds),
        },
    )?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&report)?))
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
    use baize_adapters::DetectedCapabilities;
    use baize_core::{HealthStatus, ProviderHealth, ProviderId};
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
    fn doctor_output_includes_provider_diagnostics() {
        let output = doctor_output().expect("doctor");
        let value: serde_json::Value = serde_json::from_str(&output).expect("json");
        let provider = &value["providers"][0];

        assert!(value.get("coding_ready").is_some());
        assert!(value.get("ready_prompt_providers").is_some());
        assert_eq!(value["runtime"]["command_policy"], "ask");
        assert_eq!(provider["provider_id"], "codex");
        assert!(provider.get("readiness").is_some());
        assert!(provider.get("issues").is_some());
        assert!(provider.get("suggested_actions").is_some());
    }

    #[test]
    fn doctor_report_marks_coding_ready_when_prompt_provider_is_ready() {
        let config = baize_config::BaizeConfig::default();
        let diagnostics = vec![
            diagnostic("codex", ProviderReadiness::Ready, true),
            diagnostic("copilot", ProviderReadiness::UnsupportedRuntime, false),
        ];

        let value = doctor_report(&diagnostics, &config);

        assert_eq!(value["coding_ready"], true);
        assert_eq!(value["ready_prompt_providers"][0], "codex");
        assert_eq!(
            value["blocked_prompt_providers"].as_array().unwrap().len(),
            0
        );
        assert_eq!(value["runtime"]["routing"]["sticky_window_minutes"], 30);
    }

    #[test]
    fn doctor_report_marks_not_ready_without_ready_prompt_provider() {
        let config = baize_config::BaizeConfig::default();
        let diagnostics = vec![
            diagnostic("codex", ProviderReadiness::SetupRequired, true),
            diagnostic("copilot", ProviderReadiness::UnsupportedRuntime, false),
        ];

        let value = doctor_report(&diagnostics, &config);

        assert_eq!(value["coding_ready"], false);
        assert_eq!(value["blocked_prompt_providers"][0], "codex");
        assert_eq!(value["ready_prompt_providers"].as_array().unwrap().len(), 0);
    }

    fn diagnostic(
        provider_id: &str,
        readiness: ProviderReadiness,
        prompt_runtime_supported: bool,
    ) -> ProviderDiagnostic {
        ProviderDiagnostic {
            provider_id: ProviderId(provider_id.to_string()),
            display_name: provider_id.to_string(),
            readiness,
            health: ProviderHealth {
                provider_id: ProviderId(provider_id.to_string()),
                status: HealthStatus::Healthy,
                latency_ms: Some(1),
                last_error: None,
                checked_at: chrono::Utc::now(),
            },
            version: Some("1.0.0".to_string()),
            prompt_runtime_supported,
            detected: DetectedCapabilities::default(),
            acp_proof: None,
            issues: Vec::new(),
            suggested_actions: Vec::new(),
        }
    }

    #[test]
    fn validate_unknown_provider_returns_error() {
        let error = validate_output(Some("missing".to_string())).expect_err("error");

        assert!(error.to_string().contains("unknown provider: missing"));
    }

    #[test]
    fn smoke_unknown_provider_returns_error() {
        let error = smoke_output(
            "missing".to_string(),
            false,
            "baize smoke".to_string(),
            5,
            ".".to_string(),
        )
        .expect_err("error");

        assert!(error.to_string().contains("unknown provider: missing"));
    }

    #[test]
    fn smoke_output_skips_prompt_by_default() {
        let output = smoke_output(
            "codex".to_string(),
            false,
            "baize smoke".to_string(),
            5,
            ".".to_string(),
        )
        .expect("smoke output");
        let value: serde_json::Value = serde_json::from_str(&output).expect("json");

        assert_eq!(value["provider_id"], "codex");
        assert_eq!(value["prompt_skipped"], true);
        assert_eq!(value["parser_native_session_id"], "smoke_codex_session");
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
