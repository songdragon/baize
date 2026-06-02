use anyhow::{Context, Result};
use baize_core::{
    HealthStatus, ProviderCapabilities, ProviderHealth, ProviderId, ProviderKind, ProviderProfile,
    ProviderTransport,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderValidation {
    pub provider_id: ProviderId,
    pub health: ProviderHealth,
    pub version: Option<String>,
    pub detected: DetectedCapabilities,
    pub capability_gaps: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectedCapabilities {
    pub non_interactive_prompt: bool,
    pub structured_output: bool,
    pub acp: bool,
    pub session_resume: bool,
    pub mcp_server: bool,
    pub app_server: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPromptRequest {
    pub provider_id: ProviderId,
    pub prompt: String,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub provider_id: ProviderId,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub native_session_id: Option<String>,
    pub events: Vec<AgentExecutionEvent>,
    pub stderr: String,
    pub error: Option<AgentErrorDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionEvent {
    pub kind: AgentExecutionEventKind,
    pub text: Option<String>,
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentExecutionEventKind {
    Output,
    ToolCall,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentErrorDetail {
    pub kind: AgentErrorKind,
    pub message: String,
    pub source: AgentErrorSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentErrorKind {
    Authentication,
    Timeout,
    RateLimit,
    QuotaExceeded,
    ProcessFailure,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentErrorSource {
    Stderr,
    Runtime,
}

pub fn default_provider_profiles() -> Vec<ProviderProfile> {
    vec![
        ProviderProfile {
            id: ProviderId("codex".to_string()),
            kind: ProviderKind::Codex,
            display_name: "Codex".to_string(),
            priority: 1,
            transports: vec![ProviderTransport::Cli {
                command: "codex".to_string(),
                args: vec![],
            }],
            capabilities: ProviderCapabilities {
                interactive_chat: true,
                non_interactive_prompt: true,
                shell_access: true,
                structured_output: true,
                session_resume: true,
                mcp_server: true,
                ..ProviderCapabilities::default()
            },
            enabled: true,
        },
        ProviderProfile {
            id: ProviderId("gemini".to_string()),
            kind: ProviderKind::Gemini,
            display_name: "Gemini CLI".to_string(),
            priority: 2,
            transports: vec![
                ProviderTransport::Acp {
                    command: "gemini".to_string(),
                    args: vec!["--acp".to_string()],
                },
                ProviderTransport::Cli {
                    command: "gemini".to_string(),
                    args: vec![],
                },
            ],
            capabilities: ProviderCapabilities {
                interactive_chat: true,
                non_interactive_prompt: true,
                shell_access: true,
                structured_output: true,
                acp: true,
                session_resume: true,
                ..ProviderCapabilities::default()
            },
            enabled: true,
        },
        ProviderProfile {
            id: ProviderId("copilot".to_string()),
            kind: ProviderKind::Copilot,
            display_name: "GitHub Copilot CLI".to_string(),
            priority: 3,
            transports: vec![ProviderTransport::Acp {
                command: "copilot".to_string(),
                args: vec!["--acp".to_string(), "--stdio".to_string()],
            }],
            capabilities: ProviderCapabilities {
                interactive_chat: true,
                shell_access: true,
                acp: true,
                ..ProviderCapabilities::default()
            },
            enabled: true,
        },
        ProviderProfile {
            id: ProviderId("opencode".to_string()),
            kind: ProviderKind::OpenCode,
            display_name: "OpenCode".to_string(),
            priority: 4,
            transports: vec![ProviderTransport::Acp {
                command: "opencode".to_string(),
                args: vec!["acp".to_string()],
            }],
            capabilities: ProviderCapabilities {
                interactive_chat: true,
                shell_access: true,
                acp: true,
                ..ProviderCapabilities::default()
            },
            enabled: true,
        },
    ]
}

pub fn check_provider(profile: &ProviderProfile) -> ProviderHealth {
    let command = profile.transports.first().map(command_for_transport);
    let Some(command) = command else {
        return unavailable(
            profile.id.clone(),
            "provider has no transport".to_string(),
            None,
        );
    };

    let start = Instant::now();
    let status = Command::new(command).arg("--version").output();
    let latency_ms = Some(start.elapsed().as_millis() as u64);

    match status {
        Ok(output) if output.status.success() => ProviderHealth {
            provider_id: profile.id.clone(),
            status: HealthStatus::Healthy,
            latency_ms,
            last_error: None,
            checked_at: Utc::now(),
        },
        Ok(output) => unavailable(
            profile.id.clone(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
            latency_ms,
        ),
        Err(error) => unavailable(profile.id.clone(), error.to_string(), latency_ms),
    }
}

pub fn validate_provider(profile: &ProviderProfile) -> ProviderValidation {
    let health = check_provider(profile);
    let command = profile.transports.first().map(command_for_transport);
    let (version, help) = if let Some(command) = command {
        (
            command_text(command, &["--version"]),
            command_text(command, &["--help"]),
        )
    } else {
        (None, None)
    };
    let exec_help = match profile.kind {
        ProviderKind::Codex => {
            command.and_then(|command| command_text(command, &["exec", "--help"]))
        }
        _ => None,
    };
    let resume_help = match profile.kind {
        ProviderKind::Codex => {
            command.and_then(|command| command_text(command, &["resume", "--help"]))
        }
        _ => None,
    };
    let detected = detect_capabilities(
        profile,
        help.as_deref(),
        exec_help.as_deref(),
        resume_help.as_deref(),
    );
    let capability_gaps = capability_gaps(&profile.capabilities, &detected);

    ProviderValidation {
        provider_id: profile.id.clone(),
        health,
        version: version.map(clean_version),
        detected,
        capability_gaps,
    }
}

pub fn validate_all_providers() -> Vec<ProviderValidation> {
    default_provider_profiles()
        .iter()
        .map(validate_provider)
        .collect()
}

pub fn run_agent_prompt(request: AgentPromptRequest) -> Result<AgentRunResult> {
    match request.provider_id.0.as_str() {
        "gemini" => run_gemini_prompt(request),
        "codex" => run_codex_prompt(request),
        other => anyhow::bail!("provider execution is not implemented for {other}"),
    }
}

pub fn build_gemini_args(request: &AgentPromptRequest) -> Vec<String> {
    let mut args = vec![
        "--prompt".to_string(),
        request.prompt.clone(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--approval-mode".to_string(),
        "plan".to_string(),
        "--skip-trust".to_string(),
    ];
    if let Some(session_id) = &request.session_id {
        args.push("--session-id".to_string());
        args.push(session_id.clone());
    }
    args
}

pub fn build_codex_args(request: &AgentPromptRequest) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--sandbox".to_string(),
        "read-only".to_string(),
        "--ask-for-approval".to_string(),
        "never".to_string(),
        "--cd".to_string(),
        request.cwd.to_string_lossy().to_string(),
    ];
    if let Some(session_id) = &request.session_id {
        args.push("resume".to_string());
        args.push(session_id.clone());
    }
    args.push(request.prompt.clone());
    args
}

pub fn parse_stream_json_lines(raw: &str) -> Vec<AgentExecutionEvent> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let value = serde_json::from_str::<Value>(line).ok()?;
            let kind = if looks_like_tool_call(&value) {
                AgentExecutionEventKind::ToolCall
            } else if find_text(&value).is_some() {
                AgentExecutionEventKind::Output
            } else {
                AgentExecutionEventKind::Raw
            };
            Some(AgentExecutionEvent {
                text: find_text(&value),
                kind,
                raw: Some(value),
            })
        })
        .collect()
}

pub fn extract_native_session_id(raw: &str) -> Option<String> {
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find_map(|value| find_session_id(&value))
}

pub fn classify_agent_error(text: &str, source: AgentErrorSource) -> Option<AgentErrorDetail> {
    let message = text.trim();
    if message.is_empty() {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    let kind = if lower.contains("timed out") || lower.contains("timeout") {
        AgentErrorKind::Timeout
    } else if lower.contains("unauthorized")
        || lower.contains("not authenticated")
        || lower.contains("authentication")
        || lower.contains("login required")
        || lower.contains("please login")
    {
        AgentErrorKind::Authentication
    } else if lower.contains("429")
        || lower.contains("too many requests")
        || lower.contains("rate limit")
    {
        AgentErrorKind::RateLimit
    } else if lower.contains("quota")
        || lower.contains("billing")
        || lower.contains("usage limit")
        || lower.contains("credit")
    {
        AgentErrorKind::QuotaExceeded
    } else if source == AgentErrorSource::Runtime {
        AgentErrorKind::ProcessFailure
    } else {
        AgentErrorKind::Unknown
    };

    Some(AgentErrorDetail {
        kind,
        message: message.to_string(),
        source,
    })
}

fn run_gemini_prompt(request: AgentPromptRequest) -> Result<AgentRunResult> {
    let output = run_command_with_timeout(
        "gemini",
        &build_gemini_args(&request),
        &request.cwd,
        timeout_for(&request),
    )
    .with_context(|| "failed to run gemini prompt")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(AgentRunResult {
        provider_id: request.provider_id,
        success: output.status.success(),
        exit_code: output.status.code(),
        native_session_id: extract_native_session_id(&stdout),
        events: parse_stream_json_lines(&stdout),
        error: if output.status.success() {
            None
        } else {
            classify_agent_error(&stderr, AgentErrorSource::Stderr)
        },
        stderr,
    })
}

fn run_codex_prompt(request: AgentPromptRequest) -> Result<AgentRunResult> {
    let output = run_command_with_timeout(
        "codex",
        &build_codex_args(&request),
        &request.cwd,
        timeout_for(&request),
    )
    .with_context(|| "failed to run codex prompt")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(AgentRunResult {
        provider_id: request.provider_id,
        success: output.status.success(),
        exit_code: output.status.code(),
        native_session_id: extract_native_session_id(&stdout),
        events: parse_stream_json_lines(&stdout),
        error: if output.status.success() {
            None
        } else {
            classify_agent_error(&stderr, AgentErrorSource::Stderr)
        },
        stderr,
    })
}

fn timeout_for(request: &AgentPromptRequest) -> Duration {
    Duration::from_secs(request.timeout_seconds.unwrap_or(120))
}

fn run_command_with_timeout(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    timeout: Duration,
) -> Result<Output> {
    let mut child = Command::new(command)
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {command}"))?;

    if child.wait_timeout(timeout)?.is_none() {
        let _ = child.kill();
        let output = child.wait_with_output()?;
        anyhow::bail!(
            "{command} timed out after {} seconds. stdout: {} stderr: {}",
            timeout.as_secs(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    child
        .wait_with_output()
        .with_context(|| format!("failed to collect {command} output"))
}

fn detect_capabilities(
    profile: &ProviderProfile,
    help: Option<&str>,
    exec_help: Option<&str>,
    resume_help: Option<&str>,
) -> DetectedCapabilities {
    let help = help.unwrap_or_default();
    let exec_help = exec_help.unwrap_or_default();
    let resume_help = resume_help.unwrap_or_default();
    match profile.kind {
        ProviderKind::Codex => DetectedCapabilities {
            non_interactive_prompt: help.contains("exec")
                && exec_help.contains("Run Codex non-interactively"),
            structured_output: exec_help.contains("--json")
                || exec_help.contains("--output-schema"),
            acp: help.contains("--acp"),
            session_resume: help.contains("resume") && resume_help.contains("--last"),
            mcp_server: help.contains("mcp-server"),
            app_server: help.contains("app-server"),
        },
        ProviderKind::Gemini => DetectedCapabilities {
            non_interactive_prompt: help.contains("--prompt"),
            structured_output: help.contains("--output-format") && help.contains("stream-json"),
            acp: help.contains("--acp"),
            session_resume: help.contains("--resume") && help.contains("--session-id"),
            mcp_server: help.contains("gemini mcp"),
            app_server: false,
        },
        ProviderKind::Copilot => DetectedCapabilities {
            acp: help.contains("--acp"),
            non_interactive_prompt: help.contains("programmatic") || help.contains("[PROMPT]"),
            ..DetectedCapabilities::default()
        },
        ProviderKind::OpenCode => DetectedCapabilities {
            acp: true,
            non_interactive_prompt: true,
            session_resume: true,
            ..DetectedCapabilities::default()
        },
        ProviderKind::Other(_) => DetectedCapabilities::default(),
    }
}

fn looks_like_tool_call(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty.contains("tool") || ty.contains("call"))
        .unwrap_or(false)
        || value.get("tool").is_some()
        || value.get("tool_call").is_some()
}

fn find_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            if text.trim().is_empty() {
                None
            } else {
                Some(text.clone())
            }
        }
        Value::Array(values) => values.iter().find_map(find_text),
        Value::Object(map) => {
            for key in ["text", "content", "message", "delta"] {
                if let Some(text) = map.get(key).and_then(find_text) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_session_id(value: &Value) -> Option<String> {
    match value {
        Value::Array(values) => values.iter().find_map(find_session_id),
        Value::Object(map) => {
            for key in [
                "session_id",
                "sessionId",
                "conversation_id",
                "conversationId",
                "thread_id",
                "threadId",
            ] {
                if let Some(id) = map.get(key).and_then(Value::as_str) {
                    if !id.trim().is_empty() {
                        return Some(id.to_string());
                    }
                }
            }
            map.values().find_map(find_session_id)
        }
        _ => None,
    }
}

fn capability_gaps(
    expected: &ProviderCapabilities,
    detected: &DetectedCapabilities,
) -> Vec<String> {
    let mut gaps = Vec::new();
    if expected.non_interactive_prompt && !detected.non_interactive_prompt {
        gaps.push("non_interactive_prompt".to_string());
    }
    if expected.structured_output && !detected.structured_output {
        gaps.push("structured_output".to_string());
    }
    if expected.acp && !detected.acp {
        gaps.push("acp".to_string());
    }
    if expected.session_resume && !detected.session_resume {
        gaps.push("session_resume".to_string());
    }
    if expected.mcp_server && !detected.mcp_server {
        gaps.push("mcp_server".to_string());
    }
    gaps
}

fn command_text(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn clean_version(raw: String) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rfind(|line| !line.to_ascii_lowercase().starts_with("warning:"))
        .unwrap_or_default()
        .to_string()
}

fn command_for_transport(transport: &ProviderTransport) -> &str {
    match transport {
        ProviderTransport::Acp { command, .. } => command,
        ProviderTransport::Native => "",
        ProviderTransport::Cli { command, .. } => command,
    }
}

fn unavailable(
    provider_id: ProviderId,
    last_error: String,
    latency_ms: Option<u64>,
) -> ProviderHealth {
    ProviderHealth {
        provider_id,
        status: HealthStatus::Unavailable,
        latency_ms,
        last_error: Some(last_error),
        checked_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use baize_core::ProviderTransport;

    #[test]
    fn default_profiles_prioritize_codex_and_gemini() {
        let profiles = default_provider_profiles();

        assert_eq!(profiles[0].id.0, "codex");
        assert_eq!(profiles[1].id.0, "gemini");
        assert_eq!(profiles[2].id.0, "copilot");
        assert_eq!(profiles[3].id.0, "opencode");
    }

    #[test]
    fn copilot_and_opencode_expose_acp_transports() {
        let profiles = default_provider_profiles();
        let copilot = profiles
            .iter()
            .find(|profile| profile.id.0 == "copilot")
            .expect("copilot profile");
        let opencode = profiles
            .iter()
            .find(|profile| profile.id.0 == "opencode")
            .expect("opencode profile");

        assert!(copilot.capabilities.acp);
        assert!(opencode.capabilities.acp);
        assert!(matches!(
            copilot.transports[0],
            ProviderTransport::Acp { .. }
        ));
        assert!(matches!(
            opencode.transports[0],
            ProviderTransport::Acp { .. }
        ));
    }

    #[test]
    fn codex_help_detection_finds_exec_json_and_resume() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "codex")
            .expect("codex profile");
        let help = "Commands:\n  exec\n  resume\n  mcp-server\n  app-server\n";
        let exec_help = "Run Codex non-interactively\n      --json\n      --output-schema <FILE>\n";
        let resume_help = "Resume a previous interactive session\n      --last\n";

        let detected =
            detect_capabilities(&profile, Some(help), Some(exec_help), Some(resume_help));

        assert!(detected.non_interactive_prompt);
        assert!(detected.structured_output);
        assert!(detected.session_resume);
        assert!(detected.mcp_server);
        assert!(detected.app_server);
        assert!(!detected.acp);
    }

    #[test]
    fn gemini_help_detection_finds_acp_prompt_json_and_resume() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "gemini")
            .expect("gemini profile");
        let help = "Options:\n  --prompt\n  --acp\n  --resume\n  --session-id\n  --output-format text json stream-json\nCommands:\n  gemini mcp\n";

        let detected = detect_capabilities(&profile, Some(help), None, None);

        assert!(detected.non_interactive_prompt);
        assert!(detected.structured_output);
        assert!(detected.acp);
        assert!(detected.session_resume);
        assert!(detected.mcp_server);
    }

    #[test]
    fn clean_version_ignores_warning_lines() {
        let version = clean_version(
            "codex-cli 0.133.0\nWARNING: proceeding, even though PATH failed\n".to_string(),
        );

        assert_eq!(version, "codex-cli 0.133.0");
    }

    #[test]
    fn builds_safe_gemini_stream_json_command() {
        let request = AgentPromptRequest {
            provider_id: ProviderId("gemini".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: Some("session-1".to_string()),
            timeout_seconds: None,
        };

        let args = build_gemini_args(&request);

        assert!(args.windows(2).any(|pair| pair == ["--prompt", "hello"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--output-format", "stream-json"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--approval-mode", "plan"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-id", "session-1"]));
    }

    #[test]
    fn builds_safe_codex_json_command() {
        let request = AgentPromptRequest {
            provider_id: ProviderId("codex".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: None,
            timeout_seconds: None,
        };

        let args = build_codex_args(&request);

        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_string()));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "read-only"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--ask-for-approval", "never"]));
        assert_eq!(args.last().expect("prompt"), "hello");
    }

    #[test]
    fn parses_stream_json_output_and_tool_events() {
        let raw = r#"
        {"type":"message","message":{"content":[{"text":"hello"}]}}
        {"type":"tool_call","tool":"read_file","args":{"path":"README.md"}}
        {"type":"unknown","value":123}
        "#;

        let events = parse_stream_json_lines(raw);

        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].kind, AgentExecutionEventKind::Output));
        assert_eq!(events[0].text.as_deref(), Some("hello"));
        assert!(matches!(events[1].kind, AgentExecutionEventKind::ToolCall));
        assert!(matches!(events[2].kind, AgentExecutionEventKind::Raw));
    }

    #[test]
    fn extracts_native_session_id_from_structured_output() {
        let raw = r#"
        {"type":"message","session_id":"sess_codex_1","message":"hello"}
        {"type":"message","session_id":"sess_codex_2","message":"later"}
        "#;

        assert_eq!(
            extract_native_session_id(raw).as_deref(),
            Some("sess_codex_1")
        );
    }

    #[test]
    fn extracts_nested_native_session_id_and_ignores_empty_values() {
        let raw = r#"
        {"session_id":"","message":"missing"}
        {"type":"metadata","payload":{"conversationId":"conv_gemini_1"}}
        "#;

        assert_eq!(
            extract_native_session_id(raw).as_deref(),
            Some("conv_gemini_1")
        );
    }

    #[test]
    fn native_session_id_extraction_returns_none_without_known_fields() {
        let raw = r#"
        {"type":"message","message":"hello"}
        {"type":"metadata","payload":{"id":"too broad to trust"}}
        "#;

        assert!(extract_native_session_id(raw).is_none());
    }

    #[test]
    fn classifies_authentication_rate_and_quota_errors() {
        let auth = classify_agent_error(
            "Please login before using this CLI",
            AgentErrorSource::Stderr,
        )
        .expect("auth error");
        assert_eq!(auth.kind, AgentErrorKind::Authentication);
        assert_eq!(auth.source, AgentErrorSource::Stderr);

        let rate = classify_agent_error("HTTP 429 Too Many Requests", AgentErrorSource::Stderr)
            .expect("rate error");
        assert_eq!(rate.kind, AgentErrorKind::RateLimit);

        let quota = classify_agent_error(
            "insufficient quota, check billing",
            AgentErrorSource::Stderr,
        )
        .expect("quota error");
        assert_eq!(quota.kind, AgentErrorKind::QuotaExceeded);
    }

    #[test]
    fn classifies_timeout_runtime_and_empty_error_text() {
        let timeout = classify_agent_error(
            "codex timed out after 10 seconds",
            AgentErrorSource::Runtime,
        )
        .expect("timeout error");
        assert_eq!(timeout.kind, AgentErrorKind::Timeout);

        let runtime = classify_agent_error("failed to spawn codex", AgentErrorSource::Runtime)
            .expect("runtime error");
        assert_eq!(runtime.kind, AgentErrorKind::ProcessFailure);

        assert!(classify_agent_error("   ", AgentErrorSource::Stderr).is_none());
    }

    #[test]
    fn command_timeout_prevents_hanging_processes() {
        let error = run_command_with_timeout(
            "sleep",
            &["2".to_string()],
            &PathBuf::from("."),
            Duration::from_millis(10),
        )
        .expect_err("sleep should time out");

        assert!(error.to_string().contains("timed out"));
    }
}
