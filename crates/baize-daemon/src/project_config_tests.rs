use super::*;
use crate::helpers::with_store;

async fn setup() -> (
    AppState,
    Router,
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<ContinuityExecutor>,
    String,
    String,
) {
    let (state, app, data, project, executor) = environment();
    fs::create_dir_all(project.path().join(".agents")).unwrap();
    fs::write(project.path().join(".agents/mcp_config.json"),json!({"mcpServers":{"fixture":{"command":"python3","args":["mcp.py"],"env":{"TOKEN":"never-in-api"}}}}).to_string()).unwrap();
    let (workspace, session) = setup_workspace_and_session(&app, &project).await;
    (state, app, data, project, executor, workspace, session)
}

#[tokio::test]
async fn previews_applies_and_restores_without_models_or_secret_exposure() {
    let (_, app, data, project, executor, workspace, _) = setup().await;
    let path = format!("/workspaces/{workspace}/project-config");
    let preview = api(&app, Method::GET, &path, json!(null)).await;
    assert_eq!(preview.0, StatusCode::OK);
    assert_eq!(preview.1["preview_only"], true);
    assert!(!preview.1.to_string().contains("never-in-api"));
    let outcome = api(
        &app,
        Method::POST,
        &path,
        json!({"action":"apply","plan_id":preview.1["project_config"]["plan_id"]}),
    )
    .await;
    assert_eq!(outcome.0, StatusCode::OK, "{}", outcome.1);
    assert_eq!(outcome.1["project_config"]["status"], "in_sync");
    assert!(data.path().join("artifacts/project-config").exists());
    assert!(!outcome.1.to_string().contains("never-in-api"));
    let restored = api(
        &app,
        Method::POST,
        &path,
        json!({"action":"restore","backup_id":outcome.1["change"]["backup_id"]}),
    )
    .await;
    assert_eq!(restored.0, StatusCode::OK);
    assert!(!project.path().join(".codex/config.toml").exists());
    assert!(executor.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_bad_identifiers_stale_plans_and_later_native_edits() {
    let (_, app, _data, project, _, workspace, _) = setup().await;
    let path = format!("/workspaces/{workspace}/project-config");
    assert_eq!(
        api(
            &app,
            Method::GET,
            "/workspaces/ws_missing/project-config",
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for body in [
        json!({"action":"unknown"}),
        json!({"action":"apply"}),
        json!({"action":"restore"}),
    ] {
        assert_eq!(
            api(&app, Method::POST, &path, body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        api(
            &app,
            Method::POST,
            &path,
            json!({"action":"restore","backup_id":"../../escape"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            &path,
            json!({"action":"apply","plan_id":"stale"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let preview = api(&app, Method::GET, &path, json!(null)).await.1;
    let applied = api(
        &app,
        Method::POST,
        &path,
        json!({"action":"apply","plan_id":preview["project_config"]["plan_id"]}),
    )
    .await
    .1;
    fs::write(
        project.path().join(".codex/config.toml"),
        "# later user edit\n",
    )
    .unwrap();
    assert_eq!(
        api(
            &app,
            Method::POST,
            &path,
            json!({"action":"restore","backup_id":applied["change"]["backup_id"]})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".codex/config.toml")).unwrap(),
        "# later user edit\n"
    );
}

#[tokio::test]
async fn any_active_workspace_turn_blocks_writes_even_beyond_the_first_page() {
    let (state, app, _data, project, executor, workspace, session) = setup().await;
    let path = format!("/workspaces/{workspace}/project-config");
    let preview = api(&app, Method::GET, &path, json!(null)).await.1;
    with_store(&state, |store| {
        let template = store
            .get_task_session(&TaskSessionId(session.clone()))?
            .unwrap();
        for index in 0..125 {
            let mut entry = template.clone();
            entry.id = TaskSessionId::new();
            entry.created_at = chrono::Utc::now();
            if index == 124 {
                entry.continuity.active_turn = Some(baize_core::ExecutionAttempt {
                    id: baize_core::EventId::new(),
                    provider_id: ProviderId("codex".into()),
                    uncertain: true,
                });
            }
            store.upsert_task_session(&entry)?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        api(&app, Method::GET, &path, json!(null)).await.0,
        StatusCode::OK
    );
    let rejected = api(
        &app,
        Method::POST,
        &path,
        json!({"action":"apply","plan_id":preview["project_config"]["plan_id"]}),
    )
    .await;
    assert_eq!(rejected.0, StatusCode::CONFLICT);
    assert!(rejected.1["error"].as_str().unwrap().contains("uncertain"));
    assert!(!project.path().join(".codex/config.toml").exists());
    assert!(executor.requests.lock().unwrap().is_empty());
}
