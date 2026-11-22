use super::*;

pub fn parse_stream_json_line(line: &str) -> Option<AgentExecutionEvent> {
    Some(parse_json_value(serde_json::from_str(line.trim()).ok()?))
}

fn parse_json_value(raw: Value) -> AgentExecutionEvent {
    if raw["event"].is_string() {
        return crate::antigravity_stream::parse_event(raw);
    }
    let value = if raw["type"] == "stream_event" {
        &raw["event"]
    } else {
        &raw
    };
    let event_type = value["type"].as_str().unwrap_or("");
    let item = value
        .get("item")
        .or_else(|| value.get("part"))
        .or_else(|| value.get("content_block"))
        .unwrap_or(value);
    let item_type = item["type"].as_str().unwrap_or("");
    let tool = looks_like_tool_call(value)
        || matches!(
            item_type,
            "command_execution"
                | "file_change"
                | "mcp_tool_call"
                | "web_search"
                | "tool_use"
                | "tool_result"
        );
    let delta = value
        .pointer("/delta/text")
        .and_then(Value::as_str)
        .or_else(|| value.get("delta").and_then(Value::as_str));
    let text = if tool {
        Some(tool_text(value, item))
    } else {
        delta
            .map(str::to_owned)
            .or_else(|| find_event_text(value))
            .or_else(|| value.get("error").and_then(find_text))
            .or_else(|| {
                (event_type == "result")
                    .then(|| value["result"].as_str().map(str::to_owned))
                    .flatten()
            })
    };
    let kind = if tool {
        AgentExecutionEventKind::ToolCall
    } else if event_type == "error" || event_type == "turn.failed" || value["is_error"] == true {
        AgentExecutionEventKind::Diagnostic
    } else if text.is_some() && !matches!(event_type, "system" | "status") {
        AgentExecutionEventKind::Output
    } else {
        AgentExecutionEventKind::Raw
    };
    let item_id = item["id"]
        .as_str()
        .or_else(|| value["item_id"].as_str())
        .or_else(|| value["part_id"].as_str())
        .or_else(|| value.pointer("/message/id").and_then(Value::as_str))
        .map(str::to_owned)
        .or_else(|| value["index"].as_u64().map(|i| format!("content_{i}")));
    let mode = if delta.is_some() || event_type.ends_with(".delta") {
        AgentTextMode::Delta
    } else if item_id.is_some() || matches!(event_type, "assistant" | "result") {
        AgentTextMode::Snapshot
    } else {
        AgentTextMode::Message
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

fn tool_text(event: &Value, item: &Value) -> String {
    let state = item.get("state").unwrap_or(item);
    let label = item["command"]
        .as_str()
        .or_else(|| item["name"].as_str())
        .or_else(|| item["tool"].as_str())
        .or_else(|| item["type"].as_str())
        .unwrap_or("tool");
    let status =
        state["status"]
            .as_str()
            .unwrap_or_else(|| match event["type"].as_str().unwrap_or("") {
                "item.started" | "content_block_start" => "running",
                "item.completed" | "content_block_stop" => "completed",
                "item.failed" => "failed",
                _ => "update",
            });
    let mut text = format!("{label} [{status}]");
    for (key, value) in [
        ("arguments", &item["arguments"]),
        (
            "input",
            item.get("input")
                .or_else(|| state.get("input"))
                .unwrap_or(&Value::Null),
        ),
        ("changes", &item["changes"]),
    ] {
        if !value.is_null() {
            text.push_str(&format!(
                "\n{key}: {}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            ));
        }
    }
    for value in [
        &item["aggregated_output"],
        &state["output"],
        &state["error"],
        &item["content"],
    ] {
        if let Some(value) = value.as_str().filter(|s| !s.is_empty()) {
            text.push('\n');
            text.push_str(value);
        }
    }
    if let Some(code) = item["exit_code"].as_i64() {
        text.push_str(&format!("\nexit code: {code}"));
    }
    text
}

/// Incremental stdout decoder. JSON frames stay buffered until complete, while
/// ordinary text is emitted immediately, including a final line without '\n'.
pub(super) struct OutputDecoder {
    channel: AgentOutputChannel,
    pending: Vec<u8>,
    json_line: Option<bool>,
    next_id: usize,
    last_json: bool,
    agy_response: String,
    last_output: Option<(String, String)>,
}

impl OutputDecoder {
    pub(super) fn new(channel: AgentOutputChannel) -> Self {
        Self {
            channel,
            pending: Vec::new(),
            json_line: None,
            next_id: 0,
            last_json: false,
            agy_response: String::new(),
            last_output: None,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8], eof: bool) -> Vec<AgentExecutionEvent> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        while !self.pending.is_empty() {
            if self.last_json && self.json_line.is_none() {
                let end = self
                    .pending
                    .iter()
                    .position(|b| !b.is_ascii_whitespace())
                    .unwrap_or(self.pending.len());
                self.pending.drain(..end);
                if self.pending.is_empty() {
                    break;
                }
            }
            if self.json_line.is_none() {
                self.json_line = self
                    .pending
                    .iter()
                    .find(|b| !b.is_ascii_whitespace())
                    .map(|b| *b == b'{' || *b == b'[');
            }
            let newline = self.pending.iter().position(|b| *b == b'\n').map(|i| i + 1);
            if self.channel == AgentOutputChannel::Stdout && self.json_line == Some(true) {
                let end = newline.unwrap_or(self.pending.len());
                match serde_json::from_slice::<Value>(&self.pending[..end]) {
                    Ok(raw) => {
                        self.pending.drain(..end);
                        self.json_line = None;
                        self.last_json = true;
                        let event = parse_json_value(raw);
                        events.push(self.normalize(event));
                        continue;
                    }
                    Err(error) if newline.is_none() && !eof && error.is_eof() => break,
                    Err(_) => {
                        self.json_line = Some(false);
                        self.last_json = false;
                        continue;
                    }
                }
            }
            let end = newline.unwrap_or(self.pending.len());
            let consumed = match std::str::from_utf8(&self.pending[..end]) {
                Ok(_) => end,
                Err(error) if error.error_len().is_none() && !eof => error.valid_up_to(),
                Err(_) => end,
            };
            if consumed == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&self.pending[..consumed]).into_owned();
            self.pending.drain(..consumed);
            if newline == Some(consumed) {
                self.json_line = None;
            }
            self.last_json = false;
            events.push(self.plain(text));
        }
        events
    }

    fn plain(&self, text: String) -> AgentExecutionEvent {
        AgentExecutionEvent {
            kind: if self.channel == AgentOutputChannel::Stderr {
                AgentExecutionEventKind::Diagnostic
            } else {
                AgentExecutionEventKind::Output
            },
            text: Some(text),
            raw: None,
            stream: Some(AgentStreamMetadata {
                channel: self.channel.clone(),
                mode: AgentTextMode::Delta,
                item_id: Some(
                    if self.channel == AgentOutputChannel::Stderr {
                        "stderr"
                    } else {
                        "stdout"
                    }
                    .into(),
                ),
            }),
        }
    }

    fn normalize(&mut self, mut event: AgentExecutionEvent) -> AgentExecutionEvent {
        if let Some(stream) = event.stream.as_mut() {
            let agy_result = event
                .raw
                .as_ref()
                .is_some_and(|raw| raw["event"] == "result");
            let agy_delta = event.raw.as_ref().is_some_and(|raw| {
                raw["event"] == "step_update" && raw["step_update"]["step_type"] == "agent_response"
            });
            if matches!(event.kind, AgentExecutionEventKind::Output)
                && agy_result
                && !self.agy_response.is_empty()
            {
                if let Some(text) = event
                    .text
                    .as_ref()
                    .filter(|text| text.starts_with(&self.agy_response))
                {
                    let suffix = &text[self.agy_response.len()..];
                    if suffix.is_empty() {
                        event.kind = AgentExecutionEventKind::Raw;
                        event.text = Some("result [SUCCESS]".into());
                        stream.mode = AgentTextMode::Message;
                        stream.item_id = None;
                        return event;
                    }
                    if let Some((id, previous)) = &self.last_output {
                        stream.item_id = Some(id.clone());
                        stream.mode = AgentTextMode::Snapshot;
                        event.text = Some(format!("{previous}{suffix}"));
                    }
                }
            }
            if agy_delta {
                if let Some(text) = &event.text {
                    self.agy_response.push_str(text);
                }
            }
            let is_output = matches!(event.kind, AgentExecutionEventKind::Output);
            let terminal = event
                .raw
                .as_ref()
                .is_some_and(|raw| matches!(raw["type"].as_str(), Some("assistant" | "result")));
            if is_output && terminal {
                if let (Some((id, previous)), Some(text)) = (&self.last_output, &event.text) {
                    if !previous.is_empty() && text.starts_with(previous) {
                        stream.item_id = Some(id.clone());
                        stream.mode = AgentTextMode::Snapshot;
                    }
                }
            }
            if is_output && stream.item_id.is_none() {
                if stream.mode == AgentTextMode::Delta {
                    stream.item_id = self.last_output.as_ref().map(|(id, _)| id.clone());
                }
                if stream.item_id.is_none() {
                    stream.item_id = Some(format!("message_{}", self.next_id));
                    self.next_id += 1;
                }
            }
            if is_output {
                if let (Some(id), Some(text)) = (&stream.item_id, &event.text) {
                    let prior = self
                        .last_output
                        .as_ref()
                        .filter(|(last, _)| last == id)
                        .map(|(_, text)| text.as_str())
                        .unwrap_or("");
                    let text = if stream.mode == AgentTextMode::Delta {
                        format!("{prior}{text}")
                    } else {
                        text.clone()
                    };
                    self.last_output = Some((id.clone(), text));
                }
            }
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_separators_do_not_create_fake_text_and_invalid_json_prefix_is_immediate() {
        let mut decoder = OutputDecoder::new(AgentOutputChannel::Stdout);
        let events = decoder.push(br#"{"type":"message","text":"hi"}"#, false);
        assert_eq!(events.len(), 1);
        assert!(decoder.push(b"\n", false).is_empty());
        let event = decoder.push(b"{plain progress}", false).remove(0);
        assert_eq!(event.text.as_deref(), Some("{plain progress}"));
        assert_eq!(event.stream.unwrap().mode, AgentTextMode::Delta);
        let failed = parse_stream_json_line(
            r#"{"type":"turn.failed","error":{"message":"quota exceeded"}}"#,
        )
        .unwrap();
        assert!(matches!(failed.kind, AgentExecutionEventKind::Diagnostic));
        assert_eq!(failed.text.as_deref(), Some("quota exceeded"));
    }

    #[test]
    fn metadata_round_trips_and_legacy_event_has_no_stream() {
        let metadata = AgentStreamMetadata {
            channel: AgentOutputChannel::Stderr,
            mode: AgentTextMode::Delta,
            item_id: Some("stderr".into()),
        };
        assert_eq!(
            serde_json::from_str::<AgentStreamMetadata>(&serde_json::to_string(&metadata).unwrap())
                .unwrap(),
            metadata
        );
        assert!(serde_json::from_str::<AgentStreamMetadata>(
            r#"{"channel":"unknown","mode":"delta"}"#
        )
        .is_err());
        let legacy: AgentExecutionEvent =
            serde_json::from_str(r#"{"kind":"Output","text":"old","raw":null}"#).unwrap();
        assert!(legacy.stream.is_none());
        assert!(serde_json::from_str::<AgentExecutionEvent>("{}").is_err());
    }
    #[test]
    fn plain_output_and_split_unicode_are_immediate_and_lossless() {
        let mut decoder = OutputDecoder::new(AgentOutputChannel::Stdout);
        let mut events = decoder.push(b"before newline ", false);
        assert_eq!(events[0].text.as_deref(), Some("before newline "));
        assert!(decoder.push(&[0xe4], false).is_empty());
        events.extend(decoder.push(&[0xb8, 0xad, b'\n'], false));
        events.extend(decoder.push("👩‍💻 tail".as_bytes(), true));
        let text = events
            .iter()
            .filter_map(|e| e.text.as_deref())
            .collect::<String>();
        assert_eq!(text, "before newline 中\n👩‍💻 tail");
        assert!(events
            .iter()
            .all(|e| e.stream.as_ref().unwrap().mode == AgentTextMode::Delta));
    }
    #[test]
    fn json_fragment_is_not_shown_as_partial_json_and_stderr_is_diagnostic() {
        let mut decoder = OutputDecoder::new(AgentOutputChannel::Stdout);
        assert!(decoder
            .push(
                br#"{"type":"item.completed","item":{"id":"m","type":"agent_message","text":"hel"#,
                false
            )
            .is_empty());
        let events = decoder.push(b"lo\"}}\n", false);
        assert_eq!(events[0].text.as_deref(), Some("hello"));
        assert_eq!(
            events[0].stream.as_ref().unwrap().item_id.as_deref(),
            Some("m")
        );
        let mut stderr = OutputDecoder::new(AgentOutputChannel::Stderr);
        let event = stderr.push(b"login pending", false).remove(0);
        assert!(matches!(event.kind, AgentExecutionEventKind::Diagnostic));
        assert_eq!(event.text.as_deref(), Some("login pending"));
        assert_eq!(event.stream.unwrap().channel, AgentOutputChannel::Stderr);
    }
    #[test]
    fn codex_and_opencode_tool_status_preserve_command_output_and_exit() {
        let codex=parse_stream_json_line(r#"{"type":"item.completed","item":{"id":"tool_1","type":"command_execution","command":"cargo test","aggregated_output":"3 passed","exit_code":1,"status":"failed"}}"#).unwrap();
        assert!(matches!(codex.kind, AgentExecutionEventKind::ToolCall));
        let text = codex.text.unwrap();
        assert!(
            text.contains("cargo test")
                && text.contains("failed")
                && text.contains("3 passed")
                && text.contains("exit code: 1")
        );
        let opencode=parse_stream_json_line(r#"{"type":"tool_use","part":{"id":"p1","type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"pwd"},"output":"/repo"}}}"#).unwrap();
        assert!(matches!(opencode.kind, AgentExecutionEventKind::ToolCall));
        assert!(opencode.text.unwrap().contains("/repo"));
        assert!(parse_stream_json_line("invalid").is_none());
    }
    #[test]
    fn native_deltas_and_terminal_snapshot_share_identity_without_fake_tokens() {
        let mut decoder = OutputDecoder::new(AgentOutputChannel::Stdout);
        let mut events=decoder.push(br#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello "}}}
"#,false);
        events.extend(decoder.push(br#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"world"}}}
{"type":"result","result":"hello world","status":"success"}
"#,false));
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0].stream.as_ref().unwrap().mode,
            AgentTextMode::Delta
        );
        assert_eq!(
            events[2].stream.as_ref().unwrap().mode,
            AgentTextMode::Snapshot
        );
        assert_eq!(
            events[0].stream.as_ref().unwrap().item_id,
            events[2].stream.as_ref().unwrap().item_id
        );
    }
    #[test]
    fn unknown_json_and_incomplete_tail_are_preserved() {
        let mut decoder = OutputDecoder::new(AgentOutputChannel::Stdout);
        let events = decoder.push(b"{\"type\":\"future_event\",\"custom\":42}\n{broken", true);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].raw.as_ref().unwrap()["custom"], 42);
        assert_eq!(events[1].text.as_deref(), Some("{broken"));
    }
}
