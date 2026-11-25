use super::*;

pub(super) fn apply_native_permission_event(state: &mut TuiState, event: &Value) -> bool {
    let kind = event["event_type"].as_str().unwrap_or("");
    if !matches!(kind, "permission.requested" | "permission.resolved") {
        return false;
    }
    let permission = &event["payload"]["permission"];
    if state.session_id.is_none()
        || permission["session_id"].as_str() != state.session_id.as_deref()
    {
        return true;
    }
    let Some(id) = permission["id"].as_str() else {
        return true;
    };
    let mut pending = state.pending_permissions.clone();
    pending.retain(|existing| existing.id != id);
    if permission["status"] == "Pending" {
        if let Ok(parsed) = parse_permissions(&json!({"permissions":[permission]})) {
            pending.extend(parsed);
        }
        state.ui.panel = Panel::Permissions;
        state.detail_mode = true;
        state.ui.reviewed_permission_id = None;
        state.ui.notice=Some("Native operation is waiting. Review command, directory and scope; Ctrl-A approves once, Ctrl-D denies.".into());
    }
    state.set_pending_permissions(pending);
    true
}

pub(super) fn apply_execution_stop(state: &mut TuiState, continuity: &Value) {
    let stop = &continuity["last_stop"];
    if !stop.is_object() {
        return;
    }
    let id = stop["attempt_id"].as_str().unwrap_or("unknown");
    let detail = stop["detail"].as_str().unwrap_or("");
    let (activity, label) = match stop["status"].as_str() {
        Some("stopping") => ("stopping execution", "Stopping execution"),
        Some("stopped") => ("execution stopped", "Execution stopped"),
        Some("unconfirmed") => ("execution unconfirmed", "Stop unconfirmed"),
        _ => return,
    };
    state.activity_status = activity.into();
    state.ui.notice = Some(format!("{label} · {id}. {detail}"));
}

pub(super) fn execution_stop_lines(continuity: &Value) -> Vec<Line<'static>> {
    let stop = &continuity["last_stop"];
    let Some(status) = stop["status"].as_str() else {
        return vec![];
    };
    vec![
        Line::raw(format!(
            "Execution stop: {status} · {}",
            stop["attempt_id"].as_str().unwrap_or("unknown")
        )),
        Line::raw(stop["detail"].as_str().unwrap_or("").to_string()),
    ]
}

pub(super) fn apply_execution_stop_event(state: &mut TuiState, event: &Value) {
    if event["session_id"].as_str() != state.session_id.as_deref()
        || !event["payload"]["continuity"].is_object()
    {
        return;
    }
    let continuity = &event["payload"]["continuity"];
    apply_continuity_status(state, &json!({"continuity":continuity}));
    if let Some(context) = state.ui.context.as_object_mut() {
        context.insert("continuity".into(), continuity.clone());
    }
}

pub(super) fn provider_control_lines(state: &TuiState) -> Vec<Line<'static>> {
    let controls = state
        .runtime_policy
        .as_ref()
        .map(|policy| &policy.native_control);
    let provider = state.selected_provider();
    let supported = controls.and_then(|value| value["approvals"][provider].as_u64());
    let entry = controls
        .and_then(|value| value["entrypoints"][provider].as_str())
        .unwrap_or("unverified");
    vec![
        Line::raw(format!("Actual entry: {entry}")),
        Line::raw(match supported {
            Some(1..) => "Native approvals: connected; single-request decisions",
            Some(0) => "Native approvals: unavailable here; use the provider's native entry",
            None => "Native approvals: unverified; refresh current daemon status",
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    #[test]
    fn provider_control_reports_supported_unavailable_and_unverified_entries() {
        for (provider, expected) in [
            ("codex", "connected"),
            ("antigravity", "unavailable"),
            ("unknown", "unverified"),
        ] {
            let mut state = TuiState {
                providers: vec![provider.into()],
                ..TuiState::default()
            };
            let mut policy = default_runtime_policy();
            policy.native_control = json!({"approvals":{"codex":1,"antigravity":0},"entrypoints":{"codex":"codex app-server stdio","antigravity":"agy print"}});
            state.runtime_policy = Some(policy);
            let mut terminal = Terminal::new(TestBackend::new(100, 8)).unwrap();
            terminal
                .draw(|frame| {
                    frame
                        .render_widget(Paragraph::new(provider_control_lines(&state)), frame.area())
                })
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains(expected));
        }
    }
    #[test]
    fn native_approval_events_are_scoped_live_and_do_not_steal_or_send_the_draft() {
        let mut state = TuiState {
            session_id: Some("task_active".into()),
            input: "newer draft 中文".into(),
            ..TuiState::default()
        };
        let permission = json!({"id":"perm_native","session_id":"task_active","command":"printf once\nWorking directory: /fixture\nScope: this request only","reason":"native approval","status":"Pending"});
        let event = json!({"id":"evt_request","event_type":"permission.requested","payload":{"permission":permission}});
        apply_prompt_stream_event(&mut state, &event);
        apply_prompt_stream_event(&mut state, &event);
        assert_eq!(state.pending_permissions.len(), 1);
        assert_eq!(state.input, "newer draft 中文");
        assert_eq!(state.ui.focus, Focus::Composer);
        assert!(state.ui.reviewed_permission_id.is_none());
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("printf once") && text.contains("Working directory"));
        apply_prompt_stream_event(
            &mut state,
            &json!({"id":"evt_other","event_type":"permission.requested","payload":{"permission":{"id":"perm_other","session_id":"other","status":"Pending"}}}),
        );
        assert_eq!(state.pending_permissions.len(), 1);
        apply_prompt_stream_event(
            &mut state,
            &json!({"id":"evt_expired","event_type":"permission.resolved","payload":{"permission":{"id":"perm_native","session_id":"task_active","status":"Expired"}}}),
        );
        assert!(state.pending_permissions.is_empty());
        assert_eq!(state.input, "newer draft 中文");
    }
    #[test]
    fn stop_status_is_visible_without_releasing_an_unconfirmed_execution() {
        for status in ["stopping", "stopped", "unconfirmed"] {
            let mut state = TuiState::default();
            let active = if status == "stopped" {
                Value::Null
            } else {
                json!({"id":"evt_work","uncertain":status=="unconfirmed"})
            };
            let continuity = json!({"active_turn":active,"last_stop":{"attempt_id":"evt_work","status":status,"detail":"partial changes kept"}});
            apply_continuity_status(&mut state, &json!({"continuity":continuity}));
            assert_eq!(state.ui.execution_uncertain, status != "stopped");
            assert!(state.ui.notice.as_ref().unwrap().contains("evt_work"));
            state.ui.context = json!({"continuity":continuity});
            let rendered = continuity_context_lines(&state)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(rendered.contains(&format!("Execution stop: {status}")));
        }
    }
}
