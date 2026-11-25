use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

#[path = "execution_recovery_tests.rs"]
mod recovery;

struct NativeExecutor {
    executed: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    confirmed_stop: bool,
}
impl AgentExecutor for NativeExecutor {
    fn run_prompt(
        &self,
        _: AgentPromptRequest,
        _: Option<&mut baize_adapters::AgentEventSink<'_>>,
    ) -> Result<AgentRunResult> {
        anyhow::bail!("controlled entry required")
    }
    fn run_prompt_controlled(
        &self,
        request: AgentPromptRequest,
        sink: Option<&mut baize_adapters::AgentEventSink<'_>>,
        control: &baize_adapters::AgentControl,
    ) -> Result<AgentRunResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        control.register_approval("101")?;
        if let Some(sink) = sink {
            sink(AgentExecutionEvent {
                kind: AgentExecutionEventKind::Raw,
                text: None,
                stream: None,
                raw: Some(
                    json!({"native_approval":{"request_id":"101","command":"printf verified > verified\nWorking directory: isolated fixture\nScope: single native request","reason":"unit fixture"}}),
                ),
            });
        }
        let started = Instant::now();
        loop {
            let mut result = AgentRunResult {
                provider_id: request.provider_id.clone(),
                success: true,
                exit_code: Some(0),
                native_session_id: Some("native-controlled".into()),
                events: vec![],
                stderr: String::new(),
                error: None,
            };
            if control.is_canceled() {
                result.success = false;
                result.exit_code = None;
                result.stderr = "stop evidence".into();
                result.error = Some(AgentErrorDetail {
                    kind: if self.confirmed_stop {
                        AgentErrorKind::Canceled
                    } else {
                        AgentErrorKind::ExecutionUnconfirmed
                    },
                    message: "fixture stop outcome".into(),
                    source: AgentErrorSource::Runtime,
                });
                return Ok(result);
            }
            if let Some(approve) = control.take_decision("101")? {
                self.executed.store(approve, Ordering::SeqCst);
                return Ok(result);
            }
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(3),
                "fixture approval timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

async fn waiting(app: &Router, id: &str) -> serde_json::Value {
    for _ in 0..100 {
        let response = api(
            app,
            Method::GET,
            &format!("/permissions?status=Pending&session_id={id}"),
            json!(null),
        )
        .await
        .1;
        if let Some(permission) = response["permissions"].as_array().and_then(|p| p.first()) {
            return permission.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture native request not published");
}

#[tokio::test]
async fn native_permission_http_decision_reaches_exact_execution_once_and_denial_does_not_execute()
{
    for approve in [false, true] {
        let (mut state, app, _data, project, _) = environment();
        let (_, id) = setup_workspace_and_session(&app, &project).await;
        let executed = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        state.executor = Arc::new(NativeExecutor {
            executed: executed.clone(),
            calls: calls.clone(),
            confirmed_stop: true,
        });
        let app = router(state);
        let request_app = app.clone();
        let request_id = id.clone();
        let work = tokio::spawn(async move {
            api(
                &request_app,
                Method::POST,
                &format!("/sessions/{request_id}/prompt"),
                json!({"prompt":"native approval fixture"}),
            )
            .await
        });
        let permission = waiting(&app, &id).await;
        assert_eq!(permission["native"]["provider_id"], "codex");
        let verb = if approve { "approve" } else { "deny" };
        let path = format!("/permissions/{}/{verb}", permission["id"].as_str().unwrap());
        assert_eq!(
            api(&app, Method::POST, &path, json!({})).await.0,
            StatusCode::OK
        );
        assert_eq!(
            api(&app, Method::POST, &path, json!({})).await.0,
            StatusCode::CONFLICT
        );
        assert_eq!(work.await.unwrap().0, StatusCode::OK);
        assert_eq!(executed.load(Ordering::SeqCst), approve);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cancellation_reaches_worker_expires_approval_and_commits_confirmed_stop_without_a_receipt()
{
    for confirmed_stop in [true, false] {
        let (mut state, app, data, project, _) = environment();
        let (_, id) = setup_workspace_and_session(&app, &project).await;
        let executed = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        state.executor = Arc::new(NativeExecutor {
            executed: executed.clone(),
            calls: calls.clone(),
            confirmed_stop,
        });
        let app = router(state.clone());
        let request_app = app.clone();
        let request_id = id.clone();
        let work = tokio::spawn(async move {
            api(
                &request_app,
                Method::POST,
                &format!("/sessions/{request_id}/prompt"),
                json!({"prompt":"stop this exact execution"}),
            )
            .await
        });
        let permission = waiting(&app, &id).await;
        let response = api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/cancel"),
            json!({}),
        )
        .await;
        assert_eq!(response.0, StatusCode::OK);
        assert_eq!(
            response.1["session"]["continuity"]["last_stop"]["status"],
            "stopping"
        );
        assert_eq!(
            api(
                &app,
                Method::POST,
                &format!(
                    "/permissions/{}/approve",
                    permission["id"].as_str().unwrap()
                ),
                json!({})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(work.await.unwrap().0, StatusCode::CONFLICT);
        let response = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
            .await
            .1;
        assert_eq!(
            response["session"]["continuity"]["last_stop"]["status"],
            if confirmed_stop {
                "stopped"
            } else {
                "unconfirmed"
            }
        );
        assert_eq!(
            response["session"]["continuity"]["active_turn"].is_null(),
            confirmed_stop
        );
        assert_eq!(response["session"]["continuity"]["checkpoints"], json!({}));
        assert!(!executed.load(Ordering::SeqCst));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let pending = api(
            &app,
            Method::GET,
            &format!("/permissions/{}", permission["id"].as_str().unwrap()),
            json!(null),
        )
        .await
        .1;
        assert_eq!(pending["permission"]["status"], "Expired");
        let reopened = AppState::with_executor(
            BaizeConfig::default(),
            EventStore::open(data.path().join("baize.db")).unwrap(),
            state.executor.clone(),
        );
        let saved = reopened
            .store
            .lock()
            .unwrap()
            .get_task_session(&TaskSessionId(id))
            .unwrap()
            .unwrap();
        assert_eq!(saved.continuity.active_turn.is_none(), confirmed_stop);
    }
}

#[tokio::test]
async fn disconnected_http_client_does_not_cancel_but_native_stop_evidence_survives_it() {
    let (mut state, app, _data, project, _) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let executed = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    state.executor = Arc::new(NativeExecutor {
        executed: executed.clone(),
        calls: calls.clone(),
        confirmed_stop: true,
    });
    let app = router(state.clone());
    let request_app = app.clone();
    let request_id = id.clone();
    let work = tokio::spawn(async move {
        api(
            &request_app,
            Method::POST,
            &format!("/sessions/{request_id}/prompt"),
            json!({"prompt":"client disconnect fixture"}),
        )
        .await
    });
    let permission = waiting(&app, &id).await;
    work.abort();
    let _ = work.await;
    let current = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    let attempt = current["session"]["continuity"]["active_turn"]["id"]
        .as_str()
        .unwrap();
    assert!(!state
        .executions
        .get(&baize_core::EventId(attempt.into()))
        .unwrap()
        .unwrap()
        .is_canceled());
    assert_eq!(
        prompt(&app, &id, "codex", &format!("/reconcile {attempt}")).await["not_submitted"],
        true
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!(
                "/permissions/{}/approve",
                permission["id"].as_str().unwrap()
            ),
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/cancel"),
        json!({}),
    )
    .await;
    for _ in 0..100 {
        let current = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
            .await
            .1;
        if current["session"]["continuity"]["last_stop"]["status"] == "stopped" {
            assert!(current["session"]["continuity"]["active_turn"].is_null());
            assert!(!executed.load(Ordering::SeqCst));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("native stop evidence did not survive disconnected request");
}

#[tokio::test]
async fn native_approval_cannot_be_rebound_to_another_attempt() {
    let (mut state, app, _data, project, _) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let executed = Arc::new(AtomicBool::new(false));
    state.executor = Arc::new(NativeExecutor {
        executed: executed.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
        confirmed_stop: true,
    });
    let app = router(state.clone());
    let request_app = app.clone();
    let request_id = id.clone();
    let work = tokio::spawn(async move {
        api(
            &request_app,
            Method::POST,
            &format!("/sessions/{request_id}/prompt"),
            json!({"prompt":"binding fixture"}),
        )
        .await
    });
    let permission = waiting(&app, &id).await;
    {
        let store = state.store.lock().unwrap();
        let mut permission = store
            .get_permission(&baize_core::PermissionId(
                permission["id"].as_str().unwrap().into(),
            ))
            .unwrap()
            .unwrap();
        permission.native.as_mut().unwrap().attempt_id = baize_core::EventId::new();
        store.upsert_permission(&permission).unwrap();
    }
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!(
                "/permissions/{}/approve",
                permission["id"].as_str().unwrap()
            ),
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/cancel"),
        json!({}),
    )
    .await;
    let _ = work.await.unwrap();
    assert!(!executed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn stop_current_execution_preserves_the_conversation_and_allows_handoff_only_after_confirmation(
) {
    let (mut state, app, _data, project, _) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/execution/cancel"),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            "/sessions/missing/execution/cancel",
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    state.executor = Arc::new(NativeExecutor {
        executed: Arc::new(AtomicBool::new(false)),
        calls: Arc::new(AtomicUsize::new(0)),
        confirmed_stop: true,
    });
    let app = router(state);
    let request_app = app.clone();
    let request_id = id.clone();
    let work = tokio::spawn(async move {
        api(
            &request_app,
            Method::POST,
            &format!("/sessions/{request_id}/prompt"),
            json!({"prompt":"stop only this execution"}),
        )
        .await
    });
    waiting(&app, &id).await;
    let handoff = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/handoff"),
        json!({"to_provider_id":"antigravity"}),
    )
    .await
    .1;
    let accept = format!(
        "/sessions/{id}/handoff/{}/accept",
        handoff["handoff"]["id"].as_str().unwrap()
    );
    assert_eq!(
        api(&app, Method::POST, &accept, json!({})).await.0,
        StatusCode::BAD_REQUEST
    );
    let stopping = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/execution/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(stopping.0, StatusCode::OK);
    assert_eq!(stopping.1["session"]["status"], "Running");
    assert_eq!(
        stopping.1["session"]["continuity"]["last_stop"]["status"],
        "stopping"
    );
    assert_eq!(work.await.unwrap().0, StatusCode::CONFLICT);
    let stopped = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    assert_eq!(stopped["session"]["status"], "Running");
    assert!(stopped["session"]["continuity"]["active_turn"].is_null());
    assert_eq!(
        stopped["session"]["continuity"]["last_stop"]["status"],
        "stopped"
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/execution/cancel"),
            json!({})
        )
        .await
        .1["already_stopped"],
        true
    );
    assert_eq!(
        api(&app, Method::POST, &accept, json!({})).await.0,
        StatusCode::OK
    );
}
