use crate::router::router;
use crate::state::{AgentExecutor, AppState};
use anyhow::Result;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use baize_adapters::{
    AgentErrorDetail, AgentErrorKind, AgentErrorSource, AgentExecutionEvent,
    AgentExecutionEventKind, AgentExecutionPolicy, AgentPromptRequest, AgentRunResult,
};
use baize_config::BaizeConfig;
use baize_core::{
    ProviderId, QuotaConfidence, QuotaSource, TaskSession, TaskSessionId, TaskSessionStatus,
    TaskType, WorkspaceId,
};
use baize_storage::EventStore;
use serde_json::json;
use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
};
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
struct RecordingAgentExecutor {
    result: AgentRunResult,
    requests: Arc<Mutex<Vec<AgentPromptRequest>>>,
}

impl AgentExecutor for RecordingAgentExecutor {
    fn run_prompt(&self, request: AgentPromptRequest) -> Result<AgentRunResult> {
        self.requests.lock().expect("requests").push(request);
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

#[derive(Clone)]
struct RateLimitedAgentExecutor;

impl AgentExecutor for RateLimitedAgentExecutor {
    fn run_prompt(&self, _request: AgentPromptRequest) -> Result<AgentRunResult> {
        Err(anyhow::anyhow!(
            "429 Too Many Requests: rate limit exceeded for provider"
        ))
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
                native_session_id: None,
                events: vec![AgentExecutionEvent {
                    kind: AgentExecutionEventKind::Output,
                    text: Some("fake output".to_string()),
                    raw: None,
                }],
                stderr: String::new(),
                error: None,
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
    assert_eq!(prompt["turn_status"], "completed");
    assert_eq!(prompt["session_status"], "Running");

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

    let history = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/events/history?session_id={session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let history_events = history["events"].as_array().expect("history events");
    assert!(history_events
        .iter()
        .any(|event| event["event_type"] == "session.agent.completed"));

    let provider_history = json_response(
        app.clone(),
        Request::builder()
            .uri("/events/history?provider_id=codex&event_type=session.agent.output")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let provider_events = provider_history["events"]
        .as_array()
        .expect("provider history events");
    assert!(provider_events
        .iter()
        .any(|event| event["payload"]["text"] == "fake output"));

    let routes = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/routes"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(routes["routes"][0]["selected_provider_id"], "codex");
    assert_eq!(routes["routes"][0]["task_type"], "Testing");

    let codex_routes = json_response(
        app.clone(),
        Request::builder()
            .uri(format!(
                "/sessions/{session_id}/routes?selected_provider_id=codex"
            ))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(
        codex_routes["routes"]
            .as_array()
            .expect("routes array")
            .len(),
        1
    );

    let testing_routes = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/routes?task_type=testing"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(testing_routes["routes"][0]["selected_provider_id"], "codex");

    let assisted_routes = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/routes?mode=assisted"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(assisted_routes["routes"][0]["mode"], "Assisted");

    let (task_status, invalid_task) = json_response_with_status(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/routes?task_type=unknown"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(task_status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_task["error"], "invalid route task type");

    let (mode_status, invalid_mode) = json_response_with_status(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}/routes?mode=background"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(mode_status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_mode["error"], "invalid route mode");
}

#[tokio::test]
async fn prompt_provider_target_switches_session_route_and_executor() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(RecordingAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("gemini".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
            requests: requests.clone(),
        }),
    );
    let app = router(state);

    let workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path(), "name": "switch-provider" }).to_string(),
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
                    "objective": "implement feature",
                    "provider_id": "codex"
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
            .body(Body::from(
                json!({
                    "prompt": "continue on gemini",
                    "provider_id": "gemini"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(prompt["status"], "running");
    assert_eq!(prompt["provider_id"], "gemini");

    let captured_provider = {
        let captured = requests.lock().expect("requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].execution_policy, AgentExecutionPolicy::Ask);
        captured[0].provider_id.0.clone()
    };
    assert_eq!(captured_provider, "gemini");

    let updated = json_response(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!("/sessions/{session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(updated["session"]["active_provider_id"], "gemini");

    let routes = json_response(
        app,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/sessions/{session_id}/routes"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let route_items = routes["routes"].as_array().expect("routes");
    assert_eq!(route_items.len(), 2);
    assert_eq!(route_items[1]["previous_provider_id"], "codex");
    assert_eq!(route_items[1]["selected_provider_id"], "gemini");
    assert_eq!(route_items[1]["mode"], "Manual");
    assert!(route_items[1]["reason"]
        .as_str()
        .expect("reason")
        .contains("Prompt target provider override"));
}

#[tokio::test]
async fn prompt_request_uses_workspace_command_policy() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut config = BaizeConfig::default();
    config.workspace.command_policy = "allow_project".to_string();
    let state = AppState::with_executor(
        config,
        store,
        Arc::new(RecordingAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
            requests: requests.clone(),
        }),
    );
    let app = router(state);
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let prompt = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "edit files" }).to_string()))
            .expect("request"),
    )
    .await;

    assert_eq!(prompt["status"], "running");
    let captured = requests.lock().expect("requests");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].execution_policy,
        AgentExecutionPolicy::AllowProject
    );
}

#[tokio::test]
async fn filters_sessions_by_status_provider_and_workspace() {
    let (app, _data_dir, project_dir) = test_app();
    let other_project_dir = tempfile::tempdir().expect("other project dir");
    let first_workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path(), "name": "first" }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let first_workspace_id = first_workspace["workspace"]["id"]
        .as_str()
        .expect("workspace id");
    let second_workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": other_project_dir.path(), "name": "second" }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let second_workspace_id = second_workspace["workspace"]["id"]
        .as_str()
        .expect("workspace id");

    let first_session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": first_workspace_id,
                    "objective": "write tests",
                    "provider_id": "codex"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let first_session_id = first_session["session"]["id"].as_str().expect("session id");
    let second_session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": second_workspace_id,
                    "objective": "debug failure",
                    "provider_id": "gemini"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let second_session_id = second_session["session"]["id"]
        .as_str()
        .expect("session id");

    let _canceled = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{second_session_id}/cancel"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    let canceled = json_response(
        app.clone(),
        Request::builder()
            .uri("/sessions?status=canceled")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let canceled_items = canceled["sessions"].as_array().expect("sessions");
    assert_eq!(canceled_items.len(), 1);
    assert_eq!(canceled_items[0]["id"], second_session_id);

    let codex = json_response(
        app.clone(),
        Request::builder()
            .uri("/sessions?active_provider_id=codex")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let codex_items = codex["sessions"].as_array().expect("sessions");
    assert_eq!(codex_items.len(), 1);
    assert_eq!(codex_items[0]["id"], first_session_id);

    let by_workspace = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions?workspace_id={second_workspace_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let workspace_items = by_workspace["sessions"].as_array().expect("sessions");
    assert_eq!(workspace_items.len(), 1);
    assert_eq!(workspace_items[0]["id"], second_session_id);

    let (status, invalid) = json_response_with_status(
        app,
        Request::builder()
            .uri("/sessions?status=paused")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid["error"], "invalid session status");
}

#[tokio::test]
async fn prompt_response_reports_native_provider_session_id() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let app = router(AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: Some("codex_native_session_1".to_string()),
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    ));

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
                    "objective": "preserve native session"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");

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
    assert_eq!(prompt["native_session_id"], "codex_native_session_1");

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
        .any(|event| event["event_type"] == "session.agent.completed"
            && event["payload"]["native_session_id"] == "codex_native_session_1"));
}

#[tokio::test]
async fn prompt_reuses_saved_native_provider_session_id() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = router(AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(RecordingAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: Some("codex_native_session_1".to_string()),
                events: vec![],
                stderr: String::new(),
                error: None,
            },
            requests: requests.clone(),
        }),
    ));
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let first = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "first" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(first["native_session_id"], "codex_native_session_1");

    let second = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "second" }).to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(second["status"], "running");

    let session = json_response(
        app,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/sessions/{session_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(
        session["session"]["provider_native_session_ids"]["codex"],
        "codex_native_session_1"
    );

    let captured = requests.lock().expect("requests");
    assert_eq!(captured.len(), 2);
    assert!(captured[0].session_id.is_none());
    assert_eq!(
        captured[1].session_id.as_deref(),
        Some("codex_native_session_1")
    );
}

#[tokio::test]
async fn session_diff_reports_git_hunks() {
    let (app, _data_dir, project_dir) = test_app();
    run_git(project_dir.path(), &["init", "-b", "main"]);
    fs::write(project_dir.path().join("tracked.txt"), "before\nsame\n").expect("write tracked");
    run_git(project_dir.path(), &["add", "tracked.txt"]);
    run_git(project_dir.path(), &["commit", "-m", "initial"]);
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    fs::write(project_dir.path().join("tracked.txt"), "after\nsame\n").expect("modify tracked");

    let diff = json_response(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}/diff"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(diff["diff"]["dirty"], true);
    assert_eq!(diff["diff"]["changed_files"][0], "tracked.txt");
    assert_eq!(diff["diff"]["hunks"][0]["file_path"], "tracked.txt");
    assert_eq!(diff["diff"]["hunks"][0]["old_start"], 1);
    assert!(diff["diff"]["hunks"][0]["lines"]
        .as_array()
        .expect("hunk lines")
        .iter()
        .any(|line| line == "-before"));
    assert!(diff["diff"]["hunks"][0]["lines"]
        .as_array()
        .expect("hunk lines")
        .iter()
        .any(|line| line == "+after"));
}

#[test]
fn infers_task_type_from_objective_text() {
    assert_eq!(
        crate::helpers::infer_task_type("please add unit tests"),
        TaskType::Testing
    );
    assert_eq!(
        crate::helpers::infer_task_type("debug this provider failure"),
        TaskType::Debugging
    );
    assert_eq!(
        crate::helpers::infer_task_type("refactor the TUI state"),
        TaskType::Refactor
    );
    assert_eq!(
        crate::helpers::infer_task_type("update README docs"),
        TaskType::Documentation
    );
    assert_eq!(
        crate::helpers::infer_task_type("build a new endpoint"),
        TaskType::Implementation
    );
}

#[test]
fn infers_provider_limit_from_error_text() {
    let quota = crate::helpers::infer_provider_limit(
        "Provider error: insufficient quota, please check your billing details",
    )
    .expect("quota inference");
    assert_eq!(quota.kind, crate::helpers::ProviderLimitKind::QuotaExceeded);
    assert_eq!(quota.confidence, QuotaConfidence::Estimated);
    assert_eq!(quota.source, QuotaSource::ErrorInference);

    let rate_limit =
        crate::helpers::infer_provider_limit("HTTP 429 Too Many Requests: rate limit exceeded")
            .expect("rate-limit inference");
    assert_eq!(
        rate_limit.kind,
        crate::helpers::ProviderLimitKind::RateLimit
    );
    assert_eq!(rate_limit.confidence, QuotaConfidence::Estimated);
    assert_eq!(rate_limit.source, QuotaSource::ErrorInference);

    assert!(crate::helpers::infer_provider_limit("plain adapter failure").is_none());
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
async fn prompt_failure_reports_limit_inference() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(RateLimitedAgentExecutor),
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
                    "objective": "rate limit path"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");

    let (status, prompt) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "fail" }).to_string()))
            .expect("request"),
    )
    .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(prompt["status"], "failed");
    assert_eq!(prompt["limit_inference"]["kind"], "RateLimit");
    assert_eq!(prompt["limit_inference"]["confidence"], "Estimated");
    assert_eq!(prompt["limit_inference"]["source"], "ErrorInference");

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
        .any(|event| event["event_type"] == "session.agent.failed"
            && event["payload"]["limit_inference"]["kind"] == "RateLimit"));
}

#[tokio::test]
async fn prompt_failure_reports_structured_provider_error() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let app = router(AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: false,
                exit_code: Some(1),
                native_session_id: None,
                events: vec![],
                stderr: "Please login before using Codex".to_string(),
                error: Some(AgentErrorDetail {
                    kind: AgentErrorKind::Authentication,
                    message: "Please login before using Codex".to_string(),
                    source: AgentErrorSource::Stderr,
                }),
            },
        }),
    ));

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
                    "objective": "auth failure"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");

    let (status, prompt) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "fail" }).to_string()))
            .expect("request"),
    )
    .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(prompt["status"], "failed");
    assert_eq!(prompt["provider_error"]["kind"], "Authentication");
    assert_eq!(prompt["provider_error"]["source"], "Stderr");

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
        .any(|event| event["event_type"] == "session.agent.failed"
            && event["payload"]["provider_error"]["kind"] == "Authentication"));
}

#[tokio::test]
async fn creates_handoff_artifact() {
    let (app, data_dir, project_dir) = test_app();
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
    assert_eq!(
        handoff["handoff"]["mechanical_facts"]["checkpoint_refs"][0]
            .as_str()
            .expect("checkpoint ref")
            .split(':')
            .next()
            .expect("checkpoint prefix"),
        "before_handoff"
    );
    let artifact_path = handoff["artifact_path"].as_str().expect("artifact path");
    assert!(artifact_path.contains("artifacts/handoffs"));
    assert!(std::path::Path::new(artifact_path).starts_with(data_dir.path()));
    assert_eq!(
        std::fs::read_to_string(artifact_path).expect("artifact contents"),
        handoff["handoff"]["summary_markdown"]
            .as_str()
            .expect("summary")
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
    assert!(events["events"]
        .as_array()
        .expect("events")
        .iter()
        .any(|event| event["event_type"] == "handoff.created"
            && event["payload"]["artifact_path"] == artifact_path));
}

#[tokio::test]
async fn handoff_checkpoint_refs_follow_policy() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let mut config = BaizeConfig::default();
    config.workspace.checkpoint_policy = "off".to_string();
    let app = router(AppState::with_executor(
        config,
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    ));
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
                    "objective": "handoff without checkpoint",
                    "provider_id": "codex"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let session_id = session["session"]["id"].as_str().expect("session id");

    let handoff = json_response(
        app,
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

    assert!(handoff["handoff"]["mechanical_facts"]["checkpoint_refs"]
        .as_array()
        .expect("checkpoint refs")
        .is_empty());
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
async fn create_permission_reports_command_risk() {
    let (app, _data_dir, _project_dir) = test_app();
    let permission = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/permissions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "session_id": "task_risky",
                    "command": "sudo chmod 777 /tmp/file",
                    "reason": "change permissions"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    assert_eq!(permission["permission"]["risk"]["level"], "High");
    assert_eq!(permission["permission"]["status"], "Pending");
    assert!(permission["permission"]["risk"]["reasons"]
        .as_array()
        .expect("risk reasons")
        .iter()
        .any(|reason| reason == "command can delete, overwrite or elevate privileges"));
}

#[tokio::test]
async fn filters_permissions_by_risk_level() {
    let (app, _data_dir, _project_dir) = test_app();
    let _low = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/permissions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "session_id": "task_low",
                    "command": "cargo test",
                    "reason": "verify changes"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let high = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/permissions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "session_id": "task_high",
                    "command": "sudo chmod 777 /tmp/file",
                    "reason": "change permissions"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    let high_id = high["permission"]["id"].as_str().expect("permission id");

    let filtered = json_response(
        app.clone(),
        Request::builder()
            .uri("/permissions?risk_level=high")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let permissions = filtered["permissions"].as_array().expect("permissions");
    assert_eq!(permissions.len(), 1);
    assert_eq!(permissions[0]["id"], high_id);
    assert_eq!(permissions[0]["risk"]["level"], "High");

    let (status, invalid) = json_response_with_status(
        app,
        Request::builder()
            .uri("/permissions?risk_level=surprising")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid["error"], "invalid permission risk level");
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
async fn validation_reports_acp_initialize_proof() {
    let (app, _data_dir, _project_dir) = test_app();
    let validation = json_response(
        app,
        Request::builder()
            .uri("/providers/opencode/validate")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(validation["validation"]["provider_id"], "opencode");
    assert_eq!(
        validation["validation"]["acp_proof"]["initialize_request"]["method"],
        "initialize"
    );
    assert_eq!(
        validation["validation"]["acp_proof"]["initialize_request"]["params"]["client"]["name"],
        "baize"
    );
}

#[tokio::test]
async fn diagnoses_known_provider() {
    let (app, _data_dir, _project_dir) = test_app();
    let diagnostic = json_response(
        app,
        Request::builder()
            .uri("/providers/gemini/diagnose")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(diagnostic["diagnostic"]["provider_id"], "gemini");
    assert!(diagnostic["diagnostic"]["readiness"].is_string());
    assert!(diagnostic["diagnostic"]["issues"].is_array());
    assert!(diagnostic["diagnostic"]["suggested_actions"].is_array());
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
                native_session_id: None,
                events: Vec::new(),
                stderr: String::new(),
                error: None,
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
                native_session_id: None,
                events: Vec::new(),
                stderr: String::new(),
                error: None,
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

#[tokio::test]
async fn provider_diagnostics_follow_configured_order_and_emit_event() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let mut config = BaizeConfig::default();
    config.providers.order = vec!["gemini".to_string(), "codex".to_string()];
    let state = AppState::with_executor(
        config,
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: Vec::new(),
                stderr: String::new(),
                error: None,
            },
        }),
    );
    let app = router(state.clone());

    let diagnostics = json_response(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/providers/diagnose")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(diagnostics["diagnostics"][0]["provider_id"], "gemini");
    assert_eq!(diagnostics["diagnostics"][1]["provider_id"], "codex");

    let events = crate::helpers::with_store(&state, |store| {
        store.list_events_by_type("provider.diagnostic.completed", None, None)
    })
    .expect("events");
    assert_eq!(events.len(), 1);
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
            native_session_id: None,
            events: vec![AgentExecutionEvent {
                kind: AgentExecutionEventKind::Output,
                text: Some("recovered output".to_string()),
                raw: None,
            }],
            stderr: String::new(),
            error: None,
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
    assert_eq!(prompt["turn_status"], "failed");
    assert_eq!(prompt["session_status"], "Failed");

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
async fn prompt_rejects_provider_without_runtime_support() {
    let (app, _data_dir, project_dir) = test_app();
    let (_, session_id) = setup_workspace_and_session(&app, &project_dir).await;

    let (status, prompt) = json_response_with_status(
        app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/prompt"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "prompt": "hello",
                    "provider_id": "opencode"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        prompt["error"],
        "provider opencode does not support Baize prompt execution yet"
    );
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
    assert_eq!(second["turn_status"], "completed");
    assert_eq!(second["session_status"], "Running");

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

#[test]
fn app_state_recovers_in_flight_sessions_on_startup() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let now = chrono::Utc::now();
    let running = TaskSession {
        id: TaskSessionId("task_running".to_string()),
        workspace_id: WorkspaceId("ws_1".to_string()),
        objective: "recover this".to_string(),
        active_provider_id: Some(ProviderId("codex".to_string())),
        provider_native_session_ids: Default::default(),
        status: TaskSessionStatus::Running,
        created_at: now,
        updated_at: now,
    };
    let canceled = TaskSession {
        id: TaskSessionId("task_canceled".to_string()),
        workspace_id: WorkspaceId("ws_1".to_string()),
        objective: "leave canceled".to_string(),
        active_provider_id: Some(ProviderId("gemini".to_string())),
        provider_native_session_ids: Default::default(),
        status: TaskSessionStatus::Canceled,
        created_at: now,
        updated_at: now,
    };
    store
        .upsert_task_session(&running)
        .expect("running session");
    store
        .upsert_task_session(&canceled)
        .expect("canceled session");

    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    );

    let recovered = crate::helpers::with_store(&state, |store| store.get_task_session(&running.id))
        .expect("lookup")
        .expect("running exists");
    let still_canceled =
        crate::helpers::with_store(&state, |store| store.get_task_session(&canceled.id))
            .expect("lookup")
            .expect("canceled exists");
    let events = crate::helpers::with_store(&state, |store| {
        store.list_events_for_session(&running.id, None, None)
    })
    .expect("events");

    assert!(matches!(recovered.status, TaskSessionStatus::Failed));
    assert!(matches!(still_canceled.status, TaskSessionStatus::Canceled));
    assert!(events
        .iter()
        .any(|event| event.event_type == "session.recovered"
            && event.payload["reason"] == "Recovered in-flight session after daemon startup."));
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

#[test]
fn session_was_canceled_detects_stored_canceled_state() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let now = chrono::Utc::now();
    let canceled = TaskSession {
        id: TaskSessionId("task_canceled_before_result".to_string()),
        workspace_id: WorkspaceId("ws_1".to_string()),
        objective: "cancel before provider result".to_string(),
        active_provider_id: Some(ProviderId("codex".to_string())),
        provider_native_session_ids: Default::default(),
        status: TaskSessionStatus::Canceled,
        created_at: now,
        updated_at: now,
    };
    store
        .upsert_task_session(&canceled)
        .expect("canceled session");
    let state = AppState::with_executor(
        BaizeConfig::default(),
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    );

    assert!(crate::handlers::sessions::session_was_canceled(
        &state,
        &canceled.id
    ));
    assert!(!crate::handlers::sessions::session_was_canceled(
        &state,
        &TaskSessionId("task_missing".to_string())
    ));
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
async fn workspace_projects_lists_projects_for_workspace() {
    let (app, _data_dir, project_dir) = test_app();
    let workspace = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": project_dir.path(), "name": "project-list" }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("id");
    let primary_project_id = workspace["workspace"]["primary_project_id"]
        .as_str()
        .expect("project id");

    let named = json_response(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri("/workspaces?name=project-list")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let named_workspaces = named["workspaces"].as_array().expect("workspaces array");
    assert_eq!(named_workspaces.len(), 1);
    assert_eq!(named_workspaces[0]["id"], workspace_id);

    let by_primary_project = json_response(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/workspaces?primary_project_id={primary_project_id}"
            ))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let project_workspaces = by_primary_project["workspaces"]
        .as_array()
        .expect("workspaces array");
    assert_eq!(project_workspaces.len(), 1);
    assert_eq!(project_workspaces[0]["id"], workspace_id);

    let list = json_response(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!("/workspaces/{workspace_id}/projects"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let projects = list["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["id"], primary_project_id);

    let project = json_response(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!("/projects/{primary_project_id}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(project["project"]["id"], primary_project_id);
    assert_eq!(project["project"]["workspace_id"], workspace_id);

    let directories = json_response(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/workspaces/{workspace_id}/projects?kind=directory"
            ))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let directory_projects = directories["projects"].as_array().expect("projects array");
    assert_eq!(directory_projects.len(), 1);
    assert_eq!(directory_projects[0]["id"], primary_project_id);

    let no_vcs = json_response(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!("/workspaces/{workspace_id}/projects?vcs=none"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let no_vcs_projects = no_vcs["projects"].as_array().expect("projects array");
    assert_eq!(no_vcs_projects.len(), 1);
    assert_eq!(no_vcs_projects[0]["id"], primary_project_id);

    let (status, invalid_kind) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/workspaces/{workspace_id}/projects?kind=spaceship"
            ))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_kind["error"], "invalid project kind");

    let (status, invalid_vcs) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!("/workspaces/{workspace_id}/projects?vcs=svn"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_vcs["error"], "invalid project vcs");

    let (status, missing) = json_response_with_status(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri("/workspaces/ws_missing/projects")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"], "workspace not found");

    let (status, missing_project) = json_response_with_status(
        app,
        Request::builder()
            .method(Method::GET)
            .uri("/projects/prj_missing")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing_project["error"], "project not found");
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

    let _accepted = json_response(
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
    let second = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/handoff"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "to_provider_id": "codex" }).to_string()))
            .expect("request"),
    )
    .await;
    let second_handoff_id = second["handoff"]["id"].as_str().expect("handoff id");

    let list = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/handoffs"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let handoffs = list["handoffs"].as_array().expect("handoffs array");
    assert_eq!(handoffs.len(), 2);
    assert_eq!(handoffs[0]["id"], handoff_id);
    assert_eq!(handoffs[0]["to_provider_id"], "gemini");
    assert_eq!(handoffs[0]["status"], "Accepted");

    let accepted = json_response(
        app.clone(),
        Request::builder()
            .uri(format!("/sessions/{session_id}/handoffs?status=accepted"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let accepted_handoffs = accepted["handoffs"].as_array().expect("handoffs array");
    assert_eq!(accepted_handoffs.len(), 1);
    assert_eq!(accepted_handoffs[0]["id"], handoff_id);

    let to_codex = json_response(
        app.clone(),
        Request::builder()
            .uri(format!(
                "/sessions/{session_id}/handoffs?to_provider_id=codex"
            ))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    let codex_handoffs = to_codex["handoffs"].as_array().expect("handoffs array");
    assert_eq!(codex_handoffs.len(), 1);
    assert_eq!(codex_handoffs[0]["id"], second_handoff_id);

    let (status, invalid) = json_response_with_status(
        app,
        Request::builder()
            .uri(format!("/sessions/{session_id}/handoffs?status=paused"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid["error"], "invalid handoff status");
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
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
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
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    );
    assert!(!crate::helpers::is_provider_healthy(
        &state,
        "nonexistent_provider"
    ));
}

#[test]
fn unsupported_prompt_runtime_is_not_routable() {
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
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    );

    assert!(!crate::helpers::is_provider_routable(&state, "opencode"));
    assert!(!crate::helpers::is_provider_routable(&state, "copilot"));
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
async fn create_session_rejects_provider_without_runtime_support() {
    let (app, _data_dir, _project_dir) = test_app();
    let workspace = create_test_workspace(&app).await;
    let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");

    let (status, response) = json_response_with_status(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "try opencode",
                    "provider_id": "opencode"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response["error"],
        "provider opencode does not support Baize prompt execution yet"
    );
}

#[test]
fn select_provider_fallback_skips_unsupported_runtime() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let mut config = BaizeConfig::default();
    config.providers.order = vec!["opencode".to_string()];
    let state = AppState::with_executor(
        config,
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    );

    let result = crate::helpers::select_provider(&state, None, None, None);

    assert_ne!(result.provider_id.0, "opencode");
    assert!(baize_adapters::is_prompt_runtime_supported(
        &result.provider_id
    ));
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

#[tokio::test]
async fn sticky_routing_can_be_disabled_by_config() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project_dir = tempfile::tempdir().expect("project dir");
    let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
    let mut config = BaizeConfig::default();
    config.providers.order = vec!["codex".to_string()];
    config.routing.sticky_window_minutes = 0;
    let state = AppState::with_executor(
        config,
        store,
        Arc::new(FakeAgentExecutor {
            result: AgentRunResult {
                provider_id: ProviderId("codex".to_string()),
                success: true,
                exit_code: Some(0),
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
            },
        }),
    );
    let app = router(state);

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

    let first_session = json_response(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "workspace_id": workspace_id,
                    "objective": "first session",
                    "provider_id": "gemini"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(first_session["session"]["active_provider_id"], "gemini");

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

    assert_eq!(second_session["session"]["active_provider_id"], "codex");
    let reason = second_session["route_decision"]["reason"]
        .as_str()
        .expect("reason");
    assert!(
        !reason.contains("Sticky routing"),
        "sticky routing should be disabled, got: {reason}"
    );
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
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
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
                native_session_id: None,
                events: vec![],
                stderr: String::new(),
                error: None,
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

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Baize Test")
        .env("GIT_AUTHOR_EMAIL", "baize@example.invalid")
        .env("GIT_COMMITTER_NAME", "Baize Test")
        .env("GIT_COMMITTER_EMAIL", "baize@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
