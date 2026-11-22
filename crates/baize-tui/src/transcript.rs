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
        .filter(|stderr| !state.ui.stderr_seen && should_show_prompt_stderr(response, stderr))
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
    if let Some(events) = response["events"].as_array() {
        for event in events {
            render_agent_event(state, event);
        }
    }
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
