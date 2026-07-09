use anyhow::{anyhow, Context, Result};
use baize_adapters::{ProviderDiagnostic, ProviderReadiness};
use clap::{Parser, Subcommand};
use serde_json::json;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;

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
    Ask {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = ".")]
        path: String,
        #[arg(long, default_value_t = 120)]
        timeout_seconds: u64,
        #[arg(required = true, num_args = 1..)]
        prompt: Vec<String>,
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
    RunAsk(AskOptions),
    Print(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AskOptions {
    provider: Option<String>,
    path: String,
    timeout_seconds: u64,
    prompt: String,
}

struct DaemonClient {
    host: String,
    port: u16,
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
        CliAction::RunAsk(options) => {
            print!("{}", ask_output(options)?);
            Ok(())
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
        Command::Ask {
            provider,
            path,
            timeout_seconds,
            prompt,
        } => Ok(CliAction::RunAsk(AskOptions {
            provider,
            path,
            timeout_seconds,
            prompt: prompt.join(" "),
        })),
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

fn ask_output(options: AskOptions) -> Result<String> {
    let config = baize_config::load_or_default()?;
    let client = DaemonClient {
        host: config.daemon.host,
        port: config.daemon.port,
    };
    let workspace = client.post_json(
        "/workspaces",
        json!({
            "path": options.path.clone(),
        }),
    )?;
    let workspace_id = workspace
        .get("workspace")
        .and_then(|workspace| workspace.get("id"))
        .and_then(id_value)
        .ok_or_else(|| anyhow!("daemon response missing workspace.id: {workspace}"))?;
    let mut session_body = json!({
        "workspace_id": workspace_id,
        "objective": options.prompt.clone(),
    });
    if let Some(provider) = &options.provider {
        session_body["provider_id"] = Value::String(provider.clone());
    }
    let session = client.post_json("/sessions", session_body)?;
    let session_id = session
        .get("session")
        .and_then(|session| session.get("id"))
        .and_then(id_value)
        .ok_or_else(|| anyhow!("daemon response missing session.id: {session}"))?;
    let mut prompt_body = json!({
        "prompt": options.prompt.clone(),
        "timeout_seconds": options.timeout_seconds,
    });
    if let Some(provider) = &options.provider {
        prompt_body["provider_id"] = Value::String(provider.clone());
    }
    let prompt_response =
        client.post_json(&format!("/sessions/{session_id}/prompt"), prompt_body)?;
    let diff_response = client
        .get_json(&format!("/sessions/{session_id}/diff"))
        .unwrap_or_else(|error| json!({ "diff_error": error.to_string() }));
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&ask_summary(&prompt_response, &diff_response))?
    ))
}

fn ask_summary(prompt_response: &Value, diff_response: &Value) -> Value {
    json!({
        "session_id": prompt_response.get("session_id").and_then(Value::as_str),
        "provider_id": prompt_response.get("provider_id").and_then(id_value),
        "turn_status": prompt_response.get("turn_status").and_then(Value::as_str),
        "session_status": prompt_response.get("session_status").and_then(Value::as_str),
        "assistant_text": assistant_text(prompt_response),
        "changed_files": changed_files(diff_response),
        "error": prompt_error(prompt_response),
    })
}

fn assistant_text(response: &Value) -> String {
    response
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter(|event| {
                    event
                        .get("kind")
                        .and_then(Value::as_str)
                        .is_none_or(|kind| kind == "Output")
                })
                .filter_map(|event| event.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn changed_files(response: &Value) -> Vec<String> {
    response
        .get("diff")
        .and_then(|diff| diff.get("changed_files"))
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn prompt_error(response: &Value) -> Option<String> {
    response
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| response.get("stderr").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .or_else(|| {
            response
                .get("provider_error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

impl DaemonClient {
    fn get_json(&self, path: &str) -> Result<Value> {
        self.request_json("GET", path, None)
    }

    fn post_json(&self, path: &str, body: Value) -> Result<Value> {
        self.request_json("POST", path, Some(body))
    }

    fn request_json(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let body = body.map(|body| body.to_string()).unwrap_or_default();
        let mut stream =
            TcpStream::connect((self.host.as_str(), self.port)).with_context(|| {
                format!(
                    "baize daemon is not reachable at {}:{}; start it with `baize daemon` first",
                    self.host, self.port
                )
            })?;
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            self.host,
            self.port,
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .context("send daemon request")?;

        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .context("read daemon response")?;
        parse_http_json_response(&response)
    }
}

fn parse_http_json_response(response: &str) -> Result<Value> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid daemon response"))?;
    let status_line = head.lines().next().unwrap_or_default();
    let body = body.trim();
    if body.is_empty() {
        return Err(anyhow!("daemon returned {status_line} with empty body"));
    }
    let value: Value = serde_json::from_str(body).context("parse daemon JSON response")?;
    if !status_line.contains(" 200 ") {
        if matches!(
            value.get("status").and_then(Value::as_str),
            Some("failed" | "canceled")
        ) {
            return Ok(value);
        }
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(anyhow!("daemon returned {status_line}: {error}"));
        }
        return Err(anyhow!("daemon returned {status_line}"));
    }
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(anyhow!(error.to_string()));
    }
    Ok(value)
}

fn id_value(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
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
    fn ask_command_maps_to_ask_action() {
        let action = plan_cli_action(Some(Command::Ask {
            provider: Some("gemini".to_string()),
            path: "crates".to_string(),
            timeout_seconds: 30,
            prompt: vec!["summarize".to_string(), "this".to_string()],
        }))
        .expect("action");

        match action {
            CliAction::RunAsk(options) => {
                assert_eq!(options.provider.as_deref(), Some("gemini"));
                assert_eq!(options.path, "crates");
                assert_eq!(options.timeout_seconds, 30);
                assert_eq!(options.prompt, "summarize this");
            }
            _ => panic!("expected ask action"),
        }
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
        assert_eq!(providers[1]["id"], "antigravity");
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
    fn ask_summary_formats_prompt_and_diff_response() {
        let summary = ask_summary(
            &json!({
                "session_id": "task_1",
                "provider_id": "codex",
                "turn_status": "completed",
                "session_status": "Running",
                "events": [
                    { "kind": "Output", "text": "first" },
                    { "kind": "ToolCall", "text": "cargo test" },
                    { "kind": "Output", "text": "second" }
                ]
            }),
            &json!({
                "diff": {
                    "changed_files": ["src/lib.rs", "README.md"]
                }
            }),
        );

        assert_eq!(summary["session_id"], "task_1");
        assert_eq!(summary["provider_id"], "codex");
        assert_eq!(summary["turn_status"], "completed");
        assert_eq!(summary["assistant_text"], "first\nsecond");
        assert_eq!(summary["changed_files"][0], "src/lib.rs");
        assert!(summary["error"].is_null());
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
