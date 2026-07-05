use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{Project, ProjectId, ProjectKind, TrustLevel, VcsKind, Workspace, WorkspaceId};
use chrono::Utc;

use crate::helpers::{bad_request, internal_error, not_found, ok_json, with_store};
use crate::state::{
    AppState, CreateWorkspaceRequest, WorkspaceProjectsQuery, WorkspaceStatusQuery, WorkspacesQuery,
};

pub async fn workspaces(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<WorkspacesQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let workspaces = match with_store(&state, |store| {
        if let Some(name) = &query.name {
            store.list_workspaces_by_name(name)
        } else if let Some(project_id) = &query.primary_project_id {
            store
                .get_workspace_by_primary_project(&ProjectId(project_id.clone()))
                .map(|workspace| workspace.into_iter().collect())
        } else {
            store.list_workspaces()
        }
    }) {
        Ok(workspaces) => workspaces,
        Err(error) => return internal_error(error.to_string()),
    };

    ok_json(serde_json::json!({ "workspaces": workspaces }))
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
    let root = status.git_root.clone().unwrap_or(status.root.clone());

    if let Some((workspace, project)) = match with_store(&state, |store| {
        let Some(mut project) = store.get_project_by_root(&root)? else {
            return Ok(None);
        };
        let Some(mut workspace) = store.get_workspace(&project.workspace_id)? else {
            anyhow::bail!(
                "workspace {} not found for existing project",
                project.workspace_id.0
            );
        };

        workspace.updated_at = now;
        project.active_branch = status.branch.clone();
        project.kind = if status.git_root.is_some() {
            ProjectKind::GitRepo
        } else {
            ProjectKind::Directory
        };
        project.vcs = if status.git_root.is_some() {
            VcsKind::Git
        } else {
            VcsKind::None
        };
        project.updated_at = now;
        store.upsert_workspace(&workspace)?;
        store.upsert_project(&project)?;
        Ok(Some((workspace, project)))
    }) {
        Ok(existing) => existing,
        Err(error) => return internal_error(error.to_string()),
    } {
        let mut event = baize_core::BaizeEvent::new(
            "workspace.status.changed",
            serde_json::json!({ "workspace": workspace, "project": project, "reused": true }),
        );
        event.workspace_id = Some(workspace.id.clone());
        state.record_event(event);
        return ok_json(serde_json::json!({
            "workspace": workspace,
            "project": project,
            "reused": true,
        }));
    }

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
        root,
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
    axum::extract::Query(query): axum::extract::Query<WorkspaceProjectsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let workspace_id = WorkspaceId(id);
    let workspace = match with_store(&state, |store| store.get_workspace(&workspace_id)) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return not_found("workspace not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    let kind = match query.kind.as_deref().map(normalize_project_kind) {
        Some(Some(kind)) => Some(kind),
        Some(None) => return bad_request("invalid project kind"),
        None => None,
    };
    let vcs = match query.vcs.as_deref().map(normalize_vcs_kind) {
        Some(Some(vcs)) => Some(vcs),
        Some(None) => return bad_request("invalid project vcs"),
        None => None,
    };
    let projects = match with_store(&state, |store| {
        if let Some(kind) = kind {
            store.list_projects_by_kind(kind)
        } else if let Some(vcs) = vcs {
            store.list_projects_by_vcs(vcs)
        } else {
            store.list_projects_for_workspace(&workspace.id)
        }
    }) {
        Ok(projects) => projects,
        Err(error) => return internal_error(error.to_string()),
    };
    let projects = projects
        .into_iter()
        .filter(|project| project.workspace_id.0 == workspace.id.0)
        .filter(|project| {
            kind.is_none_or(|kind| project_kind_eq(&project.kind, kind))
                && vcs.is_none_or(|vcs| vcs_kind_eq(&project.vcs, vcs))
        })
        .collect::<Vec<_>>();

    ok_json(serde_json::json!({ "projects": projects }))
}

fn normalize_project_kind(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().replace('-', "_").as_str() {
        "gitrepo" | "git_repo" => Some("GitRepo"),
        "directory" => Some("Directory"),
        _ => None,
    }
}

fn normalize_vcs_kind(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "git" => Some("Git"),
        "none" => Some("None"),
        _ => None,
    }
}

fn project_kind_eq(kind: &ProjectKind, label: &str) -> bool {
    matches!(
        (kind, label),
        (ProjectKind::GitRepo, "GitRepo") | (ProjectKind::Directory, "Directory")
    )
}

fn vcs_kind_eq(vcs: &VcsKind, label: &str) -> bool {
    matches!(
        (vcs, label),
        (VcsKind::Git, "Git") | (VcsKind::None, "None")
    )
}
