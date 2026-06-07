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
    pub acp_proof: Option<AcpProof>,
    pub capability_gaps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderReadiness {
    Ready,
    SetupRequired,
    UnsupportedRuntime,
    CapabilityMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDiagnostic {
    pub provider_id: ProviderId,
    pub display_name: String,
    pub readiness: ProviderReadiness,
    pub health: ProviderHealth,
    pub version: Option<String>,
    pub prompt_runtime_supported: bool,
    pub detected: DetectedCapabilities,
    pub acp_proof: Option<AcpProof>,
    pub issues: Vec<String>,
    pub suggested_actions: Vec<String>,
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
pub struct AcpProof {
    pub command: String,
    pub args: Vec<String>,
    pub initialize_request: baize_acp::JsonRpcRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSmokeOptions {
    pub cwd: PathBuf,
    pub run_prompt: bool,
    pub prompt: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSmokeReport {
    pub provider_id: ProviderId,
    pub version_detected: bool,
    pub help_detected: bool,
    pub prompt_command_args: Vec<String>,
    pub parser_event_count: usize,
    pub parser_native_session_id: Option<String>,
    pub prompt_result: Option<AgentRunResult>,
    pub prompt_skipped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPromptRequest {
    pub provider_id: ProviderId,
    pub prompt: String,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub execution_policy: AgentExecutionPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentExecutionPolicy {
    Ask,
    AllowProject,
    Deny,
}

impl AgentExecutionPolicy {
    pub fn from_command_policy(policy: &str) -> Self {
        match policy {
            "allow_project" => Self::AllowProject,
            "deny" => Self::Deny,
            _ => Self::Ask,
        }
    }
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

struct CommandRunOutput {
    output: Output,
    timed_out: bool,
    timeout: Duration,
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
                non_interactive_prompt: true,
                shell_access: true,
                structured_output: true,
                acp: true,
                session_resume: true,
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
        ProviderKind::OpenCode => {
            command.and_then(|command| command_text(command, &["run", "--help"]))
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
    let acp_proof = build_acp_proof(profile);
    let capability_gaps = capability_gaps(&profile.capabilities, &detected);

    ProviderValidation {
        provider_id: profile.id.clone(),
        health,
        version: version.map(clean_version),
        detected,
        acp_proof,
        capability_gaps,
    }
}

pub fn validate_all_providers() -> Vec<ProviderValidation> {
    default_provider_profiles()
        .iter()
        .map(validate_provider)
        .collect()
}

pub fn diagnose_provider(profile: &ProviderProfile) -> ProviderDiagnostic {
    let validation = validate_provider(profile);
    diagnostic_from_validation(profile, validation)
}

pub fn diagnose_all_providers() -> Vec<ProviderDiagnostic> {
    default_provider_profiles()
        .iter()
        .map(diagnose_provider)
        .collect()
}

pub fn is_prompt_runtime_supported(provider_id: &ProviderId) -> bool {
    prompt_runtime_supported(provider_id)
}

pub fn run_agent_prompt(request: AgentPromptRequest) -> Result<AgentRunResult> {
    match request.provider_id.0.as_str() {
        "gemini" => run_gemini_prompt(request),
        "codex" => run_codex_prompt(request),
        "opencode" => run_opencode_prompt(request),
        other => anyhow::bail!("provider execution is not implemented for {other}"),
    }
}

pub fn smoke_provider(
    profile: &ProviderProfile,
    options: ProviderSmokeOptions,
) -> Result<ProviderSmokeReport> {
    let provider_id = profile.id.clone();
    let command = profile
        .transports
        .first()
        .map(command_for_transport)
        .filter(|command| !command.is_empty())
        .context("provider has no executable transport")?;
    let version_detected = command_text(command, &["--version"]).is_some();
    let help_detected = command_text(command, &["--help"]).is_some();
    let prompt_request = AgentPromptRequest {
        provider_id: provider_id.clone(),
        prompt: options.prompt,
        cwd: options.cwd,
        session_id: None,
        timeout_seconds: options.timeout_seconds,
        execution_policy: AgentExecutionPolicy::Deny,
    };
    let prompt_command_args = match provider_id.0.as_str() {
        "codex" => build_codex_args(&prompt_request),
        "gemini" => build_gemini_args(&prompt_request),
        "opencode" => build_opencode_args(&prompt_request),
        other => anyhow::bail!("provider smoke is not implemented for {other}"),
    };
    let fixture = match provider_id.0.as_str() {
        "codex" => {
            r#"{"type":"message","session_id":"smoke_codex_session","message":{"content":[{"text":"baize-smoke"}]}}"#
        }
        "gemini" => {
            r#"{"type":"message","conversationId":"smoke_gemini_session","message":{"content":[{"text":"baize-smoke"}]}}"#
        }
        "opencode" => {
            r#"{"type":"message","sessionID":"smoke_opencode_session","message":{"content":[{"text":"baize-smoke"}]}}"#
        }
        _ => unreachable!("provider checked above"),
    };
    let parser_events = parse_stream_json_lines(fixture);
    let parser_native_session_id = extract_native_session_id(fixture);
    let prompt_result = if options.run_prompt {
        Some(run_agent_prompt(prompt_request)?)
    } else {
        None
    };
    Ok(ProviderSmokeReport {
        provider_id,
        version_detected,
        help_detected,
        prompt_command_args,
        parser_event_count: parser_events.len(),
        parser_native_session_id,
        prompt_skipped: prompt_result.is_none(),
        prompt_result,
    })
}

pub fn build_gemini_args(request: &AgentPromptRequest) -> Vec<String> {
    let approval_mode = match request.execution_policy {
        AgentExecutionPolicy::Ask => "default",
        AgentExecutionPolicy::AllowProject => "auto_edit",
        AgentExecutionPolicy::Deny => "plan",
    };
    let mut args = vec![
        "--prompt".to_string(),
        request.prompt.clone(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--approval-mode".to_string(),
        approval_mode.to_string(),
        "--skip-trust".to_string(),
    ];
    if let Some(session_id) = &request.session_id {
        args.push("--resume".to_string());
        args.push(session_id.clone());
    }
    args
}

pub fn build_codex_args(request: &AgentPromptRequest) -> Vec<String> {
    let sandbox = match request.execution_policy {
        AgentExecutionPolicy::Deny => "read-only",
        AgentExecutionPolicy::Ask | AgentExecutionPolicy::AllowProject => "workspace-write",
    };
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--sandbox".to_string(),
        sandbox.to_string(),
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

pub fn build_opencode_args(request: &AgentPromptRequest) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--dir".to_string(),
        request.cwd.to_string_lossy().to_string(),
    ];
    if let Some(session_id) = &request.session_id {
        args.push("--session".to_string());
        args.push(session_id.clone());
    }
    if matches!(request.execution_policy, AgentExecutionPolicy::AllowProject) {
        args.push("--dangerously-skip-permissions".to_string());
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
            let text = find_event_text(&value);
            let kind = if looks_like_tool_call(&value) {
                AgentExecutionEventKind::ToolCall
            } else if text.is_some() {
                AgentExecutionEventKind::Output
            } else {
                AgentExecutionEventKind::Raw
            };
            Some(AgentExecutionEvent {
                text,
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
    let run = run_command_with_timeout(
        "gemini",
        &build_gemini_args(&request),
        &request.cwd,
        timeout_for(&request),
    )
    .with_context(|| "failed to run gemini prompt")?;
    Ok(agent_result_from_command_run(
        request.provider_id,
        "gemini",
        run,
    ))
}

fn run_codex_prompt(request: AgentPromptRequest) -> Result<AgentRunResult> {
    let run = run_command_with_timeout(
        "codex",
        &build_codex_args(&request),
        &request.cwd,
        timeout_for(&request),
    )
    .with_context(|| "failed to run codex prompt")?;
    Ok(agent_result_from_command_run(
        request.provider_id,
        "codex",
        run,
    ))
}

fn run_opencode_prompt(request: AgentPromptRequest) -> Result<AgentRunResult> {
    let run = run_command_with_timeout(
        "opencode",
        &build_opencode_args(&request),
        &request.cwd,
        timeout_for(&request),
    )
    .with_context(|| "failed to run opencode prompt")?;
    Ok(agent_result_from_command_run(
        request.provider_id,
        "opencode",
        run,
    ))
}

fn agent_result_from_command_run(
    provider_id: ProviderId,
    command: &str,
    run: CommandRunOutput,
) -> AgentRunResult {
    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let raw_stderr = String::from_utf8_lossy(&run.output.stderr).to_string();
    let stream_completed = stream_reports_success(&stdout);
    let success = run.output.status.success() || (run.timed_out && stream_completed);
    let stderr = if run.timed_out && !stream_completed {
        timeout_stderr(command, run.timeout, &raw_stderr)
    } else {
        raw_stderr
    };
    let error = if success {
        None
    } else if run.timed_out {
        classify_agent_error(&stderr, AgentErrorSource::Runtime)
    } else {
        classify_agent_error(&stderr, AgentErrorSource::Stderr)
    };
    AgentRunResult {
        provider_id,
        success,
        exit_code: run.output.status.code(),
        native_session_id: extract_native_session_id(&stdout),
        events: parse_stream_json_lines(&stdout),
        error,
        stderr,
    }
}

fn stream_reports_success(raw: &str) -> bool {
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .any(|value| {
            let Some(object) = value.as_object() else {
                return false;
            };
            let type_is_result = object
                .get("type")
                .and_then(Value::as_str)
                .map(|kind| kind.eq_ignore_ascii_case("result"))
                .unwrap_or(false);
            let status_is_success = object
                .get("status")
                .and_then(Value::as_str)
                .map(|status| {
                    matches!(
                        status.to_ascii_lowercase().as_str(),
                        "success" | "succeeded" | "completed"
                    )
                })
                .unwrap_or(false);
            type_is_result && status_is_success
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
) -> Result<CommandRunOutput> {
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
        return Ok(CommandRunOutput {
            output,
            timed_out: true,
            timeout,
        });
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to collect {command} output"))?;
    Ok(CommandRunOutput {
        output,
        timed_out: false,
        timeout,
    })
}

fn timeout_stderr(command: &str, timeout: Duration, stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{command} timed out after {} seconds", timeout.as_secs())
    } else {
        format!(
            "{command} timed out after {} seconds. stderr: {}",
            timeout.as_secs(),
            one_line_limit(stderr, 240)
        )
    }
}

fn one_line_limit(text: &str, max_chars: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        return one_line;
    }
    let keep = max_chars.saturating_sub(3);
    format!("{}...", one_line.chars().take(keep).collect::<String>())
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
            non_interactive_prompt: help.contains("opencode run")
                || exec_help.contains("run opencode"),
            structured_output: exec_help.contains("--format") && exec_help.contains("json"),
            acp: help.contains("opencode acp") || help.contains("start ACP"),
            session_resume: exec_help.contains("--session"),
            ..DetectedCapabilities::default()
        },
        ProviderKind::Other(_) => DetectedCapabilities::default(),
    }
}

fn build_acp_proof(profile: &ProviderProfile) -> Option<AcpProof> {
    let transport = profile
        .transports
        .iter()
        .find(|transport| matches!(transport, ProviderTransport::Acp { .. }))?;
    let ProviderTransport::Acp { command, args } = transport else {
        return None;
    };

    Some(AcpProof {
        command: command.clone(),
        args: args.clone(),
        initialize_request: baize_acp::request(
            1,
            "initialize",
            serde_json::json!({
                "client": {
                    "name": "baize",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {}
            }),
        ),
    })
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

fn find_event_text(value: &Value) -> Option<String> {
    find_codex_agent_message_text(value).or_else(|| find_text(value))
}

fn find_codex_agent_message_text(value: &Value) -> Option<String> {
    let item = value.get("item")?;
    let item_type = item.get("type").and_then(Value::as_str)?;
    if item_type != "agent_message" {
        return None;
    }
    find_text(item)
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
                "sessionID",
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

fn diagnostic_from_validation(
    profile: &ProviderProfile,
    validation: ProviderValidation,
) -> ProviderDiagnostic {
    let prompt_runtime_supported = prompt_runtime_supported(&profile.id);
    let mut issues = Vec::new();
    let mut suggested_actions = Vec::new();

    if matches!(&validation.health.status, HealthStatus::Unavailable) {
        let error = validation
            .health
            .last_error
            .as_deref()
            .filter(|error| !error.trim().is_empty())
            .unwrap_or("provider command is unavailable");
        issues.push(format!("provider executable is not ready: {error}"));
        suggested_actions.push(format!(
            "install `{}` or add it to PATH, then run `baize validate {}`",
            primary_command(profile).unwrap_or(profile.id.0.as_str()),
            profile.id.0
        ));
    }

    let command_unavailable = matches!(&validation.health.status, HealthStatus::Unavailable);
    if !command_unavailable && !validation.capability_gaps.is_empty() {
        issues.push(format!(
            "missing expected capabilities: {}",
            validation.capability_gaps.join(", ")
        ));
        suggested_actions.push(format!(
            "upgrade or reconfigure `{}` until `baize validate {}` reports no capability gaps",
            profile.display_name, profile.id.0
        ));
    }

    if !prompt_runtime_supported {
        issues.push("Baize prompt runtime is not implemented for this provider yet".to_string());
        if validation.acp_proof.is_some() {
            suggested_actions.push(
                "use the ACP proof-of-life data to continue adapter implementation".to_string(),
            );
        } else {
            suggested_actions
                .push("add an adapter runtime before routing prompts to this provider".to_string());
        }
    } else if !command_unavailable && validation.capability_gaps.is_empty() {
        suggested_actions.push(format!(
            "optional: run `baize smoke {} --run-prompt --timeout-seconds 30` to verify login and spend quota intentionally",
            profile.id.0
        ));
    }

    let readiness = if command_unavailable {
        ProviderReadiness::SetupRequired
    } else if !prompt_runtime_supported {
        ProviderReadiness::UnsupportedRuntime
    } else if !validation.capability_gaps.is_empty() {
        ProviderReadiness::CapabilityMismatch
    } else {
        ProviderReadiness::Ready
    };

    ProviderDiagnostic {
        provider_id: validation.provider_id,
        display_name: profile.display_name.clone(),
        readiness,
        health: validation.health,
        version: validation.version,
        prompt_runtime_supported,
        detected: validation.detected,
        acp_proof: validation.acp_proof,
        issues,
        suggested_actions,
    }
}

fn prompt_runtime_supported(provider_id: &ProviderId) -> bool {
    matches!(provider_id.0.as_str(), "codex" | "gemini" | "opencode")
}

fn primary_command(profile: &ProviderProfile) -> Option<&str> {
    profile.transports.first().map(command_for_transport)
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
    fn copilot_and_opencode_validation_include_acp_initialize_proof() {
        let profiles = default_provider_profiles();

        for provider_id in ["copilot", "opencode"] {
            let profile = profiles
                .iter()
                .find(|profile| profile.id.0 == provider_id)
                .expect("profile");
            let proof = build_acp_proof(profile).expect("acp proof");

            assert_eq!(proof.initialize_request.jsonrpc, "2.0");
            assert_eq!(proof.initialize_request.method, "initialize");
            assert_eq!(proof.initialize_request.params["client"]["name"], "baize");
            assert!(!proof.command.is_empty());
        }
    }

    #[test]
    fn acp_proof_serializes_with_initialize_request() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "opencode")
            .expect("opencode profile");
        let proof = build_acp_proof(&profile).expect("acp proof");

        let value = serde_json::to_value(&proof).expect("serialized proof");

        assert_eq!(value["initialize_request"]["method"], "initialize");
        assert_eq!(
            value["initialize_request"]["params"]["client"]["name"],
            "baize"
        );
        assert_eq!(value["command"], "opencode");
    }

    #[test]
    fn cli_only_provider_validation_does_not_include_acp_proof() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "codex")
            .expect("codex profile");

        assert!(build_acp_proof(&profile).is_none());
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
    fn opencode_help_detection_finds_run_json_and_resume() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "opencode")
            .expect("opencode profile");
        let help = "Commands:\n  opencode acp\n  opencode run [message..]\n";
        let run_help = "run opencode with a message\n      --format default json\n      --session session id to continue\n";

        let detected = detect_capabilities(&profile, Some(help), Some(run_help), None);

        assert!(detected.non_interactive_prompt);
        assert!(detected.structured_output);
        assert!(detected.acp);
        assert!(detected.session_resume);
    }

    #[test]
    fn clean_version_ignores_warning_lines() {
        let version = clean_version(
            "codex-cli 0.133.0\nWARNING: proceeding, even though PATH failed\n".to_string(),
        );

        assert_eq!(version, "codex-cli 0.133.0");
    }

    #[test]
    fn diagnostic_reports_setup_required_for_unavailable_provider() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "codex")
            .expect("codex profile");
        let validation = ProviderValidation {
            provider_id: profile.id.clone(),
            health: ProviderHealth {
                provider_id: profile.id.clone(),
                status: HealthStatus::Unavailable,
                latency_ms: None,
                last_error: Some("No such file or directory".to_string()),
                checked_at: Utc::now(),
            },
            version: None,
            detected: DetectedCapabilities::default(),
            acp_proof: None,
            capability_gaps: vec!["structured_output".to_string()],
        };

        let diagnostic = diagnostic_from_validation(&profile, validation);

        assert_eq!(diagnostic.readiness, ProviderReadiness::SetupRequired);
        assert!(diagnostic
            .issues
            .iter()
            .any(|issue| issue.contains("provider executable is not ready")));
        assert!(diagnostic
            .suggested_actions
            .iter()
            .any(|action| action.contains("install `codex`")));
        assert!(!diagnostic
            .issues
            .iter()
            .any(|issue| issue.contains("missing expected capabilities")));
    }

    #[test]
    fn diagnostic_reports_ready_for_opencode_prompt_runtime() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "opencode")
            .expect("opencode profile");
        let validation = ProviderValidation {
            provider_id: profile.id.clone(),
            health: ProviderHealth {
                provider_id: profile.id.clone(),
                status: HealthStatus::Healthy,
                latency_ms: Some(10),
                last_error: None,
                checked_at: Utc::now(),
            },
            version: Some("opencode 1.0.0".to_string()),
            detected: DetectedCapabilities {
                acp: true,
                non_interactive_prompt: true,
                structured_output: true,
                session_resume: true,
                ..DetectedCapabilities::default()
            },
            acp_proof: build_acp_proof(&profile),
            capability_gaps: Vec::new(),
        };

        let diagnostic = diagnostic_from_validation(&profile, validation);

        assert_eq!(diagnostic.readiness, ProviderReadiness::Ready);
        assert!(diagnostic.prompt_runtime_supported);
        assert!(diagnostic
            .suggested_actions
            .iter()
            .any(|action| action.contains("baize smoke opencode --run-prompt")));
    }

    #[test]
    fn diagnostic_reports_ready_for_supported_healthy_provider() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "gemini")
            .expect("gemini profile");
        let validation = ProviderValidation {
            provider_id: profile.id.clone(),
            health: ProviderHealth {
                provider_id: profile.id.clone(),
                status: HealthStatus::Healthy,
                latency_ms: Some(10),
                last_error: None,
                checked_at: Utc::now(),
            },
            version: Some("gemini 1.0.0".to_string()),
            detected: DetectedCapabilities {
                non_interactive_prompt: true,
                structured_output: true,
                acp: true,
                session_resume: true,
                ..DetectedCapabilities::default()
            },
            acp_proof: build_acp_proof(&profile),
            capability_gaps: Vec::new(),
        };

        let diagnostic = diagnostic_from_validation(&profile, validation);

        assert_eq!(diagnostic.readiness, ProviderReadiness::Ready);
        assert!(diagnostic.issues.is_empty());
        assert!(diagnostic
            .suggested_actions
            .iter()
            .any(|action| action.contains("baize smoke gemini --run-prompt")));
    }

    #[test]
    fn prompt_runtime_support_is_explicit() {
        assert!(is_prompt_runtime_supported(&ProviderId(
            "codex".to_string()
        )));
        assert!(is_prompt_runtime_supported(&ProviderId(
            "gemini".to_string()
        )));
        assert!(is_prompt_runtime_supported(&ProviderId(
            "opencode".to_string()
        )));
        assert!(!is_prompt_runtime_supported(&ProviderId(
            "copilot".to_string()
        )));
    }

    #[test]
    fn execution_policy_maps_from_workspace_command_policy() {
        assert_eq!(
            AgentExecutionPolicy::from_command_policy("allow_project"),
            AgentExecutionPolicy::AllowProject
        );
        assert_eq!(
            AgentExecutionPolicy::from_command_policy("deny"),
            AgentExecutionPolicy::Deny
        );
        assert_eq!(
            AgentExecutionPolicy::from_command_policy("ask"),
            AgentExecutionPolicy::Ask
        );
        assert_eq!(
            AgentExecutionPolicy::from_command_policy("unknown"),
            AgentExecutionPolicy::Ask
        );
    }

    #[test]
    fn builds_deny_gemini_stream_json_command() {
        let request = AgentPromptRequest {
            provider_id: ProviderId("gemini".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: Some("session-1".to_string()),
            timeout_seconds: None,
            execution_policy: AgentExecutionPolicy::Deny,
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
            .any(|pair| pair == ["--resume", "session-1"]));
        assert!(!args.contains(&"--session-id".to_string()));
    }

    #[test]
    fn gemini_execution_policy_controls_approval_mode() {
        let mut request = AgentPromptRequest {
            provider_id: ProviderId("gemini".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: None,
            timeout_seconds: None,
            execution_policy: AgentExecutionPolicy::Ask,
        };

        let ask_args = build_gemini_args(&request);
        assert!(ask_args
            .windows(2)
            .any(|pair| pair == ["--approval-mode", "default"]));

        request.execution_policy = AgentExecutionPolicy::AllowProject;
        let allow_args = build_gemini_args(&request);
        assert!(allow_args
            .windows(2)
            .any(|pair| pair == ["--approval-mode", "auto_edit"]));
    }

    #[test]
    fn builds_deny_codex_json_command() {
        let request = AgentPromptRequest {
            provider_id: ProviderId("codex".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: None,
            timeout_seconds: None,
            execution_policy: AgentExecutionPolicy::Deny,
        };

        let args = build_codex_args(&request);

        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_string()));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "read-only"]));
        assert!(!args.contains(&"--ask-for-approval".to_string()));
        assert_eq!(args.last().expect("prompt"), "hello");
    }

    #[test]
    fn codex_execution_policy_controls_sandbox() {
        let mut request = AgentPromptRequest {
            provider_id: ProviderId("codex".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: None,
            timeout_seconds: None,
            execution_policy: AgentExecutionPolicy::Ask,
        };

        let ask_args = build_codex_args(&request);
        assert!(ask_args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "workspace-write"]));

        request.execution_policy = AgentExecutionPolicy::AllowProject;
        let allow_args = build_codex_args(&request);
        assert!(allow_args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "workspace-write"]));
    }

    #[test]
    fn codex_resume_command_preserves_session_id_and_prompt() {
        let request = AgentPromptRequest {
            provider_id: ProviderId("codex".to_string()),
            prompt: "continue".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: Some("codex-native-1".to_string()),
            timeout_seconds: None,
            execution_policy: AgentExecutionPolicy::Ask,
        };

        let args = build_codex_args(&request);
        let resume_index = args
            .iter()
            .position(|arg| arg == "resume")
            .expect("resume command");

        assert!(args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "workspace-write"]));
        assert_eq!(args[resume_index + 1], "codex-native-1");
        assert_eq!(args.last().expect("prompt"), "continue");
    }

    #[test]
    fn builds_opencode_json_command_with_safe_default_permissions() {
        let request = AgentPromptRequest {
            provider_id: ProviderId("opencode".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: Some("session-1".to_string()),
            timeout_seconds: None,
            execution_policy: AgentExecutionPolicy::Ask,
        };

        let args = build_opencode_args(&request);

        assert_eq!(args[0], "run");
        assert!(args.windows(2).any(|pair| pair == ["--format", "json"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--dir", "/tmp/project"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session", "session-1"]));
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
        assert_eq!(args.last().expect("prompt"), "hello");
    }

    #[test]
    fn opencode_allow_project_policy_can_skip_permissions() {
        let request = AgentPromptRequest {
            provider_id: ProviderId("opencode".to_string()),
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/project"),
            session_id: None,
            timeout_seconds: None,
            execution_policy: AgentExecutionPolicy::AllowProject,
        };

        let args = build_opencode_args(&request);

        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn codex_smoke_skips_real_prompt_by_default() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "codex")
            .expect("codex profile");

        let report = smoke_provider(
            &profile,
            ProviderSmokeOptions {
                cwd: PathBuf::from("."),
                run_prompt: false,
                prompt: "baize smoke".to_string(),
                timeout_seconds: Some(5),
            },
        )
        .expect("smoke report");

        assert!(report.prompt_skipped);
        assert!(report.prompt_result.is_none());
        assert!(report.prompt_command_args.contains(&"--json".to_string()));
        assert_eq!(
            report.parser_native_session_id.as_deref(),
            Some("smoke_codex_session")
        );
        assert_eq!(report.parser_event_count, 1);
    }

    #[test]
    fn gemini_smoke_builds_stream_json_command_without_prompt_run() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "gemini")
            .expect("gemini profile");

        let report = smoke_provider(
            &profile,
            ProviderSmokeOptions {
                cwd: PathBuf::from("."),
                run_prompt: false,
                prompt: "baize smoke".to_string(),
                timeout_seconds: Some(5),
            },
        )
        .expect("smoke report");

        assert!(report.prompt_skipped);
        assert!(report
            .prompt_command_args
            .windows(2)
            .any(|pair| pair == ["--output-format", "stream-json"]));
        assert_eq!(
            report.parser_native_session_id.as_deref(),
            Some("smoke_gemini_session")
        );
        assert_eq!(report.parser_event_count, 1);
    }

    #[test]
    fn opencode_smoke_builds_json_command_without_prompt_run() {
        let profile = default_provider_profiles()
            .into_iter()
            .find(|profile| profile.id.0 == "opencode")
            .expect("opencode profile");

        let report = smoke_provider(
            &profile,
            ProviderSmokeOptions {
                cwd: PathBuf::from("."),
                run_prompt: false,
                prompt: "baize smoke".to_string(),
                timeout_seconds: Some(5),
            },
        )
        .expect("smoke report");

        assert!(report.prompt_skipped);
        assert!(report
            .prompt_command_args
            .windows(2)
            .any(|pair| pair == ["--format", "json"]));
        assert_eq!(
            report.parser_native_session_id.as_deref(),
            Some("smoke_opencode_session")
        );
        assert_eq!(report.parser_event_count, 1);
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
    fn parses_codex_agent_message_item_as_output() {
        let raw = r#"
        {"type":"thread.started","thread_id":"thread_1"}
        {"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"baize-smoke"}}
        "#;

        let events = parse_stream_json_lines(raw);

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0].kind, AgentExecutionEventKind::Raw));
        assert!(matches!(events[1].kind, AgentExecutionEventKind::Output));
        assert_eq!(events[1].text.as_deref(), Some("baize-smoke"));
    }

    #[test]
    fn detects_terminal_success_result_in_stream_json() {
        let raw = r#"
        {"type":"message","content":"working"}
        {"type":"result","status":"success","stats":{"total_tokens":42}}
        "#;

        assert!(stream_reports_success(raw));
        assert!(!stream_reports_success(
            r#"{"type":"message","content":"success is just text"}"#
        ));
        assert!(!stream_reports_success(
            r#"{"type":"result","status":"failed"}"#
        ));
    }

    #[test]
    fn timeout_after_terminal_success_is_reported_as_success() {
        let script = r#"printf '%s\n' '{"type":"message","content":"done"}' '{"type":"result","status":"success"}'; sleep 2"#;
        let run = run_command_with_timeout(
            "sh",
            &["-c".to_string(), script.to_string()],
            &PathBuf::from("."),
            Duration::from_millis(20),
        )
        .expect("timeout run");

        let result = agent_result_from_command_run(ProviderId("gemini".to_string()), "gemini", run);

        assert!(result.success);
        assert!(result.error.is_none());
        assert!(!result.stderr.contains("timed out"));
        assert!(result
            .events
            .iter()
            .any(|event| event.text.as_deref() == Some("done")));
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
    fn command_timeout_returns_partial_output_without_runtime_error() {
        let run = run_command_with_timeout(
            "sleep",
            &["2".to_string()],
            &PathBuf::from("."),
            Duration::from_millis(10),
        )
        .expect("timeout run");

        assert!(run.timed_out);
        assert!(!run.output.status.success());
    }

    #[test]
    fn timeout_stderr_is_concise() {
        let message = timeout_stderr(
            "gemini",
            Duration::from_secs(10),
            "stderr line\nwith more detail",
        );

        assert_eq!(
            message,
            "gemini timed out after 10 seconds. stderr: stderr line with more detail"
        );
        assert!(!message.contains("stdout:"));
    }
}
