use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::helpers::ok_json;
use crate::state::AppState;

pub async fn runtime_status(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let command_policy = state.config.workspace.command_policy.as_str();
    ok_json(serde_json::json!({
        "runtime": {
            "command_policy": command_policy,
            "execution_mode": execution_mode_label(command_policy),
            "checkpoint_policy": state.config.workspace.checkpoint_policy,
            "routing": {
                "sticky_window_minutes": state.config.routing.sticky_window_minutes,
                "quota_switch_threshold_percent": state.config.routing.quota_switch_threshold_percent,
                "failure_threshold_count": state.config.routing.failure_threshold_count,
            }
        }
    }))
}

fn execution_mode_label(command_policy: &str) -> &'static str {
    match command_policy {
        "deny" => "read_only",
        "allow_project" => "project_write_allowed",
        _ => "ask_before_commands",
    }
}
