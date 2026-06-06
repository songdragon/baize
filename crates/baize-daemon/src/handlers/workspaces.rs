use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{Project, ProjectId, ProjectKind, TrustLevel, VcsKind, Workspace, WorkspaceId};
use chrono::Utc;

use crate::helpers::{bad_request, internal_error, json_result, not_found, ok_json, with_store};
use crate::state::{AppState, CreateWorkspaceRequest, WorkspaceStatusQuery};

pub async fn workspaces(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let workspaces = with_store(&state, |store| store.list_workspaces());
    json_result("workspaces", workspaces)
}

pub async fn create_workspace(
    State(state): State<AppState>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let status = match baize_workspace::inspect(&request.path) {
        Ok(status) => status,
        Err(error) => return bad_request(&error.to_string()),
    };
    let now = Utc::now();
    let workspace_id = WorkspaceId::new();
    let project_id = ProjectId::new();
    let name = request.name.unwrap_or_else(|| {
        status
            .root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string())
    });
    let workspace = Workspace {
        id: workspace_id.clone(),
        name,
        primary_project_id: project_id.clone(),
        created_at: now,
        updated_at: now,
    };
    let project = Project {
        id: project_id,
        workspace_id: workspace_id.clone(),
        root: status.git_root.clone().unwrap_or(status.root.clone()),
        kind: if status.git_root.is_some() {
            ProjectKind::GitRepo
        } else {
            ProjectKind::Directory
        },
        vcs: if status.git_root.is_some() {
            VcsKind::Git
        } else {
            VcsKind::None
        },
        active_branch: status.branch,
        trust_level: TrustLevel::Trusted,
        created_at: now,
        updated_at: now,
    };

    let save = with_store(&state, |store| {
        store.upsert_workspace(&workspace)?;
        store.upsert_project(&project)?;
        Ok(())
    });
    if let Err(error) = save {
        return internal_error(error.to_string());
    }

    let mut event = baize_core::BaizeEvent::new(
        "workspace.status.changed",
        serde_json::json!({ "workspace": workspace, "project": project }),
    );
    event.workspace_id = Some(workspace_id);
    state.record_event(event);
    ok_json(serde_json::json!({ "workspace": workspace, "project": project }))
}

pub async fn workspace_status(
    axum::extract::Query(query): axum::extract::Query<WorkspaceStatusQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let path = query.path.unwrap_or_else(|| std::path::PathBuf::from("."));
    match baize_workspace::inspect(path) {
        Ok(status) => ok_json(serde_json::json!({ "status": status })),
        Err(error) => bad_request(&error.to_string()),
    }
}

pub async fn workspace_status_by_id(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let workspace = match with_store(&state, |store| store.get_workspace(&WorkspaceId(id))) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return not_found("workspace not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        Ok(None) => return not_found("project not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    workspace_status(axum::extract::Query(WorkspaceStatusQuery {
        path: Some(project.root),
    }))
    .await
}

pub async fn workspace_projects(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let workspace_id = WorkspaceId(id);
    let workspace = match with_store(&state, |store| store.get_workspace(&workspace_id)) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return not_found("workspace not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    let projects = with_store(&state, |store| {
        store.list_projects_for_workspace(&workspace.id)
    });
    json_result("projects", projects)
}
