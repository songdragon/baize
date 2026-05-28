use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
}
