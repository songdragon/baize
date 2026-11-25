use anyhow::Result;
use baize_adapters::{AgentEventSink, AgentPromptRequest, AgentRunResult};
use baize_config::BaizeConfig;
use baize_core::{BaizeEvent, TaskSessionStatus};
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
    pub(crate) executions: crate::executions::ExecutionRegistry,
}

pub trait AgentExecutor: Send + Sync {
    fn run_prompt(
        &self,
        request: AgentPromptRequest,
        event_sink: Option<&mut AgentEventSink<'_>>,
    ) -> Result<AgentRunResult>;

    fn run_prompt_controlled(
        &self,
        request: AgentPromptRequest,
        event_sink: Option<&mut AgentEventSink<'_>>,
        _control: &baize_adapters::AgentControl,
    ) -> Result<AgentRunResult> {
        self.run_prompt(request, event_sink)
    }
}

#[derive(Clone)]
struct RealAgentExecutor;

impl AgentExecutor for RealAgentExecutor {
    fn run_prompt_controlled(
        &self,
        request: AgentPromptRequest,
        event_sink: Option<&mut AgentEventSink<'_>>,
        control: &baize_adapters::AgentControl,
    ) -> Result<AgentRunResult> {
        baize_adapters::run_agent_prompt_controlled(request, event_sink, control)
    }
    fn run_prompt(
        &self,
        request: AgentPromptRequest,
        event_sink: Option<&mut AgentEventSink<'_>>,
    ) -> Result<AgentRunResult> {
        baize_adapters::run_agent_prompt_streaming(request, event_sink)
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceStatusQuery {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspacesQuery {
    pub name: Option<String>,
    pub primary_project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceProjectsQuery {
    pub kind: Option<String>,
    pub vcs: Option<String>,
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
    pub provider_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PromptRequest {
    pub prompt: String,
    pub provider_id: Option<String>,
    #[serde(default)]
    pub image_paths: Vec<PathBuf>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ContextPreviewQuery {
    pub provider_id: Option<String>,
    pub prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HandoffRequest {
    pub to_provider_id: String,
    pub prompt: Option<String>,
    pub user_constraints: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct HandoffsQuery {
    pub status: Option<String>,
    pub to_provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RoutesQuery {
    pub selected_provider_id: Option<String>,
    pub task_type: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventHistoryQuery {
    pub event_type: Option<String>,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub provider_id: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
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
    pub risk_level: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct SessionsQuery {
    pub status: Option<String>,
    pub workspace_id: Option<String>,
    pub active_provider_id: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PaginationQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
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
        let state = Self {
            config,
            store: Arc::new(Mutex::new(store)),
            events,
            executor,
            executions: Default::default(),
        };
        state.recover_in_flight_sessions();
        if let Err(error) = crate::native_permissions::expire(&state, None) {
            tracing::warn!("recover native approvals: {error:#}");
        }
        state
    }

    pub fn record_event(&self, event: BaizeEvent) {
        if let Ok(store) = self.store.lock() {
            let _ = store.append_event(&event);
        }
        let _ = self.events.send(event);
    }

    fn recover_in_flight_sessions(&self) {
        let sessions = match self.store.lock() {
            Ok(store) => store.list_task_sessions(None, None).unwrap_or_default(),
            Err(_) => return,
        };

        for mut session in sessions {
            let mut confirmed_idle = false;
            // Older records have no lease field. An unmatched dispatch is still
            // uncertain; upgrading must not turn it into permission to replay.
            if session.continuity.active_turn.is_none() {
                let events = match self.store.lock() {
                    Ok(store) => store
                        .list_events_for_session(&session.id, Some(i64::MAX as u64), None)
                        .unwrap_or_default(),
                    Err(_) => continue,
                };
                for event in events {
                    match event.event_type.as_str() {
                        "session.agent.started" => {
                            session.continuity.active_turn = Some(baize_core::ExecutionAttempt {
                                id: event.id,
                                provider_id: event
                                    .provider_id
                                    .or_else(|| session.active_provider_id.clone())
                                    .unwrap_or_else(|| baize_core::ProviderId("unknown".into())),
                                uncertain: true,
                            })
                        }
                        "session.agent.completed" if event.payload["status"] != "canceled" => {
                            confirmed_idle = true;
                            session.continuity.active_turn = None
                        }
                        "session.agent.failed"
                            if event.payload["active_turn"]["uncertain"] == true =>
                        {
                            if let Ok(attempt) =
                                serde_json::from_value(event.payload["active_turn"].clone())
                            {
                                session.continuity.active_turn = Some(attempt);
                            }
                        }
                        "session.agent.failed"
                            if event.payload["exit_code"].is_number()
                                && event.payload["provider_error"]["kind"] != "Timeout" =>
                        {
                            confirmed_idle = true;
                            session.continuity.active_turn = None
                        }
                        "session.agent.ignored"
                            if event
                                .payload
                                .get("active_turn")
                                .is_some_and(serde_json::Value::is_null) =>
                        {
                            confirmed_idle = true;
                            session.continuity.active_turn = None
                        }
                        "session.execution.reconciled" => {
                            confirmed_idle = true;
                            session.continuity.active_turn = None
                        }
                        _ => {}
                    }
                }
            }
            if (!matches!(session.status, TaskSessionStatus::Running) || confirmed_idle)
                && session.continuity.active_turn.is_none()
            {
                continue;
            }
            if let Some(attempt) = session.continuity.active_turn.as_mut() {
                attempt.uncertain = true;
            }
            if let Some(stop) = session.continuity.last_stop.as_mut() {
                stop.status = baize_core::ExecutionStopStatus::Unconfirmed;
                stop.detail = "Daemon restarted; native execution stop is unconfirmed.".into();
            }
            if !matches!(session.status, TaskSessionStatus::Canceled) {
                session.status = TaskSessionStatus::Failed;
            }
            session.updated_at = chrono::Utc::now();
            if let Ok(store) = self.store.lock() {
                let _ = store.upsert_task_session(&session);
            }

            let mut event = BaizeEvent::new(
                "session.recovered",
                serde_json::json!({
                    "status": session.status,
                    "reason": "Recovered in-flight session after daemon startup.",
                    "active_turn": session.continuity.active_turn,
                }),
            );
            event.workspace_id = Some(session.workspace_id.clone());
            event.session_id = Some(session.id.clone());
            event.provider_id = session.active_provider_id.clone();
            self.record_event(event);
        }
    }
}
