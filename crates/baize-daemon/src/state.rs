use anyhow::Result;
use baize_adapters::{AgentPromptRequest, AgentRunResult};
use baize_config::BaizeConfig;
use baize_core::BaizeEvent;
use baize_storage::EventStore;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub config: BaizeConfig,
    pub store: Arc<Mutex<EventStore>>,
    pub events: broadcast::Sender<BaizeEvent>,
    pub executor: Arc<dyn AgentExecutor>,
}

pub trait AgentExecutor: Send + Sync {
    fn run_prompt(&self, request: AgentPromptRequest) -> Result<AgentRunResult>;
}

#[derive(Clone)]
struct RealAgentExecutor;

impl AgentExecutor for RealAgentExecutor {
    fn run_prompt(&self, request: AgentPromptRequest) -> Result<AgentRunResult> {
        baize_adapters::run_agent_prompt(request)
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceStatusQuery {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub path: PathBuf,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub workspace_id: String,
    pub objective: String,
    pub provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PromptRequest {
    pub prompt: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct HandoffRequest {
    pub to_provider_id: String,
    pub user_constraints: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePermissionRequest {
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct PermissionsQuery {
    pub status: Option<String>,
    pub session_id: Option<String>,
}

pub async fn run(config: BaizeConfig) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.daemon.host, config.daemon.port).parse()?;
    let state = AppState::new(config, EventStore::open_default()?);
    let app = crate::router::router(state);
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("baize daemon listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

impl AppState {
    pub fn new(config: BaizeConfig, store: EventStore) -> Self {
        Self::with_executor(config, store, Arc::new(RealAgentExecutor))
    }

    pub fn with_executor(
        config: BaizeConfig,
        store: EventStore,
        executor: Arc<dyn AgentExecutor>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            config,
            store: Arc::new(Mutex::new(store)),
            events,
            executor,
        }
    }

    pub fn record_event(&self, event: BaizeEvent) {
        if let Ok(store) = self.store.lock() {
            let _ = store.append_event(&event);
        }
        let _ = self.events.send(event);
    }
}
