use baize_core::{
    HealthStatus, ProviderCapabilities, ProviderHealth, ProviderId, ProviderKind, ProviderProfile,
    ProviderTransport,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Instant;

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
        .filter(|line| !line.to_ascii_lowercase().starts_with("warning:"))
        .last()
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
}
