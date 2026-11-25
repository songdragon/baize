use anyhow::{Context, Result};
use std::{
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

pub(super) struct OwnedChild(Child);
impl std::ops::Deref for OwnedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = stop(&mut self.0);
        }
    }
}
pub(super) fn spawn(command: &mut Command) -> Result<OwnedChild> {
    isolate(command);
    Ok(OwnedChild(
        command.spawn().context("spawn owned provider process")?,
    ))
}

pub(super) fn isolate(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

#[cfg(unix)]
fn signal_group(child: &Child, signal: i32) -> Result<()> {
    let pid = i32::try_from(child.id()).context("invalid child process id")?;
    // The group was created by isolate() for this live child. Never use a saved PID.
    if unsafe { libc::kill(-pid, signal) } == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error).context("signal owned process group");
        }
    }
    Ok(())
}

pub(super) fn stop(child: &mut Child) -> Result<ExitStatus> {
    #[cfg(unix)]
    signal_group(child, libc::SIGTERM)?;
    #[cfg(not(unix))]
    child.kill().context("stop owned process")?;
    let start = Instant::now();
    let mut status = None;
    let mut forced = false;
    loop {
        if status.is_none() {
            status = child.try_wait().context("reap owned process")?;
        }
        #[cfg(unix)]
        {
            if start.elapsed() >= Duration::from_millis(300) && !forced {
                signal_group(child, libc::SIGKILL)?;
                forced = true;
            }
        }
        #[cfg(not(unix))]
        {
            forced = true;
        }
        // Keep the short grace even if the leader exits: its children may ignore TERM.
        if forced {
            if let Some(status) = status {
                return Ok(status);
            }
        }
        if start.elapsed() > Duration::from_secs(2) {
            anyhow::bail!("owned process stop could not be confirmed");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
