use super::*;

pub(super) struct PromptEventStream {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for PromptEventStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn start_prompt_sse_reader(
    sender: Sender<PromptTaskMessage>,
    session_id: String,
    provider_id: String,
    stop: Arc<AtomicBool>,
) -> Result<PromptEventStream> {
    let (ready_sender, ready_receiver) = mpsc::channel();
    let worker_stop = stop.clone();
    let worker = thread::spawn(move || {
        let mut initialized = false;
        while !worker_stop.load(Ordering::SeqCst) {
            let result =
                read_prompt_sse_events(&sender, &session_id, &provider_id, &worker_stop, || {
                    send_recorded_events(&sender, &session_id, &provider_id)?;
                    if !initialized {
                        let _ = ready_sender.send(Ok(()));
                        initialized = true;
                    } else {
                        let _ = sender.send(PromptTaskMessage::StreamRecovered);
                    }
                    Ok(())
                });
            if worker_stop.load(Ordering::SeqCst) {
                break;
            }
            if let Err(error) = result {
                if !initialized {
                    let _ = ready_sender.send(Err(format!("{error:#}")));
                    break;
                }
                let _ = sender.send(PromptTaskMessage::StreamUnavailable(format!("{error:#}")));
            }
            thread::sleep(Duration::from_millis(200));
        }
    });
    let stream = PromptEventStream {
        stop,
        worker: Some(worker),
    };
    wait_for_subscription(&ready_receiver, Duration::from_secs(10))?;
    Ok(stream)
}

fn wait_for_subscription(receiver: &Receiver<Result<(), String>>, timeout: Duration) -> Result<()> {
    receiver
        .recv_timeout(timeout)
        .context("wait for daemon event subscription")?
        .map_err(|error| anyhow!(error))
}

pub(super) fn send_recorded_events(
    sender: &Sender<PromptTaskMessage>,
    session_id: &str,
    provider_id: &str,
) -> Result<()> {
    let history = load_session_events(session_id)?;
    publish_recorded_events(sender, &history, session_id, provider_id);
    Ok(())
}

fn publish_recorded_events(
    sender: &Sender<PromptTaskMessage>,
    history: &Value,
    session_id: &str,
    provider_id: &str,
) {
    if let Some(events) = history["events"].as_array() {
        for event in events
            .iter()
            .filter(|event| event_matches_session_provider(event, session_id, provider_id))
        {
            let _ = sender.send(PromptTaskMessage::StreamEvent(event.clone()));
        }
    }
}

fn read_prompt_sse_events(
    sender: &Sender<PromptTaskMessage>,
    session_id: &str,
    provider_id: &str,
    stop: &AtomicBool,
    on_ready: impl FnMut() -> Result<()>,
) -> Result<()> {
    let address = daemon_address();
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_millis(DAEMON_CONNECT_TIMEOUT_MS))
            .with_context(|| format!("connect to daemon at {address}"))?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(DAEMON_REQUEST_TIMEOUT_SECONDS)))?;
    // HTTP/1.0 keeps SSE close-delimited rather than interleaving chunk framing.
    write!(
        stream,
        "GET /events HTTP/1.0\r\nHost: {address}\r\nAccept: text/event-stream\r\n\r\n"
    )?;
    read_sse_lines(&mut BufReader::new(stream), stop, on_ready, |event| {
        if event["event_type"] == "daemon.resync_required" {
            anyhow::bail!("event subscriber lagged; recovering recorded history");
        }
        if event_matches_session_provider(&event, session_id, provider_id) {
            let _ = sender.send(PromptTaskMessage::StreamEvent(event));
        }
        Ok(())
    })
}

fn read_sse_lines(
    reader: &mut impl BufRead,
    stop: &AtomicBool,
    mut ready: impl FnMut() -> Result<()>,
    mut emit: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let mut bytes = Vec::new();
    let mut data = String::new();
    let (mut headers, mut first) = (true, true);
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => {
                if !data.is_empty() {
                    emit(
                        serde_json::from_str(data.trim_end())
                            .context("parse trailing SSE event")?,
                    )?;
                }
                anyhow::bail!("event stream closed");
            }
            Ok(_) => {
                let line = std::str::from_utf8(&bytes).context("event stream is not UTF-8")?;
                if first {
                    first = false;
                    if !line.starts_with("HTTP/1.") || !line.contains(" 200 ") {
                        anyhow::bail!("event stream rejected: {}", line.trim());
                    }
                } else if headers {
                    if line.trim().is_empty() {
                        headers = false;
                        ready()?;
                    }
                } else if line.trim().is_empty() {
                    if !data.is_empty() {
                        emit(serde_json::from_str(data.trim_end()).context("parse SSE event")?)?;
                        data.clear();
                    }
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push_str(
                        value
                            .strip_prefix(' ')
                            .unwrap_or(value)
                            .trim_end_matches(['\r', '\n']),
                    );
                    data.push('\n');
                }
                bytes.clear();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error).context("read daemon event stream"),
        }
    }
}

#[cfg(test)]
pub(super) fn parse_sse_data_line(line: &str) -> Option<Value> {
    serde_json::from_str(line.trim_start().strip_prefix("data:")?.trim()).ok()
}

pub(super) fn event_matches_session_provider(
    event: &Value,
    session_id: &str,
    provider_id: &str,
) -> bool {
    event["session_id"].as_str() == Some(session_id)
        && event["provider_id"].as_str() == Some(provider_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct InterruptedReader {
        part: usize,
    }
    impl Read for InterruptedReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            unreachable!()
        }
    }
    impl BufRead for InterruptedReader {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            unreachable!()
        }
        fn consume(&mut self, _: usize) {
            unreachable!()
        }
        fn read_until(&mut self, _: u8, out: &mut Vec<u8>) -> std::io::Result<usize> {
            let part = self.part;
            self.part += 1;
            let value: &[u8] = match part {
                0 => b"HTTP/1.0 200 OK\r\n",
                1 => b"\r\n",
                2 => {
                    out.extend_from_slice(b"data: {\"text\":\"\xe4");
                    return Err(std::io::ErrorKind::TimedOut.into());
                }
                3 => b"\xb8\xad\"}\n",
                _ => return Ok(0),
            };
            out.extend_from_slice(value);
            Ok(value.len())
        }
    }
    #[test]
    fn reconnect_recovers_missing_events_and_deduplicates_overlapping_live_frames() {
        let event = |id: &str, text: &str| json!({"id":id,"session_id":"task_1","provider_id":"codex","event_type":"session.agent.output","payload":{"text":text,"stream":{"item_id":"m","channel":"stdout","mode":"delta"}}});
        let first = event("1", "A");
        let missed = event("2", "B");
        let tail = event("3", "C");
        let (sender, receiver) = mpsc::channel();
        let raw = format!("HTTP/1.0 200 OK\r\n\r\ndata: {first}\n\n");
        let _ = read_sse_lines(
            &mut std::io::Cursor::new(raw),
            &AtomicBool::new(false),
            || Ok(()),
            |event| {
                sender.send(PromptTaskMessage::StreamEvent(event)).unwrap();
                Ok(())
            },
        );
        let history = json!({"events":[first,missed.clone(),{"session_id":"other","provider_id":"codex","id":"ignore"}]});
        let raw = format!("HTTP/1.0 200 OK\r\n\r\ndata: {missed}\n\ndata: {tail}\n\n");
        let _ = read_sse_lines(
            &mut std::io::Cursor::new(raw),
            &AtomicBool::new(false),
            || {
                publish_recorded_events(&sender, &history, "task_1", "codex");
                Ok(())
            },
            |event| {
                sender.send(PromptTaskMessage::StreamEvent(event)).unwrap();
                Ok(())
            },
        );
        let mut state = TuiState::default();
        for message in receiver.try_iter() {
            if let PromptTaskMessage::StreamEvent(event) = message {
                apply_prompt_stream_event(&mut state, &event);
            }
        }
        assert!(state.transcript_text().contains("ABC"));
        assert_eq!(state.transcript_text().matches("ABC").count(), 1);
        assert_eq!(state.seen_event_ids.len(), 3);
    }

    #[test]
    fn preserves_partial_unicode_event_across_read_timeouts() {
        let mut events = vec![];
        let result = read_sse_lines(
            &mut InterruptedReader { part: 0 },
            &AtomicBool::new(false),
            || Ok(()),
            |event| {
                events.push(event);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(events, vec![json!({"text":"中"})]);
    }
    #[test]
    fn subscription_is_ready_before_first_event_and_rejects_failures() {
        let log = std::cell::RefCell::new(Vec::new());
        let mut reader =
            std::io::Cursor::new(b"HTTP/1.0 200 OK\r\n\r\ndata: {\"text\":\"first\"}\n\n");
        assert!(read_sse_lines(
            &mut reader,
            &AtomicBool::new(false),
            || {
                log.borrow_mut().push("ready");
                Ok(())
            },
            |_| {
                log.borrow_mut().push("event");
                Ok(())
            }
        )
        .is_err());
        assert_eq!(*log.borrow(), vec!["ready", "event"]);
        let mut reader = std::io::Cursor::new(b"HTTP/1.0 503 Unavailable\r\n\r\n");
        assert!(read_sse_lines(
            &mut reader,
            &AtomicBool::new(false),
            || panic!("failed subscription must not be ready"),
            |_| Ok(())
        )
        .is_err());
        let mut reader = std::io::Cursor::new(b"invalid");
        assert!(read_sse_lines(
            &mut reader,
            &AtomicBool::new(true),
            || panic!("canceled"),
            |_| Ok(())
        )
        .is_ok());
    }
    #[test]
    fn ready_channel_reports_success_error_timeout_and_disconnect() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(())).unwrap();
        assert!(wait_for_subscription(&rx, Duration::from_millis(10)).is_ok());
        tx.send(Err("offline".into())).unwrap();
        assert!(wait_for_subscription(&rx, Duration::from_millis(10))
            .unwrap_err()
            .to_string()
            .contains("offline"));
        assert!(wait_for_subscription(&rx, Duration::from_millis(1)).is_err());
        drop(tx);
        assert!(wait_for_subscription(&rx, Duration::from_millis(10)).is_err());
    }
    #[test]
    fn multiline_sse_and_trailing_events_are_drained_but_malformed_data_fails() {
        let mut events = vec![];
        let mut reader =
            std::io::Cursor::new(b"HTTP/1.0 200 OK\r\n\r\ndata: {\"text\":\ndata: \"last\"}\n\n");
        let _ = read_sse_lines(
            &mut reader,
            &AtomicBool::new(false),
            || Ok(()),
            |event| {
                events.push(event);
                Ok(())
            },
        );
        assert_eq!(events, vec![json!({"text":"last"})]);
        let mut reader = std::io::Cursor::new(b"HTTP/1.0 200 OK\r\n\r\ndata: broken\n\n");
        assert!(
            read_sse_lines(&mut reader, &AtomicBool::new(false), || Ok(()), |_| Ok(()))
                .unwrap_err()
                .to_string()
                .contains("parse SSE")
        );
    }
}
