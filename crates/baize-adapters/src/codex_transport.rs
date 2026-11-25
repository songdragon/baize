use super::*;
use crate::owned_process::OwnedChild;
use crate::process_stream::{read_pipe, PipePacket};
use std::{
    io::Write,
    process::ChildStdin,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
};

pub(super) struct RpcProcess {
    child: OwnedChild,
    input: ChildStdin,
    receiver: mpsc::Receiver<PipePacket>,
    stop: Arc<AtomicBool>,
    readers: Vec<JoinHandle<std::io::Result<()>>>,
    buffer: Vec<u8>,
    closed: bool,
}

impl RpcProcess {
    pub fn spawn(command: &str, args: &[String], cwd: &PathBuf) -> Result<Self> {
        let mut process = Command::new(command);
        process
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = crate::owned_process::spawn(&mut process)?;
        let input = child.stdin.take().context("app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("app-server stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("app-server stderr unavailable")?;
        #[cfg(unix)]
        {
            crate::process_stream::nonblocking(&input)?;
            crate::process_stream::nonblocking(&stdout)?;
            crate::process_stream::nonblocking(&stderr)?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let readers = vec![
            read_pipe(
                stdout,
                AgentOutputChannel::Stdout,
                sender.clone(),
                stop.clone(),
            ),
            read_pipe(stderr, AgentOutputChannel::Stderr, sender, stop.clone()),
        ];
        Ok(Self {
            child,
            input,
            receiver,
            stop,
            readers,
            buffer: vec![],
            closed: false,
        })
    }
    pub fn send(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value).context("encode native request")?;
        anyhow::ensure!(
            bytes.len() <= 8 * 1024 * 1024,
            "native request frame exceeds 8 MiB"
        );
        bytes.push(b'\n');
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut written = 0;
        while written < bytes.len() {
            anyhow::ensure!(
                Instant::now() < deadline,
                "native stdin stalled; request delivery is unconfirmed"
            );
            match self.input.write(&bytes[written..]) {
                Ok(0) => anyhow::bail!("native stdin closed before request delivery"),
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error).context("write native request"),
            }
        }
        self.input.flush().context("flush native request")
    }
    pub fn receive(&mut self) -> Result<(Vec<Value>, String)> {
        let mut diagnostics = String::new();
        if let Ok(packet) = self.receiver.recv_timeout(Duration::from_millis(10)) {
            self.packet(packet, &mut diagnostics);
        }
        for _ in 0..255 {
            match self.receiver.try_recv() {
                Ok(packet) => self.packet(packet, &mut diagnostics),
                Err(_) => break,
            }
        }
        anyhow::ensure!(
            self.buffer.len() <= 8 * 1024 * 1024,
            "native protocol frame exceeds 8 MiB"
        );
        let mut messages = vec![];
        while let Some(index) = self.buffer.iter().position(|b| *b == b'\n') {
            let line = self.buffer.drain(..=index).collect::<Vec<_>>();
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            messages.push(serde_json::from_slice(&line).context("invalid native JSON-RPC output")?);
        }
        if self.closed && !self.buffer.is_empty() {
            messages.push(
                serde_json::from_slice(&self.buffer)
                    .context("incomplete native JSON-RPC output")?,
            );
            self.buffer.clear();
        }
        if messages.is_empty() && self.closed {
            anyhow::bail!(
                "native app-server disconnected before a terminal turn event: {}",
                diagnostics.trim()
            );
        }
        if messages.is_empty() && self.child.try_wait()?.is_some() && !self.closed {
            // Drain the pipe reader's final packet on the next iteration.
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok((messages, diagnostics))
    }
    fn packet(&mut self, packet: PipePacket, diagnostics: &mut String) {
        if packet.channel == AgentOutputChannel::Stdout {
            self.buffer.extend(packet.bytes);
            self.closed |= packet.eof;
        } else {
            diagnostics.push_str(&String::from_utf8_lossy(&packet.bytes));
        }
    }
}
impl Drop for RpcProcess {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        // OwnedChild terminates this stdio server if it is still alive; no global daemon is touched.
    }
}
