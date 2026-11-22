use super::*;
use crate::streaming::OutputDecoder;
use std::sync::mpsc;
use std::thread::JoinHandle;

struct PipePacket {
    channel: AgentOutputChannel,
    bytes: Vec<u8>,
    eof: bool,
}

fn read_pipe(
    mut pipe: impl Read + Send + 'static,
    channel: AgentOutputChannel,
    sender: mpsc::Sender<PipePacket>,
) -> JoinHandle<std::io::Result<()>> {
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(PipePacket {
                        channel,
                        bytes: Vec::new(),
                        eof: true,
                    });
                    return Ok(());
                }
                Ok(count) => {
                    if sender
                        .send(PipePacket {
                            channel: channel.clone(),
                            bytes: buffer[..count].to_vec(),
                            eof: false,
                        })
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    })
}

pub(super) fn run_command_with_timeout_streaming(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    timeout: Duration,
    mut sink: Option<&mut AgentEventSink<'_>>,
) -> Result<CommandRunOutput> {
    let mut child = Command::new(command)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {command}"))?;
    let stdout = child
        .stdout
        .take()
        .context("spawned command did not expose stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("spawned command did not expose stderr")?;
    let (sender, receiver) = mpsc::channel();
    let stdout_thread = read_pipe(stdout, AgentOutputChannel::Stdout, sender.clone());
    let stderr_thread = read_pipe(stderr, AgentOutputChannel::Stderr, sender);
    let mut capture = ProcessCapture::new();
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        capture.drain(&receiver, &mut sink);
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    stdout_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))?
        .context("read provider stdout")?;
    stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))?
        .context("read provider stderr")?;
    // All producers have stopped. Drain every queued packet before reporting completion.
    while capture.drain(&receiver, &mut sink) > 0 {}
    Ok(CommandRunOutput {
        output: Output {
            status,
            stdout: capture.stdout,
            stderr: capture.stderr,
        },
        timed_out,
        timeout,
        events: capture.events,
    })
}

struct ProcessCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    events: Vec<AgentExecutionEvent>,
    stdout_decoder: OutputDecoder,
    stderr_decoder: OutputDecoder,
}
impl ProcessCapture {
    fn new() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            events: Vec::new(),
            stdout_decoder: OutputDecoder::new(AgentOutputChannel::Stdout),
            stderr_decoder: OutputDecoder::new(AgentOutputChannel::Stderr),
        }
    }
    fn drain(
        &mut self,
        receiver: &mpsc::Receiver<PipePacket>,
        sink: &mut Option<&mut AgentEventSink<'_>>,
    ) -> usize {
        let mut packets = 0;
        for packet in receiver.try_iter().take(256) {
            packets += 1;
            let events = match packet.channel {
                AgentOutputChannel::Stdout => {
                    self.stdout.extend_from_slice(&packet.bytes);
                    self.stdout_decoder.push(&packet.bytes, packet.eof)
                }
                AgentOutputChannel::Stderr => {
                    self.stderr.extend_from_slice(&packet.bytes);
                    self.stderr_decoder.push(&packet.bytes, packet.eof)
                }
            };
            for event in events {
                if let Some(sink) = sink.as_deref_mut() {
                    sink(event.clone());
                }
                self.events.push(event);
            }
        }
        packets
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn plain_stdout_and_stderr_arrive_before_exit_without_waiting_for_newline() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let (mut stdout_at, mut stderr_at) = (None, None);
        let script = "printf 'early stdout'; printf 'early stderr' >&2; sleep 1; printf ' tail'";
        let mut sink = |event: AgentExecutionEvent| {
            if event
                .text
                .as_deref()
                .is_some_and(|s| s.contains("early stdout"))
            {
                stdout_at = Some(start.elapsed());
            }
            if matches!(event.kind, AgentExecutionEventKind::Diagnostic) {
                stderr_at = Some(start.elapsed());
            }
        };
        let run = run_command_with_timeout_streaming(
            "sh",
            &["-c".into(), script.into()],
            &dir.path().to_path_buf(),
            Duration::from_secs(3),
            Some(&mut sink),
        )
        .unwrap();
        assert!(run.output.status.success());
        assert!(stdout_at.unwrap() < Duration::from_millis(800));
        assert!(stderr_at.unwrap() < Duration::from_millis(800));
        assert_eq!(
            String::from_utf8(run.output.stdout).unwrap(),
            "early stdout tail"
        );
        assert_eq!(
            String::from_utf8(run.output.stderr).unwrap(),
            "early stderr"
        );
        assert!(run
            .events
            .iter()
            .any(|e| e.text.as_deref() == Some(" tail")));
    }
    #[test]
    fn split_utf8_json_tail_and_large_stderr_are_drained_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let script = r#"printf '\344'; sleep 0.05; printf '\270\255'; printf '\n{"type":"item.completed","item":{"id":"last","type":"agent_message","text":"FINAL"}}'; i=0; while [ $i -lt 2000 ]; do printf 'diagnostic line\n' >&2; i=$((i+1)); done"#;
        let run = run_command_with_timeout_streaming(
            "sh",
            &["-c".into(), script.into()],
            &dir.path().to_path_buf(),
            Duration::from_secs(3),
            None,
        )
        .unwrap();
        let text = run
            .events
            .iter()
            .filter(|e| matches!(e.kind, AgentExecutionEventKind::Output))
            .filter_map(|e| e.text.as_deref())
            .collect::<String>();
        assert_eq!(text, "中\nFINAL");
        assert_eq!(text.matches("FINAL").count(), 1);
        assert_eq!(run.output.stderr.len(), 2000 * 16);
    }
    #[test]
    fn spawn_failure_is_reported_and_timeout_retains_partial_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        assert!(run_command_with_timeout_streaming(
            "/definitely/not/a/provider",
            &[],
            &cwd,
            Duration::from_secs(1),
            None
        )
        .is_err());
        let run = run_command_with_timeout_streaming(
            "sh",
            &[
                "-c".into(),
                "printf 'partial' >&2; while :; do :; done".into(),
            ],
            &cwd,
            Duration::from_millis(100),
            None,
        )
        .unwrap();
        assert!(run.timed_out);
        assert_eq!(run.output.stderr, b"partial");
        assert!(run
            .events
            .iter()
            .any(|e| matches!(e.kind, AgentExecutionEventKind::Diagnostic)));
    }
}
