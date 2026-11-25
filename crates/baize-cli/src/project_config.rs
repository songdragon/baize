use super::*;

#[derive(Debug, Subcommand)]
pub(super) enum ProjectConfigCommand {
    History,
    Plan {
        #[arg(long)]
        replace: Vec<String>,
    },
    Apply {
        plan_id: String,
        #[arg(long)]
        replace: Vec<String>,
    },
    Restore {
        backup_id: String,
    },
}

fn body(command: ProjectConfigCommand) -> Value {
    match command {
        ProjectConfigCommand::History => json!({"action":"history"}),
        ProjectConfigCommand::Plan { replace } => json!({"action":"plan","replace":replace}),
        ProjectConfigCommand::Apply { plan_id, replace } => {
            json!({"action":"apply","plan_id":plan_id,"replace":replace})
        }
        ProjectConfigCommand::Restore { backup_id } => {
            json!({"action":"restore","backup_id":backup_id})
        }
    }
}

pub(super) fn output(path: &str, command: ProjectConfigCommand) -> Result<String> {
    let config = baize_config::load_or_default()?;
    let client = DaemonClient {
        host: config.daemon.host,
        port: config.daemon.port,
    };
    require_runtime(&client.get_json("/runtime/status")?)?;
    let value = client.post_json("/workspaces", json!({"path":path}))?;
    let id = value["workspace"]["id"]
        .as_str()
        .context("Missing workspace ID")?;
    format_output(&client.post_json(&format!("/workspaces/{id}/project-config"), body(command))?)
}

fn require_runtime(value: &Value) -> Result<()> {
    anyhow::ensure!(value["runtime"]["capabilities"]["project_config"].as_u64().unwrap_or(0)>=1,
        "Daemon does not support project configuration. Restart it with the current binary; no project configuration was changed");
    Ok(())
}

fn format_output(value: &Value) -> Result<String> {
    anyhow::ensure!(
        value["project_config"].is_object()
            || value["change"].is_object()
            || value["backups"].is_array(),
        "Missing project configuration response"
    );
    Ok(format!("{}\n", serde_json::to_string_pretty(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_routes_plan_apply_and_restore_without_running_them() {
        for args in [
            vec!["baize", "project-config", "--path", "fixture", "plan"],
            vec![
                "baize",
                "project-config",
                "--path",
                "fixture",
                "apply",
                "digest",
                "--replace",
                "server",
            ],
            vec![
                "baize",
                "project-config",
                "--path",
                "fixture",
                "restore",
                "backup",
            ],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(
                matches!(plan_cli_action(cli.command).unwrap(),CliAction::RunProjectConfig {path,..} if path=="fixture")
            );
        }
        assert!(Cli::try_parse_from(["baize", "project-config", "apply"]).is_err());
    }

    #[test]
    fn constructs_bound_requests_and_formats_only_valid_responses() {
        assert_eq!(
            body(ProjectConfigCommand::Apply {
                plan_id: "reviewed".into(),
                replace: vec!["server".into()]
            }),
            json!({"action":"apply","plan_id":"reviewed","replace":["server"]})
        );
        let response = json!({"project_config":{"status":"blocked","verification":"configured_only_not_runtime_verified"},"preview_only":true});
        assert_eq!(
            serde_json::from_str::<Value>(&format_output(&response).unwrap()).unwrap(),
            response
        );
        assert!(format_output(&json!({"unexpected":true})).is_err());
        assert!(require_runtime(&json!({"runtime":{"capabilities":{"project_config":1}}})).is_ok());
        assert!(
            require_runtime(&json!({"runtime":{"capabilities":{"session_continuity":3}}})).is_err()
        );
    }
}
