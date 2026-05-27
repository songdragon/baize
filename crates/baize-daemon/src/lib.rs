use anyhow::Result;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use baize_adapters::{check_provider, default_provider_profiles};
use baize_config::BaizeConfig;
use baize_core::BaizeEvent;
use baize_storage::EventStore;
use futures::stream;
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    config: BaizeConfig,
    store: Arc<Mutex<EventStore>>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceStatusQuery {
    pub path: Option<PathBuf>,
}

pub async fn run(config: BaizeConfig) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.daemon.host, config.daemon.port).parse()?;
    let state = AppState {
        config,
        store: Arc::new(Mutex::new(EventStore::open_default()?)),
    };
    let app = router(state);
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("baize daemon listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/providers", get(providers))
        .route("/providers/check", post(check_providers))
        .route("/workspaces/status", get(workspace_status))
        .route("/events", get(events))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "daemon": {
            "host": state.config.daemon.host,
            "port": state.config.daemon.port,
        }
    }))
}

async fn providers() -> Json<serde_json::Value> {
    Json(json!({ "providers": default_provider_profiles() }))
}

async fn check_providers(State(state): State<AppState>) -> Json<serde_json::Value> {
    let providers = default_provider_profiles();
    let health = providers.iter().map(check_provider).collect::<Vec<_>>();
    let event = BaizeEvent::new("provider.health.checked", json!({ "health": health }));
    if let Ok(store) = state.store.lock() {
        let _ = store.append_event(&event);
    }
    Json(json!({ "health": health }))
}

async fn workspace_status(
    axum::extract::Query(query): axum::extract::Query<WorkspaceStatusQuery>,
) -> Json<serde_json::Value> {
    let path = query.path.unwrap_or_else(|| PathBuf::from("."));
    match baize_workspace::inspect(path) {
        Ok(status) => Json(json!({ "status": status })),
        Err(error) => Json(json!({ "error": error.to_string() })),
    }
}

async fn events() -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let event = Event::default()
        .event("daemon.connected")
        .data(json!({ "status": "connected" }).to_string());
    Sse::new(stream::once(async move { Ok(event) }))
}
