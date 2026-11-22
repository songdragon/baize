use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputRole {
    Answer,
    Commentary,
    Tool,
    Diagnostic,
    Status,
}

#[derive(Debug, Clone)]
pub(super) struct OutputItem {
    pub role: OutputRole,
    pub text: String,
    pub raw: Value,
    pub final_answer: bool,
}

#[derive(Debug, Clone)]
pub(super) struct OutputTurn {
    pub anchor: usize,
    pub provider: String,
    pub items: Vec<OutputItem>,
    pub indices: HashMap<String, usize>,
    pub expanded: bool,
    pub details: bool,
    pub selected: usize,
    pub outcome: Option<String>,
    pub started: std::time::Instant,
    pub timestamp: Option<i64>,
    pub elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ConversationOutput {
    pub turns: Vec<OutputTurn>,
    pub active: Option<usize>,
    pub selected: usize,
}

pub(super) fn reset_stream_output(state: &mut TuiState) {
    state.ui.output = ConversationOutput::default();
    state.ui.stderr_seen = false;
    state.streaming_assistant_open = false;
}

pub(super) fn begin_output_turn(state: &mut TuiState, provider: &str, timestamp: Option<i64>) {
    let anchor = state.transcript.len();
    state.push_message(format!("{provider} · CLI"));
    let output = &mut state.ui.output;
    output.active = Some(output.turns.len());
    output.selected = output.turns.len();
    output.turns.push(OutputTurn {
        anchor,
        provider: provider.into(),
        items: vec![],
        indices: HashMap::new(),
        expanded: false,
        details: false,
        selected: 0,
        outcome: None,
        started: std::time::Instant::now(),
        timestamp,
        elapsed_ms: None,
    });
    state.ui.stderr_seen = false;
    state.streaming_assistant_open = false;
}

pub(super) fn ensure_output_turn(state: &mut TuiState, event: &Value) -> usize {
    if state.ui.output.active.is_none() {
        begin_output_turn(
            state,
            event["provider_id"].as_str().unwrap_or("agent"),
            event_time(event),
        );
    }
    state.ui.output.active.unwrap_or(0)
}

pub(super) fn event_time(event: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(event["timestamp"].as_str()?)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

pub(super) fn finish_output_turn(state: &mut TuiState, outcome: &str, timestamp: Option<i64>) {
    let Some(index) = state.ui.output.active else {
        return;
    };
    let turn = &mut state.ui.output.turns[index];
    if turn.outcome.is_none() {
        turn.elapsed_ms = Some(match (turn.timestamp, timestamp) {
            (Some(start), Some(end)) => end.saturating_sub(start).max(0) as u64,
            _ => turn.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        });
    }
    turn.outcome = Some(outcome.into());
    state.streaming_assistant_open = false;
}

pub(super) fn process_key(state: &mut TuiState, key: KeyEvent) -> bool {
    if key.code == KeyCode::F(9) {
        if state.ui.output.turns.is_empty() {
            return true;
        }
        state.ui.output.selected = state.ui.output.turns.len() - 1;
        state.ui.focus = Focus::Conversation;
    } else if state.ui.focus != Focus::Conversation {
        return false;
    }
    let output = &mut state.ui.output;
    match key.code {
        KeyCode::Left => output.selected = output.selected.saturating_sub(1),
        KeyCode::Right => {
            output.selected = (output.selected + 1).min(output.turns.len().saturating_sub(1))
        }
        _ => {}
    }
    let Some(turn) = output.turns.get_mut(output.selected) else {
        return false;
    };
    let entries = turn
        .items
        .iter()
        .filter(|item| item.role != OutputRole::Answer && item.role != OutputRole::Status)
        .count();
    match key.code {
        KeyCode::F(9) | KeyCode::Char(' ') => turn.expanded = !turn.expanded,
        KeyCode::Enter => {
            turn.expanded = true;
            turn.details = !turn.details;
        }
        KeyCode::Up => {
            turn.selected = turn.selected.saturating_sub(1);
            turn.details = false;
        }
        KeyCode::Down => {
            turn.selected = (turn.selected + 1).min(entries.saturating_sub(1));
            turn.details = false;
        }
        KeyCode::Left | KeyCode::Right => {}
        _ => return false,
    }
    reveal_process(state);
    true
}

pub(super) fn reveal_process(state: &mut TuiState) {
    let Some(target) = state.ui.output.turns.get(state.ui.output.selected) else {
        return;
    };
    let mut row = 0;
    for (index, line) in state.transcript.iter().enumerate() {
        if index == target.anchor {
            break;
        }
        row += if let Some(turn) = state
            .ui
            .output
            .turns
            .iter()
            .find(|turn| turn.anchor == index)
        {
            output_turn_lines(turn, state.ui.wrap_width, false, &state.workspace).len()
        } else {
            line.split('\n')
                .map(|part| wrap_transcript_raw_line(part, state.ui.wrap_width).len())
                .sum()
        };
    }
    let lines = output_turn_lines(target, state.ui.wrap_width, true, &state.workspace);
    let selected = lines
        .iter()
        .position(|line| line.to_string().starts_with("  ›"))
        .unwrap_or(1);
    let top = row + selected.saturating_sub(state.ui.viewport_height / 2);
    let height = transcript_text_for_width(state, state.ui.wrap_width)
        .lines
        .len();
    state.scroll_offset = height
        .saturating_sub(state.ui.viewport_height)
        .saturating_sub(top);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn turn_lifecycle_uses_event_timestamps_and_resets_all_session_output() {
        let mut state = TuiState::default();
        let start = event_time(&json!({"timestamp":"2026-11-22T12:00:00Z"})).unwrap();
        begin_output_turn(&mut state, "codex", Some(start));
        finish_output_turn(&mut state, "completed", Some(start + 2500));
        assert_eq!(state.ui.output.turns[0].elapsed_ms, Some(2500));
        begin_output_turn(&mut state, "antigravity", None);
        assert_eq!(state.ui.output.turns.len(), 2);
        reset_stream_output(&mut state);
        assert!(state.ui.output.turns.is_empty());
        assert_eq!(event_time(&json!({"timestamp":"invalid"})), None);
    }

    #[test]
    fn process_keys_do_not_steal_composer_spaces_or_panic_without_events() {
        let mut state = TuiState::default();
        assert!(!process_key(
            &mut state,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
        ));
        assert!(process_key(
            &mut state,
            KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE)
        ));
        begin_output_turn(&mut state, "codex", None);
        process_key(&mut state, KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
        assert!(state.ui.output.turns[0].expanded);
        process_key(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.ui.output.turns[0].selected, 0);
    }

    #[test]
    fn opening_process_reveals_it_above_a_long_final_answer() {
        let mut state = TuiState::default();
        apply_prompt_stream_event(
            &mut state,
            &json!({"id":"1","event_type":"session.agent.tool_call","payload":{"text":"cargo test [completed]"}}),
        );
        apply_prompt_stream_event(
            &mut state,
            &json!({"id":"2","event_type":"session.agent.output","payload":{"text":"answer line\n".repeat(100)}}),
        );
        finish_output_turn(&mut state, "completed", None);
        process_key(&mut state, KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
        assert!(state.scroll_offset > 0);
        let lines = transcript_text_for_width(&state, state.ui.wrap_width).lines;
        let top = lines
            .len()
            .saturating_sub(state.ui.viewport_height)
            .saturating_sub(state.scroll_offset);
        let visible = lines
            .iter()
            .skip(top)
            .take(state.ui.viewport_height)
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(visible.contains("cargo test"));
    }
}
