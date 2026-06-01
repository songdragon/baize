use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{PermissionId, PermissionRequest, PermissionStatus};
use chrono::Utc;

use crate::helpers::{
    bad_request, internal_error, json_result_option, ok_json, parse_permission_status,
    permission_status_eq, with_store,
};
use crate::state::{AppState, CreatePermissionRequest, PermissionsQuery};

pub async fn permissions(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<PermissionsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let permissions = match with_store(&state, |store| store.list_permissions()) {
        Ok(permissions) => permissions,
        Err(error) => return internal_error(error.to_string()),
    };
    let status = match query.status.as_deref().map(parse_permission_status) {
        Some(Some(status)) => Some(status),
        Some(None) => return bad_request("invalid permission status"),
        None => None,
    };
    let session_id = query.session_id.as_deref();
    let permissions = permissions
        .into_iter()
        .filter(|permission| {
            status
                .as_ref()
                .is_none_or(|status| permission_status_eq(&permission.status, status))
        })
        .filter(|permission| {
            session_id.is_none_or(|session_id| {
                permission
                    .session_id
                    .as_ref()
                    .is_some_and(|permission_session| permission_session.0 == session_id)
            })
        })
        .collect::<Vec<_>>();

    ok_json(serde_json::json!({ "permissions": permissions }))
}

pub async fn create_permission(
    State(state): State<AppState>,
    Json(request): Json<CreatePermissionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let permission = PermissionRequest {
        id: PermissionId::new(),
        workspace_id: request.workspace_id.map(baize_core::WorkspaceId),
        session_id: request.session_id.map(baize_core::TaskSessionId),
        command: request.command,
        reason: request.reason,
        status: PermissionStatus::Pending,
        created_at: Utc::now(),
        resolved_at: None,
    };
    if let Err(error) = with_store(&state, |store| store.upsert_permission(&permission)) {
        return internal_error(error.to_string());
    }
    let mut event = baize_core::BaizeEvent::new(
        "permission.requested",
        serde_json::json!({ "permission": permission }),
    );
    event.workspace_id = permission.workspace_id.clone();
    event.session_id = permission.session_id.clone();
    state.record_event(event);
    ok_json(serde_json::json!({ "permission": permission }))
}

pub async fn permission(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let permission = with_store(&state, |store| store.get_permission(&PermissionId(id)));
    json_result_option("permission", permission)
}

pub async fn approve_permission(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    resolve_permission(state, PermissionId(id), PermissionStatus::Approved)
}

pub async fn deny_permission(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    resolve_permission(state, PermissionId(id), PermissionStatus::Denied)
}

fn resolve_permission(
    state: AppState,
    id: PermissionId,
    status: PermissionStatus,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut permission = match with_store(&state, |store| store.get_permission(&id)) {
        Ok(Some(permission)) => permission,
        Ok(None) => return crate::helpers::not_found("permission not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    permission.status = status;
    permission.resolved_at = Some(Utc::now());
    if let Err(error) = with_store(&state, |store| store.upsert_permission(&permission)) {
        return internal_error(error.to_string());
    }
    let mut event = baize_core::BaizeEvent::new(
        "permission.resolved",
        serde_json::json!({ "permission": permission }),
    );
    event.workspace_id = permission.workspace_id.clone();
    event.session_id = permission.session_id.clone();
    state.record_event(event);
    ok_json(serde_json::json!({ "permission": permission }))
}
