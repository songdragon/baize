use crate::router::router;
use crate::state::{AgentExecutor, AppState};
use anyhow::Result;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use baize_adapters::{
    AgentExecutionEvent, AgentExecutionEventKind, AgentPromptRequest, AgentRunResult,
};
use baize_config::BaizeConfig;
use baize_core::ProviderId;
use baize_storage::EventStore;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

#[derive(Clone)]
struct FakeAgentExecutor {
    result: AgentRunResult,
}

impl AgentExecutor for FakeAgentExecutor {
    fn run_prompt(&self, _request: AgentPromptRequest) -> Result<AgentRunResult> {
        Ok(self.result.clone())
    }
}

#[derive(Clone)]
struct FailingAgentExecutor;

impl AgentExecutor for FailingAgentExecutor {
    fn run_prompt(&self, _request: AgentPromptRequest) -> Result<AgentRunResult> {
        Err(anyhow::anyhow!("inner failure").context("outer failure"))
    }
}

fn test_app() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                events: vec![AgentExecutionEvent {
                    kind: AgentExecutionEventKind::Output,
                    text: Some("fake output".to_string()),
                    raw: None,
                }],
                stderr: String::new(),
            },
        }),
    );
    (router(state), data_dir, project_dir)
}

async fn json_response(app: Router, request: Request<Body>) -> serde_json::Value {
    let (_, value) = json_response_with_status(app, request).await;
    value
}

async fn json_response_with_status(
    app: Router,
    request: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).expect("json");
    (status, value)
}

#[tokio::test]
async fn creates_workspace_session_prompt_and_events() {
    let (app, _data_dir, project_dir) = test_app();
    let workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path(), "name": "test-workspace" }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");

    let session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "write tests"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");
    assert_eq!(session["session"]["active_provider_id"], "codex");

    let prompt = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "hello" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(prompt["status"], "running");

    let events = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/events"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert!(events["events"]
        .as_array()
        .expect("events")
        .iter()
        .any(|event| event["event_type"] == "session.agent.completed"));
    assert!(events["events"]
        .as_array()
        .expect("events")
        .iter()
        .any(|event| event["event_type"] == "session.agent.output"
            && event["payload"]["text"] == "fake output"));

    let routes = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/routes"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(routes["routes"][0]["selected_provider_id"], "codex");
}

#[tokio::test]
async fn prompt_failure_returns_error_chain() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FailingAgentExecutor),
    );
    let app = router(state);

    let workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path() }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");
    let session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "failure path"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");

    let prompt = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "fail" }).to_string()))
            .expect("request"),
    )
    .await;

    assert_eq!(prompt["status"], "failed");
    assert_eq!(prompt["error"], "outer failure: inner failure");
}

#[tokio::test]
async fn creates_handoff_artifact() {
    let (app, _data_dir, project_dir) = test_app();
    let workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path() }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");
    let session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "handoff me",
                    "provider_id": "gemini"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");

    let handoff = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/handoff"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "to_provider_id": "codex",
                    "user_constraints": ["do not change public API"]
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    assert_eq!(handoff["handoff"]["from_provider_id"], "gemini");
    assert_eq!(handoff["handoff"]["to_provider_id"], "codex");
    assert_eq!(
        handoff["handoff"]["mechanical_facts"]["user_constraints"][0],
        "do not change public API"
    );

    let handoff_id = handoff["handoff"]["id"].as_str().expect("handoff id");
    let accepted = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!(
                "/sessions/{session_id}/handoff/{handoff_id}/accept"
            ))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(accepted["handoff"]["status"], "Accepted");
    assert_eq!(accepted["session"]["active_provider_id"], "codex");
    assert_eq!(accepted["route_decision"]["previous_provider_id"], "gemini");
    assert_eq!(accepted["route_decision"]["selected_provider_id"], "codex");

    let session = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(session["session"]["active_provider_id"], "codex");

    let events = json_response(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}/events"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert!(events["events"]
        .as_array()
        .expect("events")
        .iter()
        .any(|event| event["event_type"] == "handoff.accepted"));
}

#[tokio::test]
async fn lists_and_filters_permissions() {
    let (app, _data_dir, _project_dir) = test_app();
    let first = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/permissions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "session_id": "task_one",
                    "command": "cargo test",
                    "reason": "verify changes"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let first_id = first["permission"]["id"].as_str().expect("permission id");

    let second = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/permissions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "session_id": "task_two",
                    "command": "cargo fmt",
                    "reason": "format changes"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let second_id = second["permission"]["id"].as_str().expect("permission id");

    let _approved = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/permissions/{second_id}/approve"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    let pending = json_response(
        app.clone(),
        Request::builder()
            .uri("/permissions?status=pending")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let pending_items = pending["permissions"].as_array().expect("permissions");
    assert_eq!(pending_items.len(), 1);
    assert_eq!(pending_items[0]["id"], first_id);

    let loaded = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/permissions/{first_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(loaded["permission"]["id"], first_id);
    assert_eq!(loaded["permission"]["command"], "cargo test");

    let session_filtered = json_response(
        app.clone(),
        Request::builder()
            .uri("/permissions?session_id=task_two")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let session_items = session_filtered["permissions"]
        .as_array()
        .expect("permissions");
    assert_eq!(session_items.len(), 1);
    assert_eq!(session_items[0]["id"], second_id);
    assert_eq!(session_items[0]["status"], "Approved");

    let invalid = json_response(
        app.clone(),
        Request::builder()
            .uri("/permissions?status=maybe")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(invalid["error"], "invalid permission status");

    let missing = json_response(
        app,
        Request::builder()
            .uri("/permissions/perm_missing")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert!(missing["permission"].is_null());
}

#[tokio::test]
async fn validates_known_provider() {
    let (app, _data_dir, _project_dir) = test_app();
    let validation = json_response(
        app,
        Request::builder()
            .uri("/providers/gemini/validate")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(validation["validation"]["provider_id"], "gemini");
    assert!(validation["validation"]["detected"].is_object());
}

#[tokio::test]
async fn providers_follow_configured_order() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let mut config = BaizeConfig::default();
    config.providers.order = vec!["gemini".to_string(), "codex".to_string()];
    let app = router(AppState::with_executor(
        config,
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                events: Vec::new(),
                stderr: String::new(),
            },
        }),
    ));

    let providers = json_response(
        app,
        Request::builder()
            .uri("/providers")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(providers["providers"][0]["id"], "gemini");
    assert_eq!(providers["providers"][1]["id"], "codex");
    assert_eq!(providers["providers"][2]["id"], "copilot");
    assert_eq!(providers["providers"][3]["id"], "opencode");
}

#[tokio::test]
async fn provider_health_check_follows_configured_order() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let mut config = BaizeConfig::default();
    config.providers.order = vec!["gemini".to_string(), "codex".to_string()];
    let app = router(AppState::with_executor(
        config,
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                events: Vec::new(),
                stderr: String::new(),
            },
        }),
    ));

    let health = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/providers/check")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(health["health"][0]["provider_id"], "gemini");
    assert_eq!(health["health"][1]["provider_id"], "codex");
}

fn failing_app() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FailingAgentExecutor),
    );
    (router(state), data_dir, project_dir)
}

#[derive(Clone)]
struct RecoverableAgentExecutor {
    fail_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl AgentExecutor for RecoverableAgentExecutor {
    fn run_prompt(&self, _request: AgentPromptRequest) -> Result<AgentRunResult> {
        if self.fail_count.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            self.fail_count
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return Err(anyhow::anyhow!("transient failure"));
        }
        Ok(AgentRunResult {
            provider_id: ProviderId("codex".to_string()),
            success: true,
            exit_code: Some(0),
            events: vec![AgentExecutionEvent {
                kind: AgentExecutionEventKind::Output,
                text: Some("recovered output".to_string()),
                raw: None,
            }],
            stderr: String::new(),
        })
    }
}

fn recoverable_app() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(RecoverableAgentExecutor {
            fail_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1)),
        }),
    );
    (router(state), data_dir, project_dir)
}

async fn setup_workspace_and_session(
    app: &Router,
    project_dir: &tempfile::TempDir,
) -> (String, String) {
    let workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path() }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");
    let session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "test"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");
    (workspace_id.to_string(), session_id.to_string())
}

#[tokio::test]
async fn prompt_success_keeps_session_running() {
    let (app, _data_dir, project_dir) = test_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let prompt = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "hello" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(prompt["status"], "running");

    let session = json_response(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(session["session"]["status"], "Running");
}

#[tokio::test]
async fn prompt_failure_transitions_session_to_failed() {
    let (app, _data_dir, project_dir) = failing_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let prompt = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "fail" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(prompt["status"], "failed");

    let session = json_response(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(session["session"]["status"], "Failed");
}

#[tokio::test]
async fn failed_session_recovers_on_successful_prompt() {
    let (app, _data_dir, project_dir) = recoverable_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let first = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "fail first" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(first["status"], "failed");

    let session_after_fail = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(session_after_fail["session"]["status"], "Failed");

    let second = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "recover" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(second["status"], "running");

    let session_after_recover = json_response(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(session_after_recover["session"]["status"], "Running");
}

#[tokio::test]
async fn canceled_session_rejects_prompt() {
    let (app, _data_dir, project_dir) = test_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let cancel = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/cancel"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(cancel["session"]["status"], "Canceled");

    let prompt = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "after cancel" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(prompt["error"], "session is canceled");
}

#[tokio::test]
async fn session_status_changed_event_emitted_on_failure() {
    let (app, _data_dir, project_dir) = failing_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let _prompt = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "fail" }).to_string()))
            .expect("request"),
    )
    .await;

    let events = json_response(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}/events"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert!(events["events"]
        .as_array()
        .expect("events")
        .iter()
        .any(|event| event["event_type"] == "session.status.changed"
            && event["payload"]["status"] == "Failed"));
}

#[tokio::test]
async fn not_found_resources_return_404() {
    let (app, _data_dir, _project_dir) = test_app();

    let (status, body) = json_response_with_status(
        app.clone(),
        Request::builder()
            .uri("/sessions/task_missing")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "session not found");

    let (status, _body) = json_response_with_status(
        app.clone(),
        Request::builder()
            .uri("/permissions/perm_missing")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = json_response_with_status(
        app,
        Request::builder()
            .uri("/providers/nonexistent/health")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "provider not found");
}

#[tokio::test]
async fn bad_request_returns_400() {
    let (app, _data_dir, _project_dir) = test_app();

    let (status, body) = json_response_with_status(
        app.clone(),
        Request::builder()
            .uri("/permissions?status=maybe")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid permission status");
}

#[tokio::test]
async fn canceled_session_prompt_returns_400() {
    let (app, _data_dir, project_dir) = test_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let _cancel = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/cancel"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    let (status, body) = json_response_with_status(
        app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "after cancel" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "session is canceled");
}

#[tokio::test]
async fn prompt_failure_returns_500() {
    let (app, _data_dir, project_dir) = failing_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let (status, body) = json_response_with_status(
        app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "fail" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["status"], "failed");
}

#[tokio::test]
async fn successful_operations_return_200() {
    let (app, _data_dir, project_dir) = test_app();

    let (status, _) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri("/health")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri("/providers")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, workspace) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path() }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("id");

    let (status, _) = json_response_with_status(
        app,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/workspaces/{workspace_id}/status"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn session_handoffs_lists_handoffs_for_session() {
    let (app, _data_dir, project_dir) = test_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let handoff = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/handoff"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "to_provider_id": "gemini" }).to_string(),
            ))
            .expect("request"),
    )
    .await;

    let handoff_id = handoff["handoff"]["id"].as_str().expect("handoff id");

    let list = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/handoffs"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let handoffs = list["handoffs"].as_array().expect("handoffs array");
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0]["id"], handoff_id);
    assert_eq!(handoffs[0]["to_provider_id"], "gemini");
}

#[tokio::test]
async fn session_permissions_lists_permissions_for_session() {
    let (app, _data_dir, project_dir) = test_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/permissions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "session_id": session_id,
                    "command": "cargo test",
                    "reason": "verify"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/permissions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "session_id": session_id,
                    "command": "cargo fmt",
                    "reason": "format"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    let list = json_response(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}/permissions"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let perms = list["permissions"].as_array().expect("permissions array");
    assert_eq!(perms.len(), 2);
    assert_eq!(perms[0]["command"], "cargo test");
    assert_eq!(perms[1]["command"], "cargo fmt");
}

#[test]
fn select_provider_returns_requested_override() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                events: vec![],
                stderr: String::new(),
            },
        }),
    );
    let result = crate::helpers::select_provider(&state, Some("gemini".to_string()), None, None);
    assert_eq!(result.provider_id.0, "gemini");
    assert!(result.reason.contains("User-specified"));
}

#[test]
fn is_provider_healthy_returns_false_for_unknown_provider() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                events: vec![],
                stderr: String::new(),
            },
        }),
    );
    assert!(!crate::helpers::is_provider_healthy(
        &state,
        "nonexistent_provider"
    ));
}

async fn create_test_workspace(app: &Router) -> serde_json::Value {
    let project_dir = tempfile::tempdir().expect("project dir");
    let response = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path(), "name": "test-workspace" }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    response
}

#[tokio::test]
async fn create_session_route_decision_reflects_health_fallback() {
    let (app, _data_dir, _project_dir) = test_app();

    let workspace = create_test_workspace(&app).await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");

    let session_response = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "test objective"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    let session_provider = session_response["session"]["active_provider_id"]
        .as_str()
        .expect("provider id");

    let config = BaizeConfig::default();
    let first_choice = config
        .providers
        .order
        .first()
        .map(|s| s.as_str())
        .unwrap_or("codex");

    if session_provider == first_choice {
        let reason = session_response["route_decision"]["reason"]
            .as_str()
            .expect("reason");
        assert!(
            reason.contains("configured provider priority"),
            "expected priority reason, got: {reason}"
        );
    } else {
        let reason = session_response["route_decision"]["reason"]
            .as_str()
            .expect("reason");
        assert!(
            reason.contains("unhealthy"),
            "expected health fallback reason, got: {reason}"
        );
    }

    let confidence = session_response["route_decision"]["confidence"]
        .as_f64()
        .expect("confidence");
    if session_provider == first_choice {
        assert!((confidence - 0.75).abs() < f64::EPSILON);
    } else {
        assert!((confidence - 0.6).abs() < f64::EPSILON);
    }
}

#[tokio::test]
async fn second_session_in_workspace_reuses_provider_when_healthy() {
    let (app, _data_dir, _project_dir) = test_app();

    let workspace = create_test_workspace(&app).await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");

    let first_session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "first session"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let first_provider = first_session["session"]["active_provider_id"]
        .as_str()
        .expect("provider");

    let second_session = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "second session"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let second_provider = second_session["session"]["active_provider_id"]
        .as_str()
        .expect("provider");
    let reason = second_session["route_decision"]["reason"]
        .as_str()
        .expect("reason");

    if first_provider == second_provider && reason.contains("Sticky routing") {
        let confidence = second_session["route_decision"]["confidence"]
            .as_f64()
            .expect("confidence");
        assert!((confidence - 0.85).abs() < 0.01);
    } else {
        assert!(
            reason.contains("configured provider priority") || reason.contains("unhealthy"),
            "expected valid routing reason, got: {reason}"
        );
    }
}

#[test]
fn select_provider_without_workspace_uses_health_priority() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                events: vec![],
                stderr: String::new(),
            },
        }),
    );
    let result = crate::helpers::select_provider(&state, None, None, None);
    assert!(!result.provider_id.0.is_empty());
}

#[test]
fn select_provider_uses_custom_override_reason() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                events: vec![],
                stderr: String::new(),
            },
        }),
    );
    let result = crate::helpers::select_provider(
        &state,
        Some("gemini".to_string()),
        None,
        Some("Gemini handles multi-file edits better".to_string()),
    );
    assert_eq!(result.provider_id.0, "gemini");
    assert_eq!(result.reason, "Gemini handles multi-file edits better");
    assert!((result.confidence - 1.0).abs() < 0.01);
}
