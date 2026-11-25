//! Isolated opt-in acceptance daemon. Never reads or modifies the user's config.
//! Run only through scripts/continuity_acceptance.py (which gates real prompts).
use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = baize_config::BaizeConfig::default();
    config.daemon.port = std::env::var("BAIZE_ACCEPTANCE_PORT")
        .context("BAIZE_ACCEPTANCE_PORT must be set by the acceptance runner")?
        .parse()
        .context("invalid acceptance port")?;
    anyhow::ensure!(
        std::env::var_os("BAIZE_DATA_DIR").is_some(),
        "An isolated BAIZE_DATA_DIR is required"
    );
    anyhow::ensure!(
        std::env::var("BAIZE_ACCEPTANCE_ALLOW_PROJECT").as_deref() == Ok("1"),
        "Real editing acceptance requires explicit project-execution authorization"
    );
    config.workspace.command_policy = "allow_project".into();
    baize_daemon::run(config).await
}
