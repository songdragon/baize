use super::*;

pub(super) fn terminal_error(raw: &Value) -> Option<String> {
    if raw["event"] != "result" {
        return None;
    }
    let result = &raw["result"];
    if result["denied_actions"]
        .as_array()
        .is_some_and(|actions| !actions.is_empty())
    {
        return Some(format!(
            "Provider permission denied in headless mode: {}",
            result["denied_actions"]
        ));
    }
    if result["status"]
        .as_str()
        .is_some_and(|status| status.eq_ignore_ascii_case("success"))
    {
        return None;
    }
    Some(
        result["error"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Antigravity result [{}]", result["status"])),
    )
}

// Antigravity's documented NDJSON uses `event`, not Codex/OpenCode's `type`.
// Source: https://antigravity.google/docs/cli/headless.md
pub(super) fn parse_event(raw: Value) -> AgentExecutionEvent {
    let (kind, text, mode, item_id) = match raw["event"].as_str().unwrap_or("") {
        "step_update" => {
            let step = &raw["step_update"];
            let step_type = step["step_type"].as_str().unwrap_or("step");
            let state = step["state"].as_str().unwrap_or("update");
            let id = step["step_index"]
                .as_u64()
                .map(|index| format!("agy:{step_type}:{index}"));
            match step_type {
                "agent_response" if step["text_delta"].is_string() => (
                    AgentExecutionEventKind::Output,
                    step["text_delta"].as_str().map(str::to_owned),
                    AgentTextMode::Delta,
                    id,
                ),
                "tool" => {
                    let info = &step["tool_info"];
                    let name = step["tool_name"]
                        .as_str()
                        .or_else(|| info["name"].as_str())
                        .unwrap_or("tool");
                    let state = if info.get("error").is_some_and(|error| !error.is_null()) {
                        "failed"
                    } else {
                        state
                    };
                    let mut text = format!("{name} [{state}]");
                    if let Some(parameters) = info.get("parameters") {
                        text.push_str(&format!("\nparameters: {parameters}"));
                    }
                    if let Some(output) = info["output"].as_str() {
                        text.push('\n');
                        text.push_str(output);
                    }
                    if let Some(error) = info.get("error").filter(|error| !error.is_null()) {
                        text.push_str(&format!("\nerror: {error}"));
                    }
                    (
                        AgentExecutionEventKind::ToolCall,
                        Some(text),
                        AgentTextMode::Snapshot,
                        id,
                    )
                }
                _ => (
                    AgentExecutionEventKind::Raw,
                    Some(format!("{step_type} [{state}]")),
                    AgentTextMode::Message,
                    id,
                ),
            }
        }
        "result" => {
            let result = &raw["result"];
            let status = result["status"].as_str().unwrap_or("unknown");
            if status.eq_ignore_ascii_case("success") && terminal_error(&raw).is_none() {
                (
                    AgentExecutionEventKind::Output,
                    result["response"].as_str().map(str::to_owned),
                    AgentTextMode::Snapshot,
                    None,
                )
            } else {
                let error = terminal_error(&raw).unwrap_or_else(|| {
                    result
                        .get("error")
                        .map(|error| {
                            error
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| error.to_string())
                        })
                        .unwrap_or_else(|| format!("result [{status}]"))
                });
                (
                    AgentExecutionEventKind::Diagnostic,
                    Some(error),
                    AgentTextMode::Message,
                    None,
                )
            }
        }
        "init" => (
            AgentExecutionEventKind::Raw,
            Some("Antigravity stream connected".into()),
            AgentTextMode::Message,
            None,
        ),
        other => (
            AgentExecutionEventKind::Raw,
            Some(other.into()),
            AgentTextMode::Message,
            None,
        ),
    };
    AgentExecutionEvent {
        kind,
        text,
        raw: Some(raw),
        stream: Some(AgentStreamMetadata {
            channel: AgentOutputChannel::Stdout,
            mode,
            item_id,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headless_denial_overrides_terminal_success_and_remains_a_diagnostic() {
        let raw = serde_json::json!({"event":"result","result":{"status":"SUCCESS","response":"","denied_actions":[{"action":"command","display_name":"Bash"}]}});
        assert!(terminal_error(&raw).unwrap().contains("permission denied"));
        let event = parse_event(raw);
        assert!(matches!(event.kind, AgentExecutionEventKind::Diagnostic));
        assert!(event.text.unwrap().contains("command"));
        assert!(terminal_error(&serde_json::json!({"event":"result","result":{"status":"SUCCESS","response":"done","denied_actions":[]}})).is_none());
        assert!(terminal_error(
            &serde_json::json!({"event":"result","result":{"status":"ERROR","error":"failed"}})
        )
        .is_some());
    }
    #[test]
    fn parses_documented_step_updates_tools_and_terminal_result() {
        let delta=streaming::parse_stream_json_line(r#"{"event":"step_update","step_update":{"conversation_id":"c","step_index":3,"state":"ACTIVE","step_type":"agent_response","text_delta":"hello "}}"#).unwrap();
        assert_eq!(delta.text.as_deref(), Some("hello "));
        assert_eq!(
            delta.stream.unwrap().item_id.as_deref(),
            Some("agy:agent_response:3")
        );
        let tool=streaming::parse_stream_json_line(r#"{"event":"step_update","step_update":{"step_index":4,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hello"},"output":"hello\r\n"}}}"#).unwrap();
        assert!(matches!(tool.kind, AgentExecutionEventKind::ToolCall));
        let text = tool.text.unwrap();
        assert!(
            text.contains("run_command [DONE]")
                && text.contains("echo hello")
                && text.contains("hello\r\n")
        );
        let result=streaming::parse_stream_json_line(r#"{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","response":"hello world"}}"#).unwrap();
        assert_eq!(result.text.as_deref(), Some("hello world"));
        assert_eq!(result.stream.unwrap().mode, AgentTextMode::Snapshot);
    }
    #[test]
    fn failure_is_diagnostic_and_tool_error_is_visible() {
        let result = parse_event(
            serde_json::json!({"event":"result","result":{"status":"ERROR","error":"authentication required"}}),
        );
        assert!(matches!(result.kind, AgentExecutionEventKind::Diagnostic));
        assert_eq!(result.text.as_deref(), Some("authentication required"));
        let tool = parse_event(
            serde_json::json!({"event":"step_update","step_update":{"step_index":4,"step_type":"tool","state":"DONE","tool_name":"run_command","tool_info":{"error":{"type":"exit","message":"failed"}}}}),
        );
        assert!(tool.text.unwrap().contains("failed"));
    }
    #[test]
    fn multiple_response_steps_are_not_duplicated_by_aggregate_final_response() {
        let mut decoder = streaming::OutputDecoder::new(AgentOutputChannel::Stdout);
        let events=decoder.push(br#"{"event":"step_update","step_update":{"step_index":1,"step_type":"agent_response","state":"DONE","text_delta":"first "}}
{"event":"step_update","step_update":{"step_index":2,"step_type":"agent_response","state":"DONE","text_delta":"second"}}
{"event":"result","result":{"status":"SUCCESS","response":"first second"}}
"#,true);
        let text = events
            .iter()
            .filter(|event| matches!(event.kind, AgentExecutionEventKind::Output))
            .filter_map(|event| event.text.as_deref())
            .collect::<String>();
        assert_eq!(text, "first second");
        assert!(matches!(events[2].kind, AgentExecutionEventKind::Raw));
    }
}
