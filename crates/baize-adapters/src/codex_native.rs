use super::*;
use crate::codex_transport::RpcProcess;
use std::collections::HashMap;

pub const APPROVAL_WAIT_SECONDS: u64 = 600;

pub(super) fn run(
    request: AgentPromptRequest,
    sink: Option<&mut AgentEventSink<'_>>,
    control: &AgentControl,
) -> Result<AgentRunResult> {
    run_with_command(
        request,
        sink,
        control,
        "codex",
        &["app-server".into(), "--listen".into(), "stdio://".into()],
    )
}

fn result(
    request: &AgentPromptRequest,
    native: Option<String>,
    events: Vec<AgentExecutionEvent>,
    status: &str,
    message: String,
) -> AgentRunResult {
    let success = status == "completed";
    AgentRunResult {
        provider_id: request.provider_id.clone(),
        success,
        exit_code: if success {
            Some(0)
        } else if status == "failed" {
            Some(1)
        } else {
            None
        },
        native_session_id: native,
        events,
        stderr: message.clone(),
        error: if success {
            None
        } else {
            Some(AgentErrorDetail {
                kind: if status == "interrupted" {
                    AgentErrorKind::Canceled
                } else {
                    AgentErrorKind::ProcessFailure
                },
                source: AgentErrorSource::Runtime,
                message,
            })
        },
    }
}

fn emit(
    events: &mut Vec<AgentExecutionEvent>,
    sink: &mut Option<&mut AgentEventSink<'_>>,
    event: AgentExecutionEvent,
) {
    if let Some(sink) = sink.as_deref_mut() {
        sink(event.clone());
    }
    events.push(event);
}

pub(super) fn run_with_command(
    request: AgentPromptRequest,
    mut sink: Option<&mut AgentEventSink<'_>>,
    control: &AgentControl,
    command: &str,
    args: &[String],
) -> Result<AgentRunResult> {
    if control.is_canceled() {
        return Ok(result(
            &request,
            request.session_id.clone(),
            vec![],
            "interrupted",
            "Canceled before native turn submission; no prompt sent.".into(),
        ));
    }
    let mut rpc = RpcProcess::spawn(command, args, &request.cwd)?;
    rpc.send(serde_json::json!({"id":"baize-init","method":"initialize","params":{"clientInfo":{"name":"baize","title":"Baize","version":"0.1.0"},"capabilities":{"experimentalApi":false}}}))?;
    let mut native = None;
    let mut turn = None::<String>;
    let mut events = vec![];
    let mut items = HashMap::<String, Value>::new();
    let mut pending = HashMap::<String, Value>::new();
    let started = Instant::now();
    let mut tick = Instant::now();
    let mut waiting = Duration::ZERO;
    let mut interrupt_at = None::<Instant>;
    let mut turn_submitted = false;
    let mut interrupt_sent = false;
    let outcome = (|| -> Result<AgentRunResult> {
        loop {
            let now = Instant::now();
            if !pending.is_empty() {
                waiting += now.duration_since(tick);
            }
            tick = now;
            let expired = now.duration_since(started).saturating_sub(waiting)
                >= timeout_for(&request)
                || waiting >= Duration::from_secs(APPROVAL_WAIT_SECONDS);
            if control.is_canceled() || expired {
                interrupt_at.get_or_insert(now);
                if !interrupt_sent && turn.is_some() {
                    let thread_id = native
                        .as_ref()
                        .context("native interrupt missing thread ID")?;
                    let turn_id = turn.as_ref().context("native interrupt missing turn ID")?;
                    rpc.send(serde_json::json!({"id":"baize-interrupt","method":"turn/interrupt","params":{"threadId":thread_id,"turnId":turn_id}}))?;
                    for (id, value) in pending.drain() {
                        control.close_approval(&id);
                        rpc.send(serde_json::json!({"id":value,"result":{"decision":"cancel"}}))?;
                    }
                    interrupt_at = Some(now);
                    interrupt_sent = true;
                } else if native.is_some() && !turn_submitted {
                    return Ok(result(
                        &request,
                        native.clone(),
                        events.clone(),
                        "interrupted",
                        "Canceled before native turn submission; no prompt sent.".into(),
                    ));
                } else if expired && native.is_none() {
                    anyhow::bail!("native initialization timed out; no automatic retry");
                }
            }
            if interrupt_at.is_some_and(|at| at.elapsed() > Duration::from_secs(5)) {
                anyhow::bail!("native interruption was requested but no terminal evidence arrived; execution remains unconfirmed");
            }
            let ids = pending.keys().cloned().collect::<Vec<_>>();
            for id in ids {
                if let Some(approve) = control.take_decision(&id)? {
                    if let Some(value) = pending.remove(&id) {
                        rpc.send(serde_json::json!({"id":value,"result":{"decision":if approve {"accept"}else{"decline"}}}))?;
                    }
                }
            }
            let (messages, diagnostic) = rpc.receive()?;
            if !diagnostic.trim().is_empty() {
                emit(
                    &mut events,
                    &mut sink,
                    AgentExecutionEvent {
                        kind: AgentExecutionEventKind::Diagnostic,
                        text: Some(diagnostic),
                        raw: None,
                        stream: None,
                    },
                );
            }
            for message in messages {
                let method = message["method"].as_str().unwrap_or("");
                let params = &message["params"];
                if message.get("method").is_none() {
                    if message.get("error").is_some() {
                        anyhow::bail!("native request failed: {}", message["error"]);
                    }
                    match message["id"].as_str() {
                        Some("baize-init") => {
                            rpc.send(serde_json::json!({"method":"initialized"}))?;
                            let mut params = serde_json::json!({"cwd":request.cwd,"approvalPolicy":if request.execution_policy==AgentExecutionPolicy::Ask {"untrusted"}else{"never"},"sandbox":if request.execution_policy==AgentExecutionPolicy::Deny {"read-only"}else{"workspace-write"}});
                            let method = if let Some(id) = &request.session_id {
                                params["threadId"] = id.clone().into();
                                "thread/resume"
                            } else {
                                "thread/start"
                            };
                            rpc.send(serde_json::json!({"id":"baize-thread","method":method,"params":params}))?;
                        }
                        Some("baize-thread") => {
                            let id = message["result"]["thread"]["id"]
                                .as_str()
                                .context("native thread response missing ID")?
                                .to_string();
                            native = Some(id.clone());
                            if control.is_canceled() {
                                continue;
                            }
                            let mut input =
                                vec![serde_json::json!({"type":"text","text":request.prompt})];
                            input.extend(
                                request.image_paths.iter().map(
                                    |path| serde_json::json!({"type":"localImage","path":path}),
                                ),
                            );
                            rpc.send(serde_json::json!({"id":"baize-turn","method":"turn/start","params":{"threadId":id,"input":input}}))?;
                            turn_submitted = true;
                        }
                        Some("baize-turn") => {
                            turn = Some(
                                message["result"]["turn"]["id"]
                                    .as_str()
                                    .context("native turn response missing ID")?
                                    .into(),
                            );
                        }
                        _ => {}
                    }
                    continue;
                }
                if message.get("id").is_some() {
                    let scoped = params["threadId"].as_str() == native.as_deref()
                        && params["turnId"].as_str() == turn.as_deref()
                        && turn.is_some();
                    let supported = matches!(
                        method,
                        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
                    );
                    let safe_kind = params["kind"].as_str().is_none_or(|kind| kind == "command")
                        && params["grantRoot"].is_null();
                    if !scoped || !supported || !safe_kind {
                        rpc.send(serde_json::json!({"id":message["id"],"error":{"code":-32601,"message":"Baize does not support this scoped native interaction; use the native entry point."}}))?;
                        emit(&mut events,&mut sink,AgentExecutionEvent{kind:AgentExecutionEventKind::Diagnostic,text:Some(format!("Native interaction {method} is unavailable in Baize; no approval was granted.")),raw:Some(message),stream:None});
                        continue;
                    }
                    if control.is_canceled() || interrupt_at.is_some() {
                        rpc.send(
                            serde_json::json!({"id":message["id"],"result":{"decision":"cancel"}}),
                        )?;
                        continue;
                    }
                    let id = message["id"].to_string();
                    control.register_approval(&id)?;
                    pending.insert(id.clone(), message["id"].clone());
                    let command = approval_description(
                        method,
                        params,
                        items.get(params["itemId"].as_str().unwrap_or("")),
                    );
                    emit(
                        &mut events,
                        &mut sink,
                        AgentExecutionEvent {
                            kind: AgentExecutionEventKind::Raw,
                            text: Some("Waiting for native approval".into()),
                            raw: Some(
                                serde_json::json!({"native_approval":{"request_id":id,"command":command,"reason":params["reason"].as_str().unwrap_or("Native tool requests a single-operation decision"),"cwd":params["cwd"],"method":method,"params":params}}),
                            ),
                            stream: None,
                        },
                    );
                    continue;
                }
                if method == "turn/started" && params["threadId"].as_str() == native.as_deref() {
                    turn = params["turn"]["id"].as_str().map(str::to_string);
                    continue;
                }
                if params["threadId"].as_str() != native.as_deref()
                    || params["turnId"]
                        .as_str()
                        .is_some_and(|id| Some(id) != turn.as_deref())
                {
                    continue;
                }
                if method == "turn/completed" && params["turn"]["id"].as_str() == turn.as_deref() {
                    let status = params["turn"]["status"]
                        .as_str()
                        .context("native turn completion missing status")?;
                    anyhow::ensure!(
                        matches!(status, "completed" | "interrupted" | "failed"),
                        "unknown native terminal status"
                    );
                    return Ok(result(
                        &request,
                        native.clone(),
                        events.clone(),
                        status,
                        if status == "interrupted" {
                            "Native turn/completed confirmed interruption; partial changes are retained.".into()
                        } else if status == "failed" {
                            params["turn"]["error"].to_string()
                        } else {
                            String::new()
                        },
                    ));
                }
                if matches!(method, "item/started" | "item/completed") {
                    if let Some(id) = params["item"]["id"].as_str() {
                        items.insert(id.into(), params["item"].clone());
                    }
                }
                if let Some(event) = execution_event(method, params) {
                    emit(&mut events, &mut sink, event);
                }
            }
        }
    })();
    for id in pending.keys() {
        control.close_approval(id);
    }
    outcome
}

fn approval_description(method: &str, params: &Value, item: Option<&Value>) -> String {
    let operation = if let Some(command) = params["command"].as_str() {
        command.to_string()
    } else if let Some(item) = item {
        serde_json::to_string_pretty(item).unwrap_or_else(|_| item.to_string())
    } else {
        params.to_string()
    };
    format!("{operation}\nNative method: {method}\nWorking directory: {}\nScope: this request only; no persistent or session-wide rule.",params["cwd"].as_str().unwrap_or("native thread cwd"))
}

fn execution_event(method: &str, params: &Value) -> Option<AgentExecutionEvent> {
    let raw = match method {
        "item/agentMessage/delta" => {
            serde_json::json!({"type":"item.delta","item_id":params["itemId"],"delta":params["delta"]})
        }
        "item/started" | "item/completed" => {
            let mut item = params["item"].clone();
            item["type"] = match item["type"].as_str()? {
                "agentMessage" => "agent_message",
                "commandExecution" => "command_execution",
                "fileChange" => "file_change",
                "mcpToolCall" => "mcp_tool_call",
                _ => return None,
            }
            .into();
            if item["type"] == "agent_message" && method == "item/started" {
                return None;
            }
            serde_json::json!({"type":if method=="item/started" {"item.started"}else{"item.completed"},"item":item})
        }
        "item/commandExecution/outputDelta" => {
            serde_json::json!({"type":"item.updated","item":{"id":params["itemId"],"type":"command_execution","aggregated_output":params["delta"]}})
        }
        _ => return None,
    };
    parse_stream_json_line(&raw.to_string())
}

#[cfg(all(test, unix))]
#[path = "codex_native_tests.rs"]
mod tests;
