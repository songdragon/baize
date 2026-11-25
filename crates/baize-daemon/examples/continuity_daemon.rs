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
    config.workspace.command_policy =
        acceptance_policy(std::env::var("BAIZE_ACCEPTANCE_POLICY").ok().as_deref())?.into();
    baize_daemon::run(config).await
}

fn acceptance_policy(policy: Option<&str>) -> Result<&str> {
    let policy = policy.unwrap_or("allow_project");
    anyhow::ensure!(
        matches!(policy, "ask" | "allow_project"),
        "Acceptance policy must be ask or allow_project"
    );
    Ok(policy)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_the_explicitly_supported_acceptance_policies() {
        assert_eq!(acceptance_policy(None).unwrap(), "allow_project");
        assert_eq!(acceptance_policy(Some("ask")).unwrap(), "ask");
        assert!(acceptance_policy(Some("danger-full-access")).is_err());
    }
}
