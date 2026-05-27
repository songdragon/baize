use baize_core::{
    HealthStatus, ProviderCapabilities, ProviderHealth, ProviderId, ProviderKind, ProviderProfile,
    ProviderTransport,
};
use chrono::Utc;
use std::process::Command;
use std::time::Instant;

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
                shell_access: true,
                ..ProviderCapabilities::default()
            },
            enabled: true,
        },
        ProviderProfile {
            id: ProviderId("gemini".to_string()),
            kind: ProviderKind::Gemini,
            display_name: "Gemini CLI".to_string(),
            priority: 2,
            transports: vec![ProviderTransport::Cli {
                command: "gemini".to_string(),
                args: vec![],
            }],
            capabilities: ProviderCapabilities {
                interactive_chat: true,
                shell_access: true,
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
