use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    PreviewHandoff,
    AcceptHandoff,
    Approve,
    Deny,
    Cancel,
    Complete,
    Details,
    Requirements,
    Sessions,
}

#[derive(Debug)]
pub(crate) struct ActionResult {
    pub(super) action: Action,
    pub(super) session_id: Option<String>,
    pub(super) workspace_id: Option<String>,
    pub(super) response: Value,
    pub(super) extra: Value,
}

pub(super) fn request_action(state: &mut TuiState, task: &mut Option<UiTask>, action: Action) {
    if task.is_some() {
        state.ui.notice = Some("Wait for the current background operation".into());
        return;
    }
    if !matches!(action, Action::Sessions | Action::Approve | Action::Deny)
        && state.session_id.is_none()
    {
        state.ui.notice = Some("Start or open a session first".into());
        return;
    }
    let session_id = state.session_id.clone();
    let workspace_id = state.workspace_id.clone();
    let provider = state.selected_provider().to_string();
    let draft = if state.input.trim().is_empty() {
        "<context preview request>".into()
    } else {
        state.input.clone()
    };
    let handoff = state.pending_handoff_id.clone();
    let permission = state.selected_permission().map(|p| p.id.clone());
    if action == Action::PreviewHandoff && state.active_provider.as_deref() == Some(&provider) {
        state.ui.notice = Some("Choose another provider with Tab first".into());
        return;
    }
    if action == Action::AcceptHandoff
        && (handoff.is_none() || state.pending_handoff_session_id != session_id)
    {
        state.ui.notice = Some("Prepare and review a handoff first".into());
        return;
    }
    if matches!(action, Action::Approve | Action::Deny) && permission.is_none() {
        state.ui.notice = Some("No pending permission selected".into());
        return;
    }
    state.background_status = Some(format!("{:?}", action));
    state.ui.notice = None;
    *task = Some(start_ui_task(move || {
        let id = session_id.as_deref().unwrap_or_default();
        let mut extra = Value::Null;
        let response = match action {
            Action::PreviewHandoff => post_json(
                &format!("/sessions/{id}/handoff"),
                json!({"to_provider_id":provider,"prompt":draft}),
            )?,
            Action::AcceptHandoff => post_json(
                &format!(
                    "/sessions/{id}/handoff/{}/accept",
                    handoff.as_deref().unwrap_or_default()
                ),
                json!({}),
            )?,
            Action::Approve | Action::Deny => {
                let verb = if action == Action::Approve {
                    "approve"
                } else {
                    "deny"
                };
                post_json(
                    &format!(
                        "/permissions/{}/{verb}",
                        permission.as_deref().unwrap_or_default()
                    ),
                    json!({}),
                )?
            }
            Action::Cancel => post_json(&format!("/sessions/{id}/execution/cancel"), json!({}))?,
            Action::Complete => post_json(&format!("/sessions/{id}/complete"), json!({}))?,
            Action::Details => {
                extra = json!({"context_response":post_json(&format!("/sessions/{id}/context-preview"),json!({"provider_id":provider,"prompt":draft}))?,"events_response":load_session_events(id)?,"session_response":get_json(&format!("/sessions/{id}"))?});
                get_json(&format!("/sessions/{id}/diff"))?
            }
            Action::Requirements => get_json(&format!("/sessions/{id}/requirements"))?,
            Action::Sessions => {
                let sessions =
                    load_workspace_sessions(workspace_id.as_deref().unwrap_or_default())?;
                session_views_json(&sessions)
            }
        };
        Ok(UiTaskCompletion::Action(Box::new(ActionResult {
            action,
            session_id,
            workspace_id,
            response,
            extra,
        })))
    }));
}

pub(super) fn apply_action(state: &mut TuiState, result: ActionResult) {
    if result.action == Action::Sessions && result.workspace_id != state.workspace_id {
        state.ui.notice =
            Some("Ignored a response for a different workspace; refresh to reconcile".into());
        return;
    }
    if !matches!(
        result.action,
        Action::Approve | Action::Deny | Action::Sessions
    ) && result.session_id != state.session_id
    {
        state.ui.notice =
            Some("Ignored a response for a different session; refresh to reconcile".into());
        return;
    }
    match result.action {
        Action::PreviewHandoff => {
            let handoff = &result.response["handoff"];
            state.pending_handoff_id = handoff["id"].as_str().map(str::to_string);
            state.pending_handoff_session_id = state.session_id.clone();
            state.ui.handoff = handoff.clone();
            if result.response.get("context_preview").is_some() {
                state.ui.context = result.response["context_preview"].clone();
            }
            state.ui.panel = Panel::Handoff;
            state.detail_mode = true;
            state.ui.focus = Focus::Inspector;
            state.ui.inspector_scroll = 0;
        }
        Action::AcceptHandoff => {
            append_handoff_response(state, &result.response);
            if let Some(provider) = &state.active_provider {
                if let Some(index) = state.providers.iter().position(|p| p == provider) {
                    state.selected_provider_index = index;
                }
            }
            state.pending_handoff_id = None;
            state.pending_handoff_session_id = None;
            state.ui.handoff = result.response["handoff"].clone();
            state.ui.notice = Some("Handoff accepted. Continue in this conversation.".into());
        }
        Action::Approve | Action::Deny => {
            let id = result.response["permission"]["id"].as_str();
            state
                .pending_permissions
                .retain(|p| Some(p.id.as_str()) != id);
            state.selected_permission_index = state
                .selected_permission_index
                .min(state.pending_permissions.len().saturating_sub(1));
            append_permission_resolution(state, &result.response);
        }
        Action::Cancel | Action::Complete => {
            state.ui.session_status = result.response["session"]["status"]
                .as_str()
                .unwrap_or("Unknown")
                .into();
            if result.action == Action::Cancel {
                state.ui.execution_uncertain |= state.activity_status.starts_with("running");
                append_cancel_response(state, &result.response);
                apply_continuity_status(state, &result.response["session"]);
                if result.response["session"]["continuity"]["last_stop"].is_null() {
                    state.ui.notice = Some("Session canceled. The provider process may still be running; inspect workspace before starting more work.".into());
                }
            } else {
                append_complete_response(state, &result.response);
            }
            if let Some(session) = state
                .recent_sessions
                .iter_mut()
                .find(|s| Some(&s.id) == state.session_id.as_ref())
            {
                session.status = state.ui.session_status.clone();
            }
        }
        Action::Requirements => {
            state.ui.context = json!({"continuity":result.response["continuity"]});
            apply_continuity_status(state, &result.response);
            state.ui.panel = Panel::Context;
            state.ui.focus = Focus::Inspector;
            state.ui.inspector_scroll = 0;
            state.detail_mode = true;
        }
        Action::Details => {
            state.ui.diff = result.response["diff"].clone();
            state.ui.context = result.extra["context_response"].clone();
            state.ui.session_status = result.extra["session_response"]["session"]["status"]
                .as_str()
                .unwrap_or("Unknown")
                .into();
            if let Err(error) = replay_session(state, &result.extra["events_response"]) {
                state.ui.notice = Some(error.to_string());
            }
            apply_continuity_status(state, &result.extra["session_response"]["session"]);
        }
        Action::Sessions => match parse_session_views(&result.response) {
            Ok(sessions) => {
                state.set_recent_sessions(sessions);
                if let Some(id) = state.session_id.clone() {
                    state.select_recent_session(&id);
                }
            }
            Err(error) => state.ui.notice = Some(error.to_string()),
        },
    }
}

pub(super) fn start_workspace_load_task() -> UiTask {
    start_ui_task(|| {
        let (workspace_id, display) = create_workspace_from_current_dir()?;
        let sessions = load_workspace_sessions(&workspace_id)?;
        Ok(UiTaskCompletion::WorkspaceLoad {
            workspace_id,
            display: display.unwrap_or_default(),
            sessions,
        })
    })
}

fn session_views_json(sessions: &[SessionView]) -> Value {
    json!({"sessions":sessions.iter().map(|s|json!({"id":s.id,"workspace_id":s.workspace_id,"objective":s.objective,"active_provider_id":s.active_provider_id,"status":s.status})).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_results_are_applied_after_a_first_prompt_creates_its_session() {
        for action in [Action::Approve, Action::Deny] {
            let mut state = TuiState {
                pending_permissions: vec![PermissionView {
                    id: "perm_reviewed".into(),
                    session_id: Some("task_other".into()),
                    command: "cargo test".into(),
                    reason: "reviewed".into(),
                    status: "Pending".into(),
                    risk_level: None,
                }],
                ..TuiState::default()
            };
            // The permission action started before this new session ID arrived.
            state.session_id = Some("task_new".into());
            apply_action(
                &mut state,
                ActionResult {
                    action,
                    session_id: None,
                    workspace_id: None,
                    response: json!({"permission":{"id":"perm_reviewed","status":"Resolved"}}),
                    extra: Value::Null,
                },
            );
            assert!(state.pending_permissions.is_empty());
            assert_eq!(state.session_id.as_deref(), Some("task_new"));
        }
    }

    #[test]
    fn session_lists_are_scoped_to_workspace_while_the_current_session_can_change() {
        let mut state = TuiState {
            workspace_id: Some("ws_one".into()),
            session_id: Some("task_new".into()),
            ..TuiState::default()
        };
        apply_action(
            &mut state,
            ActionResult {
                action: Action::Sessions,
                session_id: None,
                workspace_id: Some("ws_one".into()),
                response: json!({"sessions":[{"id":"task_new","workspace_id":"ws_one","objective":"new"}]}),
                extra: Value::Null,
            },
        );
        assert_eq!(state.recent_sessions.len(), 1);
        assert_eq!(state.recent_sessions[0].id, "task_new");
        apply_action(
            &mut state,
            ActionResult {
                action: Action::Sessions,
                session_id: Some("task_new".into()),
                workspace_id: Some("ws_other".into()),
                response: json!({"sessions":[]}),
                extra: Value::Null,
            },
        );
        assert_eq!(state.recent_sessions.len(), 1);
        assert!(state.ui.notice.as_deref().unwrap().contains("workspace"));
    }

    #[test]
    fn rejects_missing_and_concurrent_actions_without_network() {
        let mut state = TuiState::default();
        let mut task = None;
        request_action(&mut state, &mut task, Action::AcceptHandoff);
        assert!(task.is_none());
        assert!(state.ui.notice.as_deref().unwrap().contains("session"));
        let (_, receiver) = mpsc::channel();
        task = Some(UiTask { receiver });
        request_action(&mut state, &mut task, Action::Sessions);
        assert!(state.ui.notice.as_deref().unwrap().contains("Wait"));
    }
    #[test]
    fn ignores_stale_actions_and_applies_preview_without_changing_identity() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            ..TuiState::default()
        };
        apply_action(
            &mut state,
            ActionResult {
                action: Action::Cancel,
                session_id: Some("other".into()),
                workspace_id: None,
                response: json!({}),
                extra: Value::Null,
            },
        );
        assert_eq!(state.ui.session_status, "New");
        apply_action(
            &mut state,
            ActionResult {
                action: Action::PreviewHandoff,
                session_id: Some("task_1".into()),
                workspace_id: None,
                response: json!({"handoff":{"id":"h_1","summary_markdown":"current facts"}}),
                extra: Value::Null,
            },
        );
        assert_eq!(state.pending_handoff_id.as_deref(), Some("h_1"));
        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert_eq!(state.ui.panel, Panel::Handoff);
    }
}
