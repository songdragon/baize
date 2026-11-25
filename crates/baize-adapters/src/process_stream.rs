use super::*;
use crate::streaming::OutputDecoder;
use std::sync::mpsc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;

pub(super) struct PipePacket {
    pub channel: AgentOutputChannel,
    pub bytes: Vec<u8>,
    pub eof: bool,
}

struct StopReaders(Arc<AtomicBool>);
impl Drop for StopReaders {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub(super) fn read_pipe(
    mut pipe: impl Read + Send + 'static,
    channel: AgentOutputChannel,
    sender: mpsc::Sender<PipePacket>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<std::io::Result<()>> {
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
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
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => return Err(error),
            }
        }
    })
}

#[cfg(test)]
pub(super) fn run_command_with_timeout_streaming(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    timeout: Duration,
    sink: Option<&mut AgentEventSink<'_>>,
) -> Result<CommandRunOutput> {
    run_command_controlled(command, args, cwd, timeout, sink, None)
}

#[cfg(unix)]
pub(super) fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> Result<()> {
    let fd = pipe.as_raw_fd();
    // Only alter the owned pipe's read end, so inherited writers cannot block cleanup.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(std::io::Error::last_os_error()).context("make provider pipe nonblocking");
    }
    Ok(())
}

pub(super) fn run_command_controlled(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    timeout: Duration,
    mut sink: Option<&mut AgentEventSink<'_>>,
    control: Option<&AgentControl>,
) -> Result<CommandRunOutput> {
    anyhow::ensure!(
        !control.is_some_and(AgentControl::is_canceled),
        "execution canceled before process launch"
    );
    let mut process = Command::new(command);
    process
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = crate::owned_process::spawn(&mut process)
        .with_context(|| format!("failed to spawn {command}"))?;
    let stdout = child
        .stdout
        .take()
        .context("spawned command did not expose stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("spawned command did not expose stderr")?;
    #[cfg(unix)]
    {
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
    }
    let (sender, receiver) = mpsc::channel();
    let stop_readers = Arc::new(AtomicBool::new(false));
    let _stop_on_error = StopReaders(stop_readers.clone());
    let stdout_thread = read_pipe(
        stdout,
        AgentOutputChannel::Stdout,
        sender.clone(),
        stop_readers.clone(),
    );
    let stderr_thread = read_pipe(
        stderr,
        AgentOutputChannel::Stderr,
        sender,
        stop_readers.clone(),
    );
    let mut capture = ProcessCapture::new();
    let started = Instant::now();
    let mut timed_out = false;
    let mut canceled = false;
    let status = loop {
        capture.drain(&receiver, &mut sink);
        if control.is_some_and(AgentControl::is_canceled) {
            canceled = true;
            break crate::owned_process::stop(&mut child)?;
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            break crate::owned_process::stop(&mut child)?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let drain_deadline = Instant::now() + Duration::from_millis(250);
    while !(stdout_thread.is_finished() && stderr_thread.is_finished())
        && Instant::now() < drain_deadline
    {
        capture.drain(&receiver, &mut sink);
        thread::sleep(Duration::from_millis(5));
    }
    let output_unconfirmed = !(stdout_thread.is_finished() && stderr_thread.is_finished());
    stop_readers.store(true, Ordering::SeqCst);
    #[cfg(unix)]
    {
        stdout_thread
            .join()
            .map_err(|_| anyhow::anyhow!("stdout reader panicked"))??;
        stderr_thread
            .join()
            .map_err(|_| anyhow::anyhow!("stderr reader panicked"))??;
    }
    // Non-Unix reader interruption has not been validated; never wait forever.
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
        canceled,
        output_unconfirmed,
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
    fn cancellation_stops_the_owned_group_and_does_not_report_native_success() {
        let dir = tempfile::tempdir().unwrap();
        let control = AgentControl::default();
        let mut sink = |event: AgentExecutionEvent| {
            if event
                .text
                .as_deref()
                .is_some_and(|text| text.contains("ready"))
            {
                control.cancel();
            }
        };
        let started = Instant::now();
        let run=run_command_controlled("sh", &["-c".into(),"trap '' TERM; (trap '' TERM; printf ready; sleep 30; printf 'must not run') & wait".into()],&dir.path().to_path_buf(),Duration::from_secs(5),Some(&mut sink),Some(&control)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(run.canceled && !run.output.status.success());
        assert!(!String::from_utf8_lossy(&run.output.stdout).contains("must not run"));
        let result = agent_result_from_command_run(ProviderId("antigravity".into()), "test", run);
        assert!(!result.success);
        assert_eq!(
            result.error.unwrap().kind,
            AgentErrorKind::ExecutionUnconfirmed
        );
    }
    #[test]
    fn inherited_pipes_do_not_hold_the_worker_forever_and_prelaunch_cancel_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let run = run_command_controlled(
            "sh",
            &["-c".into(), "sleep 1 & printf finished".into()],
            &dir.path().to_path_buf(),
            Duration::from_secs(5),
            None,
            None,
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_millis(900));
        assert!(run.output_unconfirmed);
        assert_eq!(run.output.stdout, b"finished");
        let control = AgentControl::default();
        control.cancel();
        assert!(run_command_controlled(
            "sh",
            &["-c".into(), "touch sent".into()],
            &dir.path().to_path_buf(),
            Duration::from_secs(1),
            None,
            Some(&control)
        )
        .is_err());
        assert!(!dir.path().join("sent").exists());
    }
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
