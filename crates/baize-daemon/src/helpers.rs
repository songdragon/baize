use anyhow::Result;
use axum::http::StatusCode;
use axum::Json;
use baize_adapters::default_provider_profiles;
use baize_config::BaizeConfig;
use baize_core::PermissionStatus;
use baize_storage::EventStore;
use chrono::Utc;
use serde::Serialize;

use crate::state::AppState;

const STICKY_WINDOW_MINUTES: i64 = 30;

pub struct RoutingResult {
    pub provider_id: baize_core::ProviderId,
    pub previous_provider_id: Option<baize_core::ProviderId>,
    pub reason: String,
    pub confidence: f32,
}

pub fn with_store<T>(state: &AppState, f: impl FnOnce(&EventStore) -> Result<T>) -> Result<T> {
    let store = state
        .store
        .lock()
        .map_err(|_| anyhow::anyhow!("storage lock poisoned"))?;
    f(&store)
}

pub fn json_result<T: Serialize>(
    key: &str,
    result: Result<T>,
) -> (StatusCode, Json<serde_json::Value>) {
    match result {
        Ok(value) => (StatusCode::OK, Json(serde_json::json!({ key: value }))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

pub fn json_result_option<T: Serialize>(
    key: &str,
    result: Result<Option<T>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match result {
        Ok(Some(value)) => (StatusCode::OK, Json(serde_json::json!({ key: value }))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("{key} not found") })),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

pub fn not_found(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": message })),
    )
}

pub fn bad_request(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message })),
    )
}

pub fn internal_error(message: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": message })),
    )
}

pub fn ok_json(value: serde_json::Value) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(value))
}

pub fn select_provider(
    state: &AppState,
    requested: Option<String>,
    workspace_id: Option<&baize_core::WorkspaceId>,
) -> RoutingResult {
    if let Some(requested) = requested {
        return RoutingResult {
            provider_id: baize_core::ProviderId(requested),
            previous_provider_id: None,
            reason: "User-specified provider override.".to_string(),
            confidence: 1.0,
        };
    }

    if let Some(wid) = workspace_id {
        if let Some(sticky) = try_sticky_provider(state, wid) {
            return sticky;
        }
    }

    let providers = default_provider_profiles();
    let ordered_ids: Vec<&String> = state.config.providers.order.iter().collect();
    for id in &ordered_ids {
        let Some(provider) = providers.iter().find(|p| p.id.0 == **id && p.enabled) else {
            continue;
        };
        if is_provider_healthy(state, &provider.id.0) {
            return RoutingResult {
                provider_id: baize_core::ProviderId((*id).clone()),
                previous_provider_id: None,
                reason: format!("Selected {} from configured provider priority.", id),
                confidence: 0.75,
            };
        }
    }
    let fallback = ordered_ids
        .first()
        .map(|id| baize_core::ProviderId((*id).clone()))
        .unwrap_or_else(|| baize_core::ProviderId("codex".to_string()));
    RoutingResult {
        provider_id: fallback.clone(),
        previous_provider_id: None,
        reason: format!(
            "Selected {} (higher-priority providers unhealthy).",
            fallback.0
        ),
        confidence: 0.6,
    }
}

fn try_sticky_provider(
    state: &AppState,
    workspace_id: &baize_core::WorkspaceId,
) -> Option<RoutingResult> {
    let latest = with_store(state, |store| {
        store.get_latest_session_for_workspace(workspace_id)
    })
    .ok()??;
    let active = latest.active_provider_id.as_ref()?;
    let elapsed = Utc::now()
        .signed_duration_since(latest.created_at)
        .num_minutes();
    if elapsed > STICKY_WINDOW_MINUTES {
        return None;
    }
    if !is_provider_healthy(state, &active.0) {
        return None;
    }
    Some(RoutingResult {
        provider_id: active.clone(),
        previous_provider_id: Some(active.clone()),
        reason: format!(
            "Sticky routing: reusing {} from recent session ({} min ago).",
            active.0, elapsed
        ),
        confidence: 0.85,
    })
}

pub fn is_provider_healthy(_state: &AppState, provider_id: &str) -> bool {
    let providers = default_provider_profiles();
    let Some(provider) = providers.iter().find(|p| p.id.0 == provider_id) else {
        return false;
    };
    let health = baize_adapters::check_provider(provider);
    matches!(
        health.status,
        baize_core::HealthStatus::Healthy | baize_core::HealthStatus::Unknown
    )
}

pub fn ordered_provider_profiles(config: &BaizeConfig) -> Vec<baize_core::ProviderProfile> {
    let providers = default_provider_profiles();
    let mut ordered = config
        .providers
        .order
        .iter()
        .filter_map(|id| {
            providers
                .iter()
                .find(|provider| provider.id.0 == *id && provider.enabled)
                .cloned()
        })
        .collect::<Vec<_>>();

    for provider in providers {
        if provider.enabled && !ordered.iter().any(|existing| existing.id == provider.id) {
            ordered.push(provider);
        }
    }

    ordered
}

pub fn parse_permission_status(status: &str) -> Option<PermissionStatus> {
    match status.to_ascii_lowercase().as_str() {
        "pending" => Some(PermissionStatus::Pending),
        "approved" => Some(PermissionStatus::Approved),
        "denied" => Some(PermissionStatus::Denied),
        _ => None,
    }
}

pub fn permission_status_eq(left: &PermissionStatus, right: &PermissionStatus) -> bool {
    matches!(
        (left, right),
        (PermissionStatus::Pending, PermissionStatus::Pending)
            | (PermissionStatus::Approved, PermissionStatus::Approved)
            | (PermissionStatus::Denied, PermissionStatus::Denied)
    )
}

pub fn format_error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        parts.push(error.to_string());
        source = error.source();
    }
    parts.join(": ")
}
