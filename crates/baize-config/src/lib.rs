use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
    toml::from_str(&raw).with_context(|| format!("failed to parse config at {}", path.display()))
}
