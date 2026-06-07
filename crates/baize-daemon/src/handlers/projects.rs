use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::ProjectId;

use crate::helpers::{json_result_option, with_store};
use crate::state::AppState;

pub async fn project(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let project = with_store(&state, |store| store.get_project(&ProjectId(id)));
    json_result_option("project", project)
}
