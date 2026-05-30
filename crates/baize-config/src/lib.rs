use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const KNOWN_PROVIDERS: &[&str] = &["codex", "gemini", "copilot", "opencode"];
const COMMAND_POLICIES: &[&str] = &["ask", "allow_project", "deny"];
const CHECKPOINT_POLICIES: &[&str] = &["before_handoff", "off"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaizeConfig {
    pub daemon: DaemonConfig,
    pub workspace: WorkspaceConfig,
    pub providers: ProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub command_policy: String,
    pub checkpoint_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub order: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigValidation {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Default for BaizeConfig {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig {
                host: "127.0.0.1".to_string(),
                port: 7878,
            },
            workspace: WorkspaceConfig {
                command_policy: "ask".to_string(),
                checkpoint_policy: "before_handoff".to_string(),
            },
            providers: ProviderConfig {
                order: vec![
                    "codex".to_string(),
                    "gemini".to_string(),
                    "copilot".to_string(),
                    "opencode".to_string(),
                ],
            },
        }
    }
}

pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("baize")
        .join("config.toml")
}

pub fn load_or_default() -> Result<BaizeConfig> {
    let path = default_config_path();
    if !path.exists() {
        return Ok(BaizeConfig::default());
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    parse_config(&raw).with_context(|| format!("failed to parse config at {}", path.display()))
}

pub fn parse_config(raw: &str) -> Result<BaizeConfig> {
    toml::from_str(raw).context("failed to parse config")
}

pub fn default_config_toml() -> Result<String> {
    toml::to_string_pretty(&BaizeConfig::default()).context("failed to serialize default config")
}

pub fn init_default_config(force: bool) -> Result<PathBuf> {
    let path = default_config_path();
    write_default_config(path, force)
}

pub fn write_default_config(path: impl AsRef<Path>, force: bool) -> Result<PathBuf> {
    let path = path.as_ref();
    if path.exists() && !force {
        bail!("config already exists at {}", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, default_config_toml()?)
        .with_context(|| format!("failed to write config at {}", path.display()))?;
    Ok(path.to_path_buf())
}

pub fn validate_config(config: &BaizeConfig) -> ConfigValidation {
    let mut errors = Vec::new();
    let warnings = Vec::new();

    if config.daemon.host.trim().is_empty() {
        errors.push("daemon.host must not be empty".to_string());
    }
    if !COMMAND_POLICIES.contains(&config.workspace.command_policy.as_str()) {
        errors.push(format!(
            "workspace.command_policy must be one of: {}",
            COMMAND_POLICIES.join(", ")
        ));
    }
    if !CHECKPOINT_POLICIES.contains(&config.workspace.checkpoint_policy.as_str()) {
        errors.push(format!(
            "workspace.checkpoint_policy must be one of: {}",
            CHECKPOINT_POLICIES.join(", ")
        ));
    }
    if config.providers.order.is_empty() {
        errors.push("providers.order must not be empty".to_string());
    }

    let mut seen = HashSet::new();
    for provider in &config.providers.order {
        if !seen.insert(provider.as_str()) {
            errors.push(format!(
                "providers.order contains duplicate provider: {provider}"
            ));
        }
        if !KNOWN_PROVIDERS.contains(&provider.as_str()) {
            errors.push(format!(
                "providers.order contains unknown provider: {provider}"
            ));
        }
    }

    ConfigValidation {
        valid: errors.is_empty(),
        errors,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_codex_gemini_first() {
        let config = BaizeConfig::default();

        assert_eq!(config.daemon.host, "127.0.0.1");
        assert_eq!(config.daemon.port, 7878);
        assert_eq!(config.providers.order[0], "codex");
        assert_eq!(config.providers.order[1], "gemini");
        assert_eq!(config.workspace.command_policy, "ask");
    }

    #[test]
    fn parses_toml_config() {
        let config = parse_config(
            r#"
            [daemon]
            host = "0.0.0.0"
            port = 9000

            [workspace]
            command_policy = "allow_project"
            checkpoint_policy = "off"

            [providers]
            order = ["gemini", "codex"]
            "#,
        )
        .expect("config should parse");

        assert_eq!(config.daemon.host, "0.0.0.0");
        assert_eq!(config.daemon.port, 9000);
        assert_eq!(config.workspace.command_policy, "allow_project");
        assert_eq!(config.providers.order, vec!["gemini", "codex"]);
    }

    #[test]
    fn default_config_toml_round_trips() {
        let raw = default_config_toml().expect("toml");
        let config = parse_config(&raw).expect("parse");

        assert_eq!(config.daemon.host, "127.0.0.1");
        assert_eq!(config.providers.order[0], "codex");
        assert!(raw.contains("[providers]"));
    }

    #[test]
    fn write_default_config_refuses_to_overwrite_without_force() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        write_default_config(&path, false).expect("write");
        let error = write_default_config(&path, false).expect_err("overwrite should fail");

        assert!(error.to_string().contains("config already exists"));
    }

    #[test]
    fn write_default_config_overwrites_with_force() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "not toml").expect("seed");

        write_default_config(&path, true).expect("overwrite");

        let config = parse_config(&fs::read_to_string(path).expect("read")).expect("parse");
        assert_eq!(config.daemon.port, 7878);
    }

    #[test]
    fn validates_default_config() {
        let validation = validate_config(&BaizeConfig::default());

        assert!(validation.valid);
        assert!(validation.errors.is_empty());
    }

    #[test]
    fn validation_reports_invalid_values() {
        let mut config = BaizeConfig::default();
        config.daemon.host = " ".to_string();
        config.workspace.command_policy = "always".to_string();
        config.workspace.checkpoint_policy = "sometimes".to_string();
        config.providers.order = vec![
            "codex".to_string(),
            "codex".to_string(),
            "unknown".to_string(),
        ];

        let validation = validate_config(&config);

        assert!(!validation.valid);
        assert!(validation
            .errors
            .iter()
            .any(|error| error.contains("daemon.host")));
        assert!(validation
            .errors
            .iter()
            .any(|error| error.contains("duplicate provider: codex")));
        assert!(validation
            .errors
            .iter()
            .any(|error| error.contains("unknown provider: unknown")));
    }
}
