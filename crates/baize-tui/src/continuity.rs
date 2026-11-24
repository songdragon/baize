use super::*;

pub(super) fn is_continuity_command(text: &str) -> bool {
    matches!(
        text.split_whitespace().next(),
        Some("/remember" | "/forget" | "/requirements" | "/reconcile" | "/reset-provider")
    )
}

pub(super) fn can_reconcile(state: &TuiState) -> bool {
    state
        .ui
        .uncertain_turn_id
        .as_ref()
        .is_some_and(|id| state.input.trim() == format!("/reconcile {id}"))
}

pub(super) fn apply_continuity_status(state: &mut TuiState, value: &Value) {
    let Some(continuity) = value.get("continuity") else {
        return;
    };
    let Some(turn) = continuity.get("active_turn") else {
        return;
    };
    state.ui.execution_uncertain = !turn.is_null();
    state.ui.uncertain_turn_id = if turn["uncertain"] == true {
        turn["id"].as_str().map(str::to_string)
    } else {
        None
    };
    if let Some(id) = state.ui.uncertain_turn_id.as_deref() {
        state.ui.notice = Some(format!("Execution {id} is uncertain. Check the provider has stopped and inspect changes; /reconcile {id} acknowledges that review without replaying."));
    } else if turn.is_null()
        && state.ui.notice.as_deref().is_some_and(|notice| {
            notice.starts_with("Execution ") && notice.contains("is uncertain")
        })
    {
        state.ui.notice = None;
    }
}

pub(super) fn continuity_context_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw(
        "Saved requirements: /remember <key> <text> · /forget <key> · /requirements",
    )];
    let value = &state.ui.context;
    if value.is_null() {
        lines.push(Line::raw(
            "F5 previews context for the selected provider and current draft.",
        ));
        return lines;
    }
    let summary = &value["context"];
    lines.push(Line::raw(
        "Snapshot only; sending rebuilds context from the current draft and workspace.",
    ));
    if summary["provider_id"]
        .as_str()
        .is_some_and(|provider| provider != state.selected_provider())
    {
        lines.push(Line::raw(
            "Selected target changed; F5 rebuilds its preview.",
        ));
    }
    lines.push(Line::raw(format!(
        "Preview only · {} · {} estimated tokens · {}",
        summary["provider_id"].as_str().unwrap_or("unknown"),
        summary["estimated_tokens"],
        if summary["blocked"] == true {
            "BLOCKED: required context does not fit"
        } else {
            "not delivered"
        }
    )));
    if let Some(warnings) = summary["warnings"].as_array() {
        for warning in warnings {
            if let Some(text) = warning.as_str() {
                lines.push(Line::styled(
                    text.to_string(),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
    }
    if let Some(requirements) = value["continuity"]["requirements"].as_object() {
        for (key, requirement) in requirements {
            lines.push(Line::raw(format!(
                "{key} [{} r{}]: {}",
                if requirement["active"] == true {
                    "active"
                } else {
                    "revoked"
                },
                requirement["revision"],
                requirement["text"].as_str().unwrap_or("")
            )));
        }
    }
    if let Some(turn) = value["continuity"]["active_turn"].as_object() {
        lines.push(Line::raw(format!(
            "Execution {} · uncertain={}",
            turn.get("id").unwrap_or(&Value::Null),
            turn.get("uncertain").unwrap_or(&Value::Null)
        )));
        lines.push(Line::raw(
            "Check execution has stopped, inspect changes, then /reconcile <turn-id>.",
        ));
    }
    lines.push(Line::raw(
        "Native reference invalid? /reset-provider <provider> clears it without replaying.",
    ));
    if let Some(sources) = summary["sources"].as_array() {
        lines.push(Line::raw("SOURCES"));
        for source in sources {
            lines.push(Line::raw(format!(
                "{} {} {} · {}{}",
                source["event_id"].as_str().unwrap_or("?"),
                source["role"].as_str().unwrap_or("?"),
                source["provider_id"].as_str().unwrap_or("unknown"),
                if source["included"] == true {
                    "included"
                } else {
                    "omitted"
                },
                if source["truncated"] == true {
                    " (excerpt)"
                } else {
                    ""
                }
            )));
        }
    }
    lines.push(Line::raw("PROMPT PREVIEW (sensitive lines redacted)"));
    if let Some(preview) = value["prompt_preview"].as_str() {
        lines.extend(preview.lines().map(|line| Line::raw(line.to_string())));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn capture_real_continuity_session_without_provider_calls() {
        let Ok(directory) = std::env::var("BAIZE_CONTINUITY_EVIDENCE_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let load = |name: &str| -> Value {
            serde_json::from_slice(&std::fs::read(directory.join(name)).unwrap()).unwrap()
        };
        let mut state = TuiState {
            workspace: "Baize continuity acceptance".into(),
            detail_mode: true,
            ..TuiState::default()
        };
        apply_loaded_session(&mut state, &load("final-session.json")["session"]).unwrap();
        replay_session(&mut state, &load("final-events.json")).unwrap();
        apply_continuity_status(&mut state, &load("final-session.json")["session"]);
        let session = load("final-session.json")["session"].clone();
        state.ui.session_status = session["status"].as_str().unwrap().into();
        state.workspace = load("result.json")["workspace"].as_str().unwrap().into();
        state.recent_sessions = vec![SessionView {
            id: session["id"].as_str().unwrap().into(),
            workspace_id: session["workspace_id"].as_str().unwrap().into(),
            objective: session["objective"].as_str().unwrap().into(),
            active_provider_id: Some("codex".into()),
            status: state.ui.session_status.clone(),
        }];
        state.ui.context = load("3-preview.json");
        state.ui.panel = Panel::Context;
        state.ui.focus = Focus::Inspector;
        state.daemon_status = "daemon: archived acceptance evidence (offline render)".into();
        for (width, height) in [(132, 40), (100, 32), (40, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let frame = terminal.draw(|frame| render(frame, &state)).unwrap();
            let cells=frame.buffer.content.iter().map(|cell|json!({"symbol":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg)})).collect::<Vec<_>>();
            std::fs::write(
                directory.join(format!("tui-{width}x{height}.json")),
                serde_json::to_vec(&json!({"width":width,"height":height,"cells":cells})).unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn restored_requirement_commands_keep_provider_turns_and_user_sources_separate() {
        let mut state = TuiState::default();
        replay_session(&mut state,&json!({"events":[
            {"id":"1","provider_id":"codex","event_type":"session.agent.started","payload":{"prompt":"implement"}},
            {"id":"2","provider_id":"codex","event_type":"session.agent.output","payload":{"text":"original agent answer"}},
            {"id":"3","provider_id":"codex","event_type":"session.agent.completed","payload":{"success":true}},
            {"id":"4","event_type":"session.requirement.changed","payload":{"prompt":"/remember language 中文回答","message":"Saved requirement language"}},
            {"id":"5","provider_id":"antigravity","event_type":"session.agent.started","payload":{"prompt":"continue"}}
        ]})).unwrap();
        assert_eq!(state.ui.output.turns.len(), 3);
        assert_eq!(state.ui.output.turns[0].provider, "codex");
        assert_eq!(state.ui.output.turns[1].provider, "Baize");
        assert_eq!(state.ui.output.turns[2].provider, "antigravity");
        assert!(state.ui.output.turns[0]
            .items
            .iter()
            .all(|item| !item.text.contains("Saved requirement")));
        assert!(state
            .transcript
            .iter()
            .any(|line| line.contains("/remember language")));
    }

    #[test]
    fn rejected_submission_restores_draft_and_preserves_another_clients_unknown_execution() {
        for uncertain in [false, true] {
            let mut state = TuiState {
                session_id: Some("task_1".into()),
                ..TuiState::default()
            };
            state.ui.last_submission = Some(PendingPromptSubmission {
                prompt: "keep my request".into(),
                provider_id: "antigravity".into(),
                image_paths: vec![],
                workspace_id: Some("ws_1".into()),
                session_id: Some("task_1".into()),
            });
            let turn = if uncertain {
                json!({"id":"evt_other","provider_id":"codex","uncertain":true})
            } else {
                Value::Null
            };
            let (sender, receiver) = mpsc::channel();
            sender.send(PromptTaskMessage::Rejected {response:json!({"error":"not dispatched","not_submitted":true}),session:json!({"active_provider_id":"codex","continuity":{"active_turn":turn}})}).unwrap();
            let mut task = Some(PromptTask {
                receiver,
                stop_stream: Arc::new(AtomicBool::new(false)),
            });
            poll_prompt_task(&mut state, &mut task);
            assert!(task.is_none());
            assert_eq!(state.input, "keep my request");
            assert_eq!(state.active_provider.as_deref(), Some("codex"));
            assert_eq!(state.ui.execution_uncertain, uncertain);
        }
    }

    #[test]
    fn reconciliation_requires_the_exact_uncertain_turn_and_keeps_legacy_behavior() {
        let mut state = TuiState::default();
        state.ui.execution_uncertain = true;
        apply_continuity_status(&mut state, &json!({}));
        assert!(state.ui.execution_uncertain);
        apply_continuity_status(
            &mut state,
            &json!({"continuity":{"active_turn":{"id":"evt_exact","uncertain":true}}}),
        );
        state.input = "/reconcile evt_other".into();
        assert!(!can_reconcile(&state));
        state.input = "/reconcile evt_exact".into();
        assert!(can_reconcile(&state));
        apply_continuity_status(&mut state, &json!({"continuity":{"active_turn":null}}));
        assert!(!state.ui.execution_uncertain && !can_reconcile(&state));
    }

    #[test]
    fn preview_renders_requirements_sources_omissions_and_budget_failure() {
        let mut state = TuiState::default();
        state.ui.context = json!({"context":{"provider_id":"codex","blocked":true,"estimated_tokens":99,"warnings":["budget omitted output"],"sources":[{"event_id":"evt_source","role":"user","provider_id":"antigravity","included":true,"truncated":false}]},"continuity":{"requirements":{"language":{"text":"中文回答","active":true,"revision":3}}},"prompt_preview":"quoted current workspace"});
        let mut terminal = Terminal::new(TestBackend::new(110, 20)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(continuity_context_lines(&state)),
                    frame.area(),
                )
            })
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for expected in [
            "Preview only",
            "BLOCKED",
            "language [active r3]",
            "evt_source",
            "antigravity",
            "quoted current workspace",
            "budget omitted output",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        state.ui.context = Value::Null;
        assert!(continuity_context_lines(&state)
            .iter()
            .any(|line| line.to_string().contains("F5")));
    }

    #[test]
    fn continuity_commands_are_local_only_and_require_an_existing_session() {
        let mut state = TuiState {
            input: "/remember language 中文回答".into(),
            ..TuiState::default()
        };
        assert!(begin_prompt_submission(&mut state).is_none());
        assert!(!state.input.is_empty());
        assert!(is_continuity_command("/remember"));
        assert!(!is_continuity_command("/remembered example"));
        assert!(!is_continuity_command("Implement feature"));
    }
}
