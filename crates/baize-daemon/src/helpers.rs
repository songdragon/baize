use anyhow::Result;
use axum::http::StatusCode;
use axum::Json;
use baize_adapters::default_provider_profiles;
use baize_config::BaizeConfig;
use baize_core::{PermissionStatus, QuotaConfidence, QuotaSource, TaskType};
use baize_storage::EventStore;
use chrono::Utc;
use serde::Serialize;

use crate::state::AppState;

pub struct RoutingResult {
    pub provider_id: baize_core::ProviderId,
    pub previous_provider_id: Option<baize_core::ProviderId>,
    pub reason: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderLimitInference {
    pub kind: ProviderLimitKind,
    pub confidence: QuotaConfidence,
    pub source: QuotaSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ProviderLimitKind {
    QuotaExceeded,
    RateLimit,
}

pub fn infer_task_type(objective: &str) -> TaskType {
    let objective = objective.to_ascii_lowercase();
    if contains_any(&objective, &["test", "tests", "testing", "ut", "coverage"]) {
        TaskType::Testing
    } else if contains_any(&objective, &["debug", "bug", "fix", "failure", "error"]) {
        TaskType::Debugging
    } else if contains_any(&objective, &["refactor", "cleanup", "restructure"]) {
        TaskType::Refactor
    } else if contains_any(&objective, &["doc", "docs", "readme", "spec"]) {
        TaskType::Documentation
    } else {
        TaskType::Implementation
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

pub fn infer_provider_limit(message: &str) -> Option<ProviderLimitInference> {
    let normalized = message.to_ascii_lowercase();
    if contains_any(
        &normalized,
        &[
            "insufficient quota",
            "quota exceeded",
            "usage limit",
            "credit balance",
            "billing hard limit",
            "exceeded your current quota",
        ],
    ) {
        return Some(ProviderLimitInference {
            kind: ProviderLimitKind::QuotaExceeded,
            confidence: QuotaConfidence::Estimated,
            source: QuotaSource::ErrorInference,
        });
    }

    if contains_any(
        &normalized,
        &[
            "rate limit",
            "rate_limit",
            "too many requests",
            "429",
            "rpm",
            "tpm",
        ],
    ) {
        return Some(ProviderLimitInference {
            kind: ProviderLimitKind::RateLimit,
            confidence: QuotaConfidence::Estimated,
            source: QuotaSource::ErrorInference,
        });
    }

    None
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
    override_reason: Option<String>,
) -> RoutingResult {
    if let Some(requested) = requested {
        let reason =
            override_reason.unwrap_or_else(|| "User-specified provider override.".to_string());
        return RoutingResult {
            provider_id: baize_core::ProviderId(requested),
            previous_provider_id: None,
            reason,
            confidence: 1.0,
        };
    }

    let mut skipped = Vec::new();
    if let Some(wid) = workspace_id {
        if let Some(sticky) = try_sticky_provider(state, wid) {
            return sticky;
        }
    }

    let ordered = ordered_provider_profiles(&state.config);
    for provider in &ordered {
        if let Some(reason) = provider_failure_block_reason(state, &provider.id.0) {
            skipped.push(reason);
            continue;
        }
        if is_provider_routable(state, &provider.id.0) {
            return RoutingResult {
                provider_id: provider.id.clone(),
                previous_provider_id: None,
                reason: append_skipped_provider_reasons(
                    format!(
                        "Selected {} from configured provider priority.",
                        provider.id.0
                    ),
                    &skipped,
                ),
                confidence: 0.75,
            };
        }
    }
    let fallback = ordered
        .iter()
        .find(|provider| {
            baize_adapters::is_prompt_runtime_supported(&provider.id)
                && provider_failure_block_reason(state, &provider.id.0).is_none()
        })
        .or_else(|| {
            ordered
                .iter()
                .find(|provider| baize_adapters::is_prompt_runtime_supported(&provider.id))
        })
        .map(|provider| provider.id.clone())
        .unwrap_or_else(|| baize_core::ProviderId("codex".to_string()));
    RoutingResult {
        provider_id: fallback.clone(),
        previous_provider_id: None,
        reason: append_skipped_provider_reasons(
            format!(
                "Selected {} (higher-priority providers unavailable, unsupported, or failure-limited).",
                fallback.0
            ),
            &skipped,
        ),
        confidence: 0.6,
    }
}

fn try_sticky_provider(
    state: &AppState,
    workspace_id: &baize_core::WorkspaceId,
) -> Option<RoutingResult> {
    let sticky_window_minutes = i64::from(state.config.routing.sticky_window_minutes);
    if sticky_window_minutes == 0 {
        return None;
    }

    let latest = with_store(state, |store| {
        store.get_latest_session_for_workspace(workspace_id)
    })
    .ok()??;
    let active = latest.active_provider_id.as_ref()?;
    let elapsed = Utc::now()
        .signed_duration_since(latest.created_at)
        .num_minutes();
    if elapsed > sticky_window_minutes {
        return None;
    }
    if !is_provider_routable(state, &active.0) {
        return None;
    }
    if provider_failure_block_reason(state, &active.0).is_some() {
        return None;
    }
    Some(RoutingResult {
        provider_id: active.clone(),
        previous_provider_id: Some(active.clone()),
        reason: format!(
            "Sticky routing: reusing {} from recent session ({} min ago, window {} min).",
            active.0, elapsed, sticky_window_minutes
        ),
        confidence: 0.85,
    })
}

fn append_skipped_provider_reasons(mut reason: String, skipped: &[String]) -> String {
    if !skipped.is_empty() {
        reason.push_str(" Skipped providers: ");
        reason.push_str(&skipped.join("; "));
    }
    reason
}

pub(crate) fn provider_failure_block_reason(state: &AppState, provider_id: &str) -> Option<String> {
    let threshold = usize::from(state.config.routing.failure_threshold_count);
    if threshold == 0 {
        return None;
    }
    let count = provider_runtime_failure_count(state, provider_id);
    if count < threshold {
        return None;
    }
    Some(format!(
        "{provider_id} skipped after {count} provider/runtime failures since last success (threshold {threshold})"
    ))
}

pub(crate) fn provider_runtime_failure_count(state: &AppState, provider_id: &str) -> usize {
    let provider_id = baize_core::ProviderId(provider_id.to_string());
    let events = with_store(state, |store| {
        store.list_events_for_provider(&provider_id, Some(1000), None)
    })
    .unwrap_or_default();
    let mut count = 0;
    for event in events {
        if is_successful_provider_completion(&event) {
            count = 0;
        } else if is_provider_runtime_failure(&event) {
            count += 1;
        }
    }
    count
}

fn is_successful_provider_completion(event: &baize_core::BaizeEvent) -> bool {
    event.event_type == "session.agent.completed"
        && event
            .payload
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

fn is_provider_runtime_failure(event: &baize_core::BaizeEvent) -> bool {
    if event.event_type != "session.agent.failed" {
        return false;
    }
    event.payload.get("limit_inference").is_some()
        || provider_error_kind_is_runtime(&event.payload)
        || payload_text_is_runtime_failure(&event.payload)
}

fn provider_error_kind_is_runtime(payload: &serde_json::Value) -> bool {
    payload
        .get("provider_error")
        .and_then(|error| error.get("kind"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| {
            matches!(
                kind,
                "Authentication" | "Timeout" | "RateLimit" | "QuotaExceeded"
            )
        })
}

fn payload_text_is_runtime_failure(payload: &serde_json::Value) -> bool {
    let text = payload
        .get("error")
        .and_then(serde_json::Value::as_str)
        .or_else(|| payload.get("stderr").and_then(serde_json::Value::as_str))
        .unwrap_or_default();
    if infer_provider_limit(text).is_some() {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    contains_any(
        &lower,
        &[
            "authentication",
            "authenticating",
            "not authenticated",
            "login required",
            "please login",
            "ineligibletier",
            "unsupported_client",
            "timeout",
            "timed out",
        ],
    )
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

pub fn is_provider_routable(state: &AppState, provider_id: &str) -> bool {
    baize_adapters::is_prompt_runtime_supported(&baize_core::ProviderId(provider_id.to_string()))
        && is_provider_healthy(state, provider_id)
}

pub fn ordered_provider_profiles(config: &BaizeConfig) -> Vec<baize_core::ProviderProfile> {
    let providers = default_provider_profiles();
    let mut ordered = effective_provider_order(config)
        .filter_map(|id| {
            providers
                .iter()
                .find(|provider| provider.id.0 == id && provider.enabled)
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

fn effective_provider_order(config: &BaizeConfig) -> impl Iterator<Item = String> + '_ {
    let old_default = ["codex", "gemini", "copilot", "opencode"];
    if config
        .providers
        .order
        .iter()
        .map(String::as_str)
        .eq(old_default)
    {
        return vec![
            "codex".to_string(),
            "antigravity".to_string(),
            "opencode".to_string(),
            "copilot".to_string(),
        ]
        .into_iter();
    }

    let has_antigravity = config
        .providers
        .order
        .iter()
        .any(|provider| provider == "antigravity");
    config
        .providers
        .order
        .iter()
        .map(move |provider| {
            if provider == "gemini" && !has_antigravity {
                "antigravity".to_string()
            } else {
                provider.clone()
            }
        })
        .collect::<Vec<_>>()
        .into_iter()
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
