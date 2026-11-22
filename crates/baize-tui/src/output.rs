use super::*;

#[derive(Debug, Clone)]
pub(crate) struct StreamItem {
    start: usize,
    len: usize,
    text: String,
}

pub(super) fn reset_stream_output(state: &mut TuiState) {
    state.ui.stream_items.clear();
    state.ui.stderr_seen = false;
    state.streaming_assistant_open = false;
}

pub(super) fn render_agent_event(state: &mut TuiState, event: &Value) {
    let payload = &event["payload"];
    let event_type = event["event_type"].as_str().unwrap_or("");
    match event_type {
        "session.agent.output" if payload["stream"].is_object() => {
            append_stream_item(state, event, "  ", true)
        }
        "session.agent.output" if !append_stream_assistant_event(state, payload) => {
            append_stream_status_event(state, payload)
        }
        "session.agent.output" => {}
        "session.agent.diagnostic" => {
            let stderr = payload["stream"]["channel"] == "stderr";
            state.ui.stderr_seen |= stderr;
            append_stream_item(
                state,
                event,
                if stderr {
                    "stderr │ "
                } else {
                    "diagnostic │ "
                },
                false,
            );
        }
        "session.agent.tool_call" => append_stream_item(state, event, "tool │ ", false),
        "session.agent.status" => {
            if let Some(text) = payload["text"].as_str() {
                state.push_message(format!("status │ {}", safe_text(text)));
            } else {
                append_stream_status_event(state, payload);
            }
        }
        "session.agent.failed" => {
            state.activity_status = "failed".into();
            state.push_message(format!(
                "error │ {}",
                status_event_line(event_type, payload)
            ));
        }
        _ => {}
    }
}

fn safe_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}

fn append_stream_item(state: &mut TuiState, event: &Value, prefix: &str, assistant: bool) {
    let payload = &event["payload"];
    let Some(text) = payload["text"].as_str() else {
        return;
    };
    let text = text.to_string();
    if text.is_empty() {
        return;
    }
    let id = payload["stream"]["item_id"]
        .as_str()
        .or_else(|| event["id"].as_str())
        .unwrap_or("unknown");
    let key = format!(
        "{}:{}:{}:{id}",
        event["provider_id"].as_str().unwrap_or("agent"),
        event["event_type"].as_str().unwrap_or("event"),
        payload["stream"]["channel"].as_str().unwrap_or("stdout")
    );
    let previous = state.ui.stream_items.get(&key).cloned();
    let merged = if payload["stream"]["mode"] == "delta" {
        format!(
            "{}{text}",
            previous
                .as_ref()
                .map(|item| item.text.as_str())
                .unwrap_or("")
        )
    } else {
        text
    };
    let display = safe_text(&merged);
    let lines = display
        .split('\n')
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>();
    let len = lines.len();
    let start = if let Some(previous) = previous {
        let start = previous.start;
        let end = start + previous.len;
        let old_height: usize = state.transcript[start..end]
            .iter()
            .map(|line| wrap_transcript_raw_line(line, state.ui.wrap_width).len())
            .sum();
        let new_height: usize = lines
            .iter()
            .map(|line| wrap_transcript_raw_line(line, state.ui.wrap_width).len())
            .sum();
        state.transcript.splice(start..end, lines);
        for (other_key, item) in &mut state.ui.stream_items {
            if other_key != &key && item.start >= end {
                item.start = item.start + len - previous.len;
            }
        }
        if state.scroll_offset > 0 {
            state.scroll_offset = state
                .scroll_offset
                .saturating_add(new_height)
                .saturating_sub(old_height);
        }
        start
    } else {
        if assistant && !state.streaming_assistant_open {
            push_transcript_section(state, "assistant:");
            state.streaming_assistant_open = true;
        }
        let start = state.transcript.len();
        for line in lines {
            state.push_message(line);
        }
        start
    };
    state.ui.stream_items.insert(
        key,
        StreamItem {
            start,
            len,
            text: merged,
        },
    );
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
}
