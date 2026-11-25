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
    let Some(continuity) = value.get("continuity").or_else(|| {
        value
            .get("session")
            .and_then(|session| session.get("continuity"))
    }) else {
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
    apply_execution_stop(state, continuity);
}

pub(super) fn continuity_context_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw(
        "Saved requirements: /remember <key> <text> · /forget <key> · /requirements",
    )];
    let value = &state.ui.context;
    lines.extend(execution_stop_lines(&value["continuity"]));
    if value.is_null() {
        lines.push(Line::raw(
            "F5 previews context for the selected provider and current draft.",
        ));
        return lines;
    }
    let summary = &value["context"];
    let preview_only = value["preview_only"] != false;
    if summary.is_object() {
        if preview_only {
            lines.push(Line::raw(
                "Snapshot only; sending rebuilds context from the current draft and workspace.",
            ));
        }
        if summary["provider_id"]
            .as_str()
            .is_some_and(|provider| provider != state.selected_provider())
        {
            lines.push(Line::raw(
                "Selected target changed; F5 rebuilds its preview.",
            ));
        }
        lines.push(Line::raw(format!(
            "{} · {} · {} estimated tokens · {}",
            if preview_only {
                "Preview only"
            } else {
                "Last submitted context"
            },
            summary["provider_id"].as_str().unwrap_or("unknown"),
            summary["estimated_tokens"],
            if summary["blocked"] == true {
                "BLOCKED: inspect warnings before sending"
            } else if preview_only {
                "not delivered"
            } else {
                value["outcome"].as_str().unwrap_or("outcome unconfirmed")
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
    } else {
        lines.push(Line::raw(
            "F5 previews context for the selected provider and current draft.",
        ));
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
            if let Some(source) = requirement["source_ref"].as_str() {
                lines.push(Line::raw(format!("  source: {source}")));
            }
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
    if let Some(preview) = value["prompt_preview"].as_str() {
        lines.push(Line::raw("PROMPT PREVIEW (sensitive lines redacted)"));
        lines.extend(preview.lines().map(|line| Line::raw(line.to_string())));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn attachments_can_be_detached_before_reconciling_an_unknown_execution() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            input: "/unimage".into(),
            image_paths: vec![PathBuf::from("newer-draft.png")],
            ..TuiState::default()
        };
        state.ui.execution_uncertain = true;
        state.ui.uncertain_turn_id = Some("evt_unknown".into());
        let (mut prompt, mut ui) = (None, None);
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut prompt,
            &mut ui,
        )
        .unwrap();
        assert!(state.image_paths.is_empty());
        assert!(state.input.is_empty());
        assert!(state.ui.execution_uncertain && prompt.is_none() && ui.is_none());
    }

    #[test]
    fn read_only_query_keeps_the_provider_submission_and_pending_execution() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            input: "newer input".into(),
            ..TuiState::default()
        };
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: "original provider request".into(),
            provider_id: "codex".into(),
            image_paths: vec![],
            workspace_id: Some("ws_1".into()),
            session_id: Some("task_1".into()),
        });
        apply_action(
            &mut state,
            ActionResult {
                action: Action::Requirements,
                session_id: Some("task_1".into()),
                workspace_id: None,
                response: json!({"continuity":{"active_turn":{"id":"evt_running","uncertain":true},"requirements":{"api":{"text":"keep API","active":true,"revision":1,"source_ref":"evt_user"}}}}),
                extra: Value::Null,
            },
        );
        assert_eq!(
            state.ui.last_submission.as_ref().unwrap().prompt,
            "original provider request"
        );
        assert_eq!(state.input, "newer input");
        assert!(state.ui.execution_uncertain);
        assert_eq!(state.ui.panel, Panel::Context);
        let text = continuity_context_lines(&state)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("keep API") && text.contains("source: evt_user"));
        assert!(!text.contains("unknown · null"));
    }

    #[test]
    fn a_requirements_query_cannot_clear_input_when_another_ui_task_is_busy() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            input: "/requirements".into(),
            ..TuiState::default()
        };
        let (_sender, receiver) = mpsc::channel();
        let mut ui = Some(UiTask { receiver });
        let mut prompt = None;
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut prompt,
            &mut ui,
        )
        .unwrap();
        assert_eq!(state.input, "/requirements");
        assert!(ui.is_some() && prompt.is_none());
    }

    #[test]
    fn cancellation_reflects_a_lease_started_by_another_client() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            activity_status: "idle".into(),
            ..TuiState::default()
        };
        apply_action(
            &mut state,
            ActionResult {
                action: Action::Cancel,
                session_id: Some("task_1".into()),
                workspace_id: None,
                response: json!({"session":{"id":"task_1","status":"Canceled","active_provider_id":"codex","continuity":{"active_turn":{"id":"evt_other","uncertain":false}}}}),
                extra: Value::Null,
            },
        );
        assert!(state.ui.execution_uncertain);
        let (mut prompt, mut ui) = (None, None);
        run_command(&mut state, CommandId::New, &mut prompt, &mut ui).unwrap();
        assert_eq!(state.session_id.as_deref(), Some("task_1"));
    }

    #[test]
    fn a_submitted_context_is_distinct_from_an_unsent_preview() {
        let mut state = TuiState::default();
        state.ui.context = json!({"context":{"provider_id":"codex","estimated_tokens":10,"blocked":false},"preview_only":false,"outcome":"completed"});
        let lines = continuity_context_lines(&state)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(lines.contains("Last submitted context") && lines.contains("completed"));
        assert!(!lines.contains("not delivered") && !lines.contains("Preview only"));
    }

    #[test]
    fn local_command_completion_applies_nested_state_and_keeps_requirements_visible() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            input: "newer draft".into(),
            ..TuiState::default()
        };
        apply_continuity_status(
            &mut state,
            &json!({"continuity":{"active_turn":{"id":"evt_old","uncertain":true}}}),
        );
        let continuity = json!({"active_turn":null,"requirements":{"language":{"text":"中文回答","active":true,"revision":2}}});
        apply_prompt_completion(
            &mut state,
            PromptTaskCompletion {
                workspace_id: Some("ws_1".into()),
                workspace_display: None,
                session_id: "task_1".into(),
                active_provider: Some("codex".into()),
                route_reason: None,
                prompt_response: json!({"local_command":true,"turn_status":"completed","session_status":"Running","session":{"active_provider_id":"codex","continuity":continuity}}),
                events_response: json!({"events":[{"event_type":"session.execution.reconciled","payload":{"message":"Reviewed without replay"}}]}),
                routes_response: json!({"routes":[]}),
                diff_response: json!({"diff":{}}),
            },
        );
        assert!(!state.ui.execution_uncertain);
        assert!(state.ui.uncertain_turn_id.is_none());
        assert_eq!(
            state.ui.context["continuity"]["requirements"]["language"]["text"],
            "中文回答"
        );
        assert_eq!(state.input, "newer draft");
        let text = continuity_context_lines(&state)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("unknown · null"));
    }

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
