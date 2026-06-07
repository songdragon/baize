use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::helpers::{ok_json, ordered_provider_profiles};
use crate::state::AppState;

pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    ok_json(serde_json::json!({
        "status": "ok",
        "daemon": {
            "host": state.config.daemon.host,
            "port": state.config.daemon.port,
        }
    }))
}

pub async fn providers(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    ok_json(serde_json::json!({ "providers": ordered_provider_profiles(&state.config) }))
}

pub async fn provider_health(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let providers = baize_adapters::default_provider_profiles();
    let Some(provider) = providers.into_iter().find(|provider| provider.id.0 == id) else {
        return crate::helpers::not_found("provider not found");
    };
    ok_json(serde_json::json!({ "health": baize_adapters::check_provider(&provider) }))
}

pub async fn provider_validate(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let providers = baize_adapters::default_provider_profiles();
    let Some(provider) = providers.into_iter().find(|provider| provider.id.0 == id) else {
        return crate::helpers::not_found("provider not found");
    };
    ok_json(serde_json::json!({ "validation": baize_adapters::validate_provider(&provider) }))
}

pub async fn provider_diagnose(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let providers = baize_adapters::default_provider_profiles();
    let Some(provider) = providers.into_iter().find(|provider| provider.id.0 == id) else {
        return crate::helpers::not_found("provider not found");
    };
    ok_json(serde_json::json!({ "diagnostic": baize_adapters::diagnose_provider(&provider) }))
}

pub async fn check_providers(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let providers = ordered_provider_profiles(&state.config);
    let health = providers
        .iter()
        .map(baize_adapters::check_provider)
        .collect::<Vec<_>>();
    let event = baize_core::BaizeEvent::new(
        "provider.health.changed",
        serde_json::json!({ "health": health }),
    );
    state.record_event(event);
    ok_json(serde_json::json!({ "health": health }))
}

pub async fn validate_providers(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let validations = baize_adapters::validate_all_providers();
    let event = baize_core::BaizeEvent::new(
        "provider.validation.completed",
        serde_json::json!({ "validations": validations }),
    );
    state.record_event(event);
    ok_json(serde_json::json!({ "validations": validations }))
}

pub async fn diagnose_providers(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let providers = ordered_provider_profiles(&state.config);
    let diagnostics = providers
        .iter()
        .map(baize_adapters::diagnose_provider)
        .collect::<Vec<_>>();
    let event = baize_core::BaizeEvent::new(
        "provider.diagnostic.completed",
        serde_json::json!({ "diagnostics": diagnostics }),
    );
    state.record_event(event);
    ok_json(serde_json::json!({ "diagnostics": diagnostics }))
}
