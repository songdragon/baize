use super::*;

pub(super) fn append_prompt_response(state: &mut TuiState, response: &Value) {
    let turn_status = response
        .get("turn_status")
        .and_then(Value::as_str)
        .or_else(|| response.get("status").and_then(Value::as_str))
        .unwrap_or("unknown");
    let session_status = response
        .get("session_status")
        .and_then(Value::as_str)
        .or_else(|| response.get("status").and_then(Value::as_str))
        .unwrap_or("unknown");
    let legacy_status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let provider = response
        .get("provider_id")
        .and_then(provider_id)
        .unwrap_or("unknown");
    let mut response_block_started = false;
    if state.detail_mode {
        push_transcript_gap(state);
        response_block_started = true;
        if response.get("turn_status").is_some() || response.get("session_status").is_some() {
            state.push_message(format!(
                "turn result: {turn_status} via {provider} (session {session_status})"
            ));
        } else {
            state.push_message(format!("prompt result: {legacy_status} via {provider}"));
        }
        if let Some(status) = context_status_line(response) {
            state.push_message(status);
        }
    } else if let Some(status) =
        compact_prompt_status_line(response, turn_status, legacy_status, provider)
    {
        push_transcript_gap(state);
        response_block_started = true;
        state.push_message(status);
    }

    if let Some(error) = response.get("error").and_then(Value::as_str) {
        if !response_block_started {
            push_transcript_gap(state);
            response_block_started = true;
        }
        state.push_message(format!("  error: {}", transcript_detail(error)));
    }
    if let Some(stderr) = response
        .get("stderr")
        .and_then(Value::as_str)
        .filter(|stderr| should_show_prompt_stderr(response, stderr))
    {
        if !stderr.trim().is_empty() {
            if !response_block_started {
                push_transcript_gap(state);
                response_block_started = true;
            }
            state.push_message(format!("  stderr: {}", transcript_detail(stderr)));
        }
    }
    if let Some(hint) = provider_error_hint(response) {
        if !response_block_started {
            push_transcript_gap(state);
        }
        state.push_message(hint);
    }
}

pub(super) fn compact_prompt_status_line(
    response: &Value,
    turn_status: &str,
    legacy_status: &str,
    provider: &str,
) -> Option<String> {
    let status =
        if response.get("turn_status").is_some() || response.get("session_status").is_some() {
            turn_status
        } else {
            legacy_status
        };
    match status {
        "completed" | "running" | "ok" | "success" | "unknown" => None,
        status => Some(format!(
            "{status} · {provider}{}",
            context_summary_suffix(response)
        )),
    }
}

pub(super) fn context_summary_suffix(response: &Value) -> String {
    context_summary(response)
        .map(|summary| format!(" · {summary}"))
        .unwrap_or_default()
}

pub(super) fn context_summary(response: &Value) -> Option<String> {
    let context = response.get("context")?;
    let strategy = context.get("strategy").and_then(Value::as_str)?;
    let estimated_tokens = context
        .get("estimated_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let strategy_label = match strategy {
        "RawNativeResume" => "native resume",
        "CrossProviderDelta" => "cross-provider delta",
        "FullContextPack" => "full context pack",
        _ => strategy,
    };
    Some(format!("{strategy_label} · {estimated_tokens} tokens"))
}

pub(super) fn context_status_line(response: &Value) -> Option<String> {
    let context = response.get("context")?;
    let strategy = context.get("strategy").and_then(Value::as_str)?;
    let estimated_tokens = context
        .get("estimated_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let truncated = context
        .get("truncated_sections")
        .and_then(Value::as_array)
        .map(|sections| {
            sections
                .iter()
                .filter_map(Value::as_str)
                .take(3)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let strategy_label = match strategy {
        "RawNativeResume" => "native resume",
        "CrossProviderDelta" => "cross-provider delta",
        "FullContextPack" => "full context pack",
        _ => strategy,
    };
    let mut line = format!("context: {strategy_label} {estimated_tokens} est tokens");
    if !truncated.is_empty() {
        line.push_str(&format!(" / truncated {}", truncated.join(", ")));
    }
    Some(line)
}

pub(super) fn prompt_activity_after_response(response: &Value) -> &'static str {
    match response
        .get("turn_status")
        .and_then(Value::as_str)
        .or_else(|| response.get("status").and_then(Value::as_str))
    {
        Some("failed") => "failed",
        _ => "idle",
    }
}

pub(super) fn should_show_prompt_stderr(response: &Value, stderr: &str) -> bool {
    !stderr.trim().is_empty()
        && matches!(
            response
                .get("turn_status")
                .and_then(Value::as_str)
                .or_else(|| response.get("status").and_then(Value::as_str)),
            Some("failed" | "canceled")
        )
}

pub(super) fn append_recent_events(state: &mut TuiState, response: &Value) {
    let Some(events) = response.get("events").and_then(Value::as_array) else {
        return;
    };

    let recent_events = events
        .iter()
        .rev()
        .take(TRANSCRIPT_RECENT_EVENT_LIMIT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let mut tool_lines = Vec::new();
    let mut assistant_lines = Vec::new();
    let mut status_lines = Vec::new();
    for event in recent_events {
        let event_type = event
            .get("event_type")
            .and_then(Value::as_str)
            .unwrap_or("event");
        let payload = event.get("payload").unwrap_or(&Value::Null);

        match event_type {
            "session.agent.output" => {
                assistant_lines.extend(assistant_event_lines(payload));
            }
            "session.agent.tool_call" => {
                if let Some(detail) = event_detail(payload) {
                    if !detail.is_empty() {
                        tool_lines.push(detail);
                    } else {
                        tool_lines.push("tool call".to_string());
                    }
                } else {
                    tool_lines.push("tool call".to_string());
                }
            }
            _ => status_lines.push(status_event_line(event_type, payload)),
        }
    }

    if !assistant_lines.is_empty() {
        push_transcript_section(state, "assistant:");
        for line in &assistant_lines {
            state.push_message(format!("  {line}"));
        }
    }
    if !tool_lines.is_empty() {
        push_transcript_section(state, "command/tool events:");
        for line in tool_lines.iter().take(TRANSCRIPT_TOOL_LINE_LIMIT) {
            state.push_message(format!("  {line}"));
        }
        append_more_line(state, tool_lines.len(), TRANSCRIPT_TOOL_LINE_LIMIT);
    }
    if !status_lines.is_empty() {
        push_transcript_section(state, "status:");
        for line in status_lines.iter().take(TRANSCRIPT_STATUS_LINE_LIMIT) {
            state.push_message(format!("  {line}"));
        }
        append_more_line(state, status_lines.len(), TRANSCRIPT_STATUS_LINE_LIMIT);
    }
}

pub(super) fn append_assistant_events(state: &mut TuiState, response: &Value) {
    let Some(events) = response.get("events").and_then(Value::as_array) else {
        return;
    };
    let assistant_lines = events
        .iter()
        .filter(|event| {
            event
                .get("event_type")
                .and_then(Value::as_str)
                .is_some_and(|event_type| event_type == "session.agent.output")
        })
        .filter_map(|event| event.get("payload"))
        .flat_map(assistant_event_lines)
        .collect::<Vec<_>>();
    if assistant_lines.is_empty() {
        return;
    }

    push_transcript_section(state, "assistant:");
    for line in assistant_lines {
        state.push_message(format!("  {line}"));
    }
}

pub(super) fn assistant_event_lines(payload: &Value) -> Vec<String> {
    payload
        .get("text")
        .and_then(Value::as_str)
        .map(split_display_lines)
        .unwrap_or_default()
}

pub(super) fn split_display_lines(text: &str) -> Vec<String> {
    let lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.is_empty() {
        vec![text.to_string()]
    } else {
        lines
    }
}

pub(super) fn should_progressively_render_assistant(state: &TuiState, lines: &[String]) -> bool {
    has_pending_assistant_stream(state)
        || lines.iter().map(|line| line.chars().count()).sum::<usize>()
            >= ASSISTANT_STREAM_MIN_CHARS
}

pub(super) fn advance_pending_assistant_stream(state: &mut TuiState) {
    advance_pending_assistant_stream_by(state, ASSISTANT_STREAM_CHARS_PER_TICK);
}

pub(super) fn flush_pending_assistant_stream(state: &mut TuiState) {
    while has_pending_assistant_stream(state) {
        advance_pending_assistant_stream_by(state, usize::MAX / 4);
    }
}

pub(super) fn clear_pending_assistant_stream(state: &mut TuiState) {
    state.pending_assistant_lines.clear();
    state.pending_assistant_current_line = None;
    state.pending_assistant_line_index = None;
}

pub(super) fn has_pending_assistant_stream(state: &TuiState) -> bool {
    state.pending_assistant_current_line.is_some() || !state.pending_assistant_lines.is_empty()
}

pub(super) fn advance_pending_assistant_stream_by(state: &mut TuiState, mut budget: usize) {
    while budget > 0 {
        if state.pending_assistant_current_line.is_none() {
            let Some(next_line) = state.pending_assistant_lines.pop_front() else {
                state.pending_assistant_line_index = None;
                return;
            };
            state.push_message("  ".to_string());
            state.pending_assistant_line_index = state.transcript.len().checked_sub(1);
            state.pending_assistant_current_line = Some(next_line);
        }

        let Some(line_index) = state.pending_assistant_line_index else {
            clear_pending_assistant_stream(state);
            return;
        };
        if line_index >= state.transcript.len() {
            clear_pending_assistant_stream(state);
            return;
        }

        let current = state
            .pending_assistant_current_line
            .take()
            .unwrap_or_default();
        if current.is_empty() {
            state.pending_assistant_line_index = None;
            continue;
        }

        let chunk = current.chars().take(budget).collect::<String>();
        let consumed = chunk.chars().count();
        state.transcript[line_index].push_str(&chunk);
        budget = budget.saturating_sub(consumed);

        let remaining = current.chars().skip(consumed).collect::<String>();
        if remaining.is_empty() {
            state.pending_assistant_line_index = None;
        } else {
            state.pending_assistant_current_line = Some(remaining);
            break;
        }
    }
}

pub(super) fn event_detail(payload: &Value) -> Option<String> {
    payload
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| payload.get("error").and_then(Value::as_str))
        .or_else(|| payload.get("stderr").and_then(Value::as_str))
        .map(transcript_detail)
}

pub(super) fn status_event_line(event_type: &str, payload: &Value) -> String {
    let label = match event_type {
        "session.created" => "session created",
        "session.route.decided" => "route decided",
        "session.agent.started" => "agent started",
        "session.agent.completed" => "agent completed",
        "session.agent.failed" => "agent failed",
        "session.status.changed" => "status changed",
        other => other,
    };
    if event_type == "session.agent.completed" {
        return label.to_string();
    }
    if let Some(detail) = status_event_detail(payload) {
        format!("{label}: {detail}")
    } else {
        label.to_string()
    }
}

pub(super) fn status_event_detail(payload: &Value) -> Option<String> {
    payload
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| payload.get("stderr").and_then(Value::as_str))
        .map(transcript_detail)
}

pub(super) fn append_more_line(state: &mut TuiState, total: usize, shown: usize) {
    if let Some(hidden) = total.checked_sub(shown).filter(|hidden| *hidden > 0) {
        state.push_message(format!("  (+{hidden} more)"));
    }
}

pub(super) fn push_transcript_section(state: &mut TuiState, label: &str) {
    push_transcript_gap(state);
    state.push_message(format!("╭─ {} ─────────────────────────────", label));
}

pub(super) fn push_transcript_gap(state: &mut TuiState) {
    if state
        .transcript
        .last()
        .is_some_and(|line| !line.trim().is_empty())
    {
        state.push_message(String::new());
    }
}

pub(super) fn append_test_result_section(state: &mut TuiState, response: &Value) {
    let Some(events) = response.get("events").and_then(Value::as_array) else {
        return;
    };
    let test_lines = events
        .iter()
        .filter(|event| {
            event
                .get("event_type")
                .and_then(Value::as_str)
                .is_some_and(|event_type| event_type != "session.agent.output")
        })
        .filter_map(|event| event.get("payload"))
        .filter_map(|payload| {
            payload
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| payload.get("stderr").and_then(Value::as_str))
        })
        .map(one_line)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            contains_any(
                &lower,
                &[
                    "test result",
                    "test failed",
                    "test passed",
                    "tests passed",
                    "test result:",
                    "cargo test",
                    "pytest",
                ],
            )
        })
        .take(3)
        .collect::<Vec<_>>();
    if test_lines.is_empty() {
        return;
    }

    push_transcript_section(state, "test results:");
    for line in test_lines {
        state.push_message(format!("  {line}"));
    }
}

pub(super) fn append_recent_events_with_sections(state: &mut TuiState, response: &Value) {
    append_recent_events(state, response);
    append_test_result_section(state, response);
}
