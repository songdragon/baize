use super::*;

pub(super) fn start_prompt_sse_reader(
    sender: Sender<PromptTaskMessage>,
    session_id: String,
    provider_id: String,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        if let Err(error) = read_prompt_sse_events(sender.clone(), &session_id, &provider_id, stop)
        {
            let _ = sender.send(PromptTaskMessage::StreamUnavailable(format!("{error:#}")));
        }
    });
}

fn read_prompt_sse_events(
    sender: Sender<PromptTaskMessage>,
    session_id: &str,
    provider_id: &str,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let address = daemon_address();
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_millis(DAEMON_CONNECT_TIMEOUT_MS))
            .with_context(|| format!("connect to daemon at {address}"))?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(DAEMON_REQUEST_TIMEOUT_SECONDS)))?;
    // HTTP/1.0 uses a close-delimited stream, avoiding chunk framing inside SSE data lines.
    write!(
        stream,
        "GET /events HTTP/1.0\r\nHost: {address}\r\nAccept: text/event-stream\r\n\r\n"
    )?;
    read_sse_lines(&mut BufReader::new(stream), &stop, |event| {
        if event_matches_session_provider(&event, session_id, provider_id) {
            let _ = sender.send(PromptTaskMessage::StreamEvent(event));
        }
    })
}

fn read_sse_lines(
    reader: &mut impl BufRead,
    stop: &AtomicBool,
    mut emit: impl FnMut(Value),
) -> Result<()> {
    let mut bytes = Vec::new();
    let mut headers = true;
    let mut first = true;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => return Err(anyhow!("event stream closed")),
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
                    }
                } else if let Some(event) = parse_sse_data_line(line) {
                    emit(event);
                }
                bytes.clear();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // read_until may have appended a partial UTF-8 line. Preserve it across timeouts.
            }
            Err(error) => return Err(error).context("read daemon event stream"),
        }
    }
}

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
    fn preserves_partial_unicode_event_across_read_timeouts() {
        let mut events = vec![];
        let result = read_sse_lines(
            &mut InterruptedReader { part: 0 },
            &AtomicBool::new(false),
            |event| events.push(event),
        );
        assert!(result.is_err());
        assert_eq!(events, vec![json!({"text":"中"})]);
    }
    #[test]
    fn rejects_http_failure_and_stops_without_reading_after_cancel() {
        let mut reader = std::io::Cursor::new(b"HTTP/1.0 503 Unavailable\r\n\r\n");
        assert!(read_sse_lines(&mut reader, &AtomicBool::new(false), |_| {}).is_err());
        let mut reader = std::io::Cursor::new(b"invalid");
        assert!(
            read_sse_lines(&mut reader, &AtomicBool::new(true), |_| panic!(
                "unexpected event"
            ))
            .is_ok()
        );
    }
}
