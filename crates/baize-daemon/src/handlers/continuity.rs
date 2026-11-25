use crate::{
    helpers::{bad_request, internal_error, not_found, ok_json, with_store},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use baize_core::TaskSessionId;

pub async fn requirements(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_store(&state, |store| store.get_task_session(&TaskSessionId(id))) {
        Ok(Some(mut session)) => {
            if let Err(error) = crate::continuity::import_requirements(&state, &mut session) {
                return internal_error(error.to_string());
            }
            ok_json(serde_json::json!({"continuity":session.continuity}))
        }
        Ok(None) => not_found("session not found"),
        Err(error) => internal_error(error.to_string()),
    }
}

pub async fn update_requirement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(command): Json<crate::continuity::ContinuityCommand>,
) -> (StatusCode, Json<serde_json::Value>) {
    match crate::continuity::apply_command(
        &state,
        &TaskSessionId(id),
        command,
        "Explicit user continuity command via API",
    ) {
        Ok(value) => ok_json(value),
        Err(error) => bad_request(&error.to_string()),
    }
}
