use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;

use crate::handlers;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::providers::health))
        .route("/runtime/status", get(handlers::runtime::runtime_status))
        .route(
            "/workspaces",
            get(handlers::workspaces::workspaces).post(handlers::workspaces::create_workspace),
        )
        .route(
            "/workspaces/:id/status",
            get(handlers::workspaces::workspace_status_by_id),
        )
        .route(
            "/workspaces/:id/projects",
            get(handlers::workspaces::workspace_projects),
        )
        .route("/projects/:id", get(handlers::projects::project))
        .route("/providers", get(handlers::providers::providers))
        .route(
            "/providers/:id/health",
            get(handlers::providers::provider_health),
        )
        .route(
            "/providers/:id/validate",
            get(handlers::providers::provider_validate),
        )
        .route(
            "/providers/:id/diagnose",
            get(handlers::providers::provider_diagnose),
        )
        .route(
            "/providers/check",
            post(handlers::providers::check_providers),
        )
        .route(
            "/providers/validate",
            post(handlers::providers::validate_providers),
        )
        .route(
            "/providers/diagnose",
            post(handlers::providers::diagnose_providers),
        )
        .route(
            "/workspaces/status",
            get(handlers::workspaces::workspace_status),
        )
        .route(
            "/sessions",
            get(handlers::sessions::sessions).post(handlers::sessions::create_session),
        )
        .route("/sessions/:id", get(handlers::sessions::session))
        .route(
            "/sessions/:id/prompt",
            post(handlers::sessions::prompt_session),
        )
        .route(
            "/sessions/:id/cancel",
            post(handlers::sessions::cancel_session),
        )
        .route(
            "/sessions/:id/routes",
            get(handlers::sessions::session_routes),
        )
        .route(
            "/sessions/:id/handoffs",
            get(handlers::sessions::session_handoffs),
        )
        .route(
            "/sessions/:id/permissions",
            get(handlers::sessions::session_permissions),
        )
        .route(
            "/sessions/:id/handoff",
            post(handlers::handoffs::create_handoff),
        )
        .route(
            "/sessions/:id/handoff/:handoff_id/accept",
            post(handlers::handoffs::accept_handoff),
        )
        .route(
            "/sessions/:id/events",
            get(handlers::sessions::session_events),
        )
        .route("/sessions/:id/diff", get(handlers::sessions::session_diff))
        .route(
            "/sessions/:id/handoff/:handoff_id",
            get(handlers::handoffs::handoff),
        )
        .route(
            "/permissions",
            get(handlers::permissions::permissions).post(handlers::permissions::create_permission),
        )
        .route("/permissions/:id", get(handlers::permissions::permission))
        .route(
            "/permissions/:id/approve",
            post(handlers::permissions::approve_permission),
        )
        .route(
            "/permissions/:id/deny",
            post(handlers::permissions::deny_permission),
        )
        .route("/events", get(handlers::events::events))
        .route("/events/history", get(handlers::events::history))
        .with_state(state)
        .layer(CorsLayer::permissive())
}
