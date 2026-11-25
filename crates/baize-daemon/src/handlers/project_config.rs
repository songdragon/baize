use crate::{
    helpers::{bad_request, internal_error, not_found, ok_json, with_store},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use baize_core::{BaizeEvent, WorkspaceId};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRequest {
    action: String,
    #[serde(default)]
    replace: Vec<String>,
    plan_id: Option<String>,
    backup_id: Option<String>,
}

pub async fn inspect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> (StatusCode, Json<Value>) {
    execute(
        &state,
        &WorkspaceId(id),
        ConfigRequest {
            action: "plan".into(),
            replace: vec![],
            plan_id: None,
            backup_id: None,
        },
    )
}

pub async fn change(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ConfigRequest>,
) -> (StatusCode, Json<Value>) {
    execute(&state, &WorkspaceId(id), request)
}

fn execute(
    state: &AppState,
    id: &WorkspaceId,
    request: ConfigRequest,
) -> (StatusCode, Json<Value>) {
    if !matches!(
        request.action.as_str(),
        "plan" | "apply" | "restore" | "history"
    ) {
        return bad_request("Expected action plan, apply, history or restore");
    }
    if (request.action == "apply" && request.plan_id.as_deref().is_none_or(str::is_empty))
        || (request.action == "restore" && request.backup_id.as_deref().is_none_or(str::is_empty))
    {
        return bad_request("Apply requires plan_id; restore requires backup_id");
    }
    if request.replace.len() > 128 {
        return bad_request("Too many replacement names");
    }
    let root = match with_store(state, |store| {
        let Some(workspace) = store.get_workspace(id)? else {
            return Ok(None);
        };
        Ok(store
            .get_primary_project(&workspace)?
            .map(|project| project.root))
    }) {
        Ok(Some(root)) => root,
        Ok(None) => return not_found("Workspace or project not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    let result = with_store(state, |store| {
        if request.action == "history" {
            return Ok(
                json!({"backups":baize_config::project::history(&root,&store.project_config_backup_dir())?,"preview_only":true}),
            );
        }
        if request.action == "plan" {
            return Ok(
                json!({"project_config":baize_config::project::plan(&root,&request.replace)?,"preview_only":true}),
            );
        }
        anyhow::ensure!(!store.has_active_execution_in_workspace(id)?,"Workspace has a running or uncertain execution; wait for confirmed stop or reconcile before changing provider configuration");
        let backup_root = store.project_config_backup_dir();
        let change = if request.action == "apply" {
            baize_config::project::apply(
                &root,
                request.plan_id.as_deref().unwrap_or_default(),
                &request.replace,
                &backup_root,
            )?
        } else {
            baize_config::project::restore(
                &root,
                request.backup_id.as_deref().unwrap_or_default(),
                &backup_root,
            )?
        };
        Ok(
            json!({"change":change,"project_config":baize_config::project::plan(&root,&[]).ok(),"preview_only":false}),
        )
    });
    match result {
        Ok(value) => {
            if matches!(request.action.as_str(), "apply" | "restore") {
                let mut event = BaizeEvent::new(
                    "workspace.config.changed",
                    json!({"action":request.action,"change":value["change"]}),
                );
                event.workspace_id = Some(id.clone());
                state.record_event(event);
            }
            ok_json(value)
        }
        Err(error) if matches!(request.action.as_str(), "plan" | "history") => {
            bad_request(&error.to_string())
        }
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({"error":error.to_string()})),
        ),
    }
}
