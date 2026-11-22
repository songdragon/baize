use super::*;

pub(super) fn safe_output_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}

pub(super) fn render_agent_event(state: &mut TuiState, event: &Value) {
    let old_height = if state.scroll_offset > 0 {
        transcript_text_for_width(state, state.ui.wrap_width)
            .lines
            .len()
    } else {
        0
    };
    apply_agent_event(state, event);
    if state.scroll_offset > 0 {
        let new_height = transcript_text_for_width(state, state.ui.wrap_width)
            .lines
            .len();
        state.scroll_offset = state
            .scroll_offset
            .saturating_add(new_height)
            .saturating_sub(old_height);
    }
}

fn apply_agent_event(state: &mut TuiState, event: &Value) {
    let kind = event["event_type"].as_str().unwrap_or("");
    let payload = &event["payload"];
    match kind {
        "session.agent.started" => {
            if state
                .ui
                .output
                .active
                .is_none_or(|index| state.ui.output.turns[index].outcome.is_some())
            {
                if let Some(prompt) = payload["prompt"].as_str() {
                    push_transcript_gap(state);
                    state.push_message(format!("> {prompt}"));
                }
                begin_output_turn(
                    state,
                    event["provider_id"].as_str().unwrap_or("agent"),
                    event_time(event),
                );
            }
            let index = ensure_output_turn(state, event);
            let turn = &mut state.ui.output.turns[index];
            if let Some(provider) = event["provider_id"].as_str() {
                turn.provider = provider.into();
            }
            if let Some(timestamp) = event_time(event) {
                turn.timestamp = Some(timestamp);
            }
            return;
        }
        "session.agent.completed"
        | "session.agent.failed"
        | "session.agent.ignored"
        | "session.canceled" => {
            if kind == "session.agent.failed" {
                state.activity_status = "failed".into();
                if let Some(text) = payload["error"]
                    .as_str()
                    .or_else(|| payload["provider_error"].as_str())
                    .or_else(|| payload["stderr"].as_str())
                    .filter(|s| !s.is_empty())
                {
                    record_output_item(state, event, OutputRole::Diagnostic, text, false);
                }
            }
            let outcome = match kind {
                "session.agent.completed" => "completed",
                "session.agent.failed" => "failed",
                _ => "canceled",
            };
            finish_output_turn(state, outcome, event_time(event));
            return;
        }
        _ => {}
    }
    let role = match kind {
        "session.agent.output" => OutputRole::Answer,
        "session.agent.tool_call" => OutputRole::Tool,
        "session.agent.diagnostic" => {
            state.ui.stderr_seen |= payload["stream"]["channel"] == "stderr";
            OutputRole::Diagnostic
        }
        "session.agent.status" => OutputRole::Status,
        _ => return,
    };
    let raw = &payload["raw"];
    // The native result is authoritative even when the adapter deduplicated its
    // text into a status event. Reading raw also supports existing event logs.
    let final_text = if raw["event"] == "result" && raw["result"]["status"] == "SUCCESS" {
        raw["result"]["response"].as_str()
    } else if raw["type"] == "result" && raw["is_error"] != true {
        raw["result"].as_str()
    } else {
        None
    };
    if let Some(text) = final_text.filter(|text| !text.is_empty()) {
        let index = ensure_output_turn(state, event);
        let turn = &mut state.ui.output.turns[index];
        let candidate = turn
            .items
            .iter()
            .rposition(|item| item.role == OutputRole::Answer);
        for item in &mut turn.items {
            if item.role == OutputRole::Answer {
                item.role = OutputRole::Commentary;
            }
        }
        if let Some(candidate) = candidate {
            let item = &mut turn.items[candidate];
            item.text = text.into();
            item.raw = raw.clone();
            item.role = OutputRole::Answer;
            item.final_answer = true;
        } else {
            record_output_item(state, event, OutputRole::Answer, text, true);
        }
        return;
    }
    let phase = raw
        .pointer("/item/phase")
        .or_else(|| raw.get("phase"))
        .and_then(Value::as_str);
    let reasoning = raw
        .pointer("/item/type")
        .or_else(|| raw.pointer("/part/type"))
        .and_then(Value::as_str)
        == Some("reasoning");
    let role = if role == OutputRole::Answer && (phase == Some("commentary") || reasoning) {
        OutputRole::Commentary
    } else {
        role
    };
    let role = if role == OutputRole::Answer && !payload["text"].is_string() {
        OutputRole::Status
    } else {
        role
    };
    let text = payload["text"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| raw_stream_event_detail(payload));
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        record_output_item(state, event, role, &text, phase == Some("final_answer"));
    }
}

fn record_output_item(
    state: &mut TuiState,
    event: &Value,
    role: OutputRole,
    text: &str,
    final_answer: bool,
) {
    let index = ensure_output_turn(state, event);
    let turn = &mut state.ui.output.turns[index];
    let payload = &event["payload"];
    let id = payload["stream"]["item_id"]
        .as_str()
        .or_else(|| event["id"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("item_{}", turn.items.len()));
    let key = format!(
        "{}:{}:{id}",
        event["event_type"].as_str().unwrap_or("output"),
        payload["stream"]["channel"].as_str().unwrap_or("stdout")
    );
    // Untagged text before the next tool call is progress commentary. A native
    // explicit final-answer phase is never demoted by subsequent tool events.
    if role == OutputRole::Tool && !turn.indices.contains_key(&key) {
        for item in &mut turn.items {
            if item.role == OutputRole::Answer && !item.final_answer {
                item.role = OutputRole::Commentary;
            }
        }
    }
    if role == OutputRole::Answer
        && id == "stdout"
        && payload["stream"]["mode"] == "delta"
        && turn
            .indices
            .get(&key)
            .is_some_and(|index| turn.items[*index].role == OutputRole::Commentary)
    {
        // Plain stdout has one channel identity across tools; start a fresh
        // answer segment instead of appending it to now-folded commentary.
        turn.indices.remove(&key);
    }
    if let Some(index) = turn.indices.get(&key).copied() {
        let item = &mut turn.items[index];
        if payload["stream"]["mode"] == "delta" {
            item.text.push_str(text);
        } else {
            item.text = text.into();
        }
        item.raw = payload["raw"].clone();
        if final_answer {
            item.role = OutputRole::Answer;
            item.final_answer = true;
        }
    } else {
        turn.indices.insert(key, turn.items.len());
        turn.items.push(OutputItem {
            role,
            text: text.into(),
            raw: payload["raw"].clone(),
            final_answer,
        });
    }
    state.streaming_assistant_open |= role == OutputRole::Answer;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    fn event(id: &str, kind: &str, item: &str, mode: &str, text: &str) -> Value {
        json!({"id":id,"session_id":"task_1","provider_id":"codex","event_type":kind,"payload":{"text":text,"stream":{"item_id":item,"channel":if kind=="session.agent.diagnostic"{"stderr"}else{"stdout"},"mode":mode}}})
    }
    #[test]
    fn renders_arrived_text_immediately_then_merges_deltas_and_snapshot() {
        let mut state = TuiState::default();
        apply_prompt_stream_event(
            &mut state,
            &event(
                "1",
                "session.agent.output",
                "m",
                "delta",
                "你好 first chunk without waiting for a full message",
            ),
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("without waiting"));
        apply_prompt_stream_event(
            &mut state,
            &event("2", "session.agent.output", "m", "delta", "\nnext"),
        );
        apply_prompt_stream_event(
            &mut state,
            &event(
                "3",
                "session.agent.output",
                "m",
                "snapshot",
                "你好 first chunk without waiting for a full message\nnext",
            ),
        );
        assert_eq!(state.transcript_text().matches("first chunk").count(), 1);
        assert!(state.transcript_text().contains("next"));
    }
    #[test]
    fn interleaved_sources_and_tool_updates_match_restored_history() {
        let events = vec![
            event("1", "session.agent.output", "m", "delta", "start"),
            event(
                "2",
                "session.agent.diagnostic",
                "stderr",
                "delta",
                "auth warning",
            ),
            event(
                "3",
                "session.agent.tool_call",
                "t",
                "snapshot",
                "cargo test [running]",
            ),
            event("4", "session.agent.output", "m", "delta", "\nend"),
            event(
                "5",
                "session.agent.tool_call",
                "t",
                "snapshot",
                "cargo test [completed]\nall passed",
            ),
            event("6", "session.agent.output", "m", "snapshot", "start\nend"),
        ];
        let mut live = TuiState {
            transcript: vec![],
            ..TuiState::default()
        };
        for event in &events {
            apply_prompt_stream_event(&mut live, event);
        }
        let mut restored = TuiState::default();
        replay_session(&mut restored, &json!({"events":events})).unwrap();
        assert_eq!(live.transcript, restored.transcript);
        let text = live.transcript_text();
        assert_eq!(text.matches("auth warning").count(), 1);
        assert_eq!(text.matches("cargo test").count(), 1);
        assert!(
            text.contains("stderr │ auth warning")
                && text.contains("tool │ cargo test [completed]")
        );
        assert!(live.ui.stderr_seen);
    }
    #[test]
    fn preserves_full_tool_output_and_never_emits_terminal_control_sequences() {
        let mut state = TuiState::default();
        let text = format!(
            "command [completed]\n{}\nTAIL\u{1b}]52;c;data\u{7}",
            "output ".repeat(200)
        );
        apply_prompt_stream_event(
            &mut state,
            &event("1", "session.agent.tool_call", "t", "snapshot", &text),
        );
        assert!(state.transcript_text().contains("TAIL"));
        assert!(!state.transcript_text().contains('\u{1b}'));
        assert!(!state.transcript_text().contains('\u{7}'));
        let before = state.transcript.clone();
        apply_prompt_stream_event(
            &mut state,
            &event("1", "session.agent.tool_call", "t", "snapshot", &text),
        );
        assert_eq!(before, state.transcript);
    }

    #[test]
    fn native_final_result_replaces_candidate_without_repeating_commentary() {
        let mut state = TuiState::default();
        for value in [
            event(
                "1",
                "session.agent.output",
                "intro",
                "delta",
                "I will read files",
            ),
            event(
                "2",
                "session.agent.tool_call",
                "read",
                "snapshot",
                "read_file [DONE]",
            ),
            event(
                "3",
                "session.agent.output",
                "answer",
                "delta",
                "Final partial",
            ),
            json!({"id":"4","event_type":"session.agent.status","payload":{"text":"result [SUCCESS]","raw":{"event":"result","result":{"status":"SUCCESS","response":"Final complete"}}}}),
        ] {
            apply_prompt_stream_event(&mut state, &value);
        }
        let items = &state.ui.output.turns[0].items;
        assert_eq!(
            items
                .iter()
                .filter(|item| item.role == OutputRole::Answer)
                .count(),
            1
        );
        assert_eq!(items.last().unwrap().text, "Final complete");
        assert_eq!(items[0].role, OutputRole::Commentary);
        let text = transcript_text_for_width(&state, 100)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.matches("Final complete").count(), 1);
        assert!(!text.contains("I will read files"));
    }

    #[test]
    fn phase_tagged_final_answer_survives_later_tool_and_unknown_progress_is_not_prose() {
        let mut state = TuiState::default();
        let mut answer = event("1", "session.agent.output", "m", "delta", "Explicit final");
        answer["payload"]["raw"] = json!({"item":{"phase":"final_answer"}});
        for value in [
            answer,
            event(
                "2",
                "session.agent.tool_call",
                "t",
                "snapshot",
                "cargo test [completed]",
            ),
            json!({"id":"3","event_type":"session.agent.output","payload":{"raw":{"type":"step_start"}}}),
        ] {
            apply_prompt_stream_event(&mut state, &value);
        }
        let turn = &state.ui.output.turns[0];
        assert_eq!(turn.items[0].role, OutputRole::Answer);
        assert_eq!(turn.items[2].role, OutputRole::Status);
    }

    #[test]
    fn canonical_result_resize_preserves_history_scroll_and_duplicate_events_are_ignored() {
        let mut state = TuiState::default();
        apply_prompt_stream_event(
            &mut state,
            &event("1", "session.agent.output", "m", "delta", "short"),
        );
        state.scroll_offset = 4;
        let final_event = json!({"id":"2","event_type":"session.agent.status","payload":{"raw":{"event":"result","result":{"status":"SUCCESS","response":"long\nanswer\ntail"}}}});
        apply_prompt_stream_event(&mut state, &final_event);
        assert_eq!(state.scroll_offset, 6);
        let before = state.ui.output.turns[0].items.len();
        apply_prompt_stream_event(&mut state, &final_event);
        assert_eq!(state.ui.output.turns[0].items.len(), before);
    }

    #[test]
    fn plain_stdout_after_a_tool_remains_visible_as_a_new_answer_segment() {
        let mut state = TuiState::default();
        for value in [
            event(
                "1",
                "session.agent.output",
                "stdout",
                "delta",
                "Reading files",
            ),
            event(
                "2",
                "session.agent.tool_call",
                "t",
                "snapshot",
                "read [DONE]",
            ),
            event(
                "3",
                "session.agent.output",
                "stdout",
                "delta",
                "Final answer",
            ),
            event("4", "session.agent.output", "stdout", "delta", " tail"),
        ] {
            apply_prompt_stream_event(&mut state, &value);
        }
        let text = transcript_text_for_width(&state, 100)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Final answer tail"));
        assert!(!text.contains("Reading files"));
    }
}
