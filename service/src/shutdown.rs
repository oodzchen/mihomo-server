use anyhow::Result;
pub use clash_verge_signal::{SHUTDOWN_LATCH, ShutdownLatch, ShutdownOutcome};

#[cfg(unix)]
pub struct ShutdownSignals {
    signals: clash_verge_signal::UnixSignals,
}

#[cfg(unix)]
impl ShutdownSignals {
    pub fn register() -> Result<Self> {
        Ok(Self {
            signals: clash_verge_signal::UnixSignals::new()?,
        })
    }

    pub async fn wait(&mut self) -> &'static str {
        loop {
            let sig = self.signals.recv().await;
            if !SHUTDOWN_LATCH.try_begin() {
                eprintln!("[signal] already shutting down, ignoring repeated {sig}");
                continue;
            }
            return sig;
        }
    }
}

#[cfg(windows)]
pub struct ShutdownSignals {
    interrupt: tokio::signal::windows::CtrlC,
    close: tokio::signal::windows::CtrlClose,
}

#[cfg(windows)]
impl ShutdownSignals {
    pub fn register() -> Result<Self> {
        Ok(Self {
            interrupt: tokio::signal::windows::ctrl_c()?,
            close: tokio::signal::windows::ctrl_close()?,
        })
    }

    pub async fn wait(&mut self) -> &'static str {
        loop {
            let sig = tokio::select! {
                _ = self.interrupt.recv() => "CtrlC",
                _ = self.close.recv() => "CtrlClose",
            };
            if !SHUTDOWN_LATCH.try_begin() {
                eprintln!("[signal] already shutting down, ignoring repeated {sig}");
                continue;
            }
            return sig;
        }
    }
}

/// Emergency parent death must not leave a core/validator running through startup recovery.
pub(crate) fn bind_child_lifetime(command: &mut tokio::process::Command) {
    #[cfg(target_os = "linux")]
    {
        let parent = std::process::id() as libc::pid_t;
        // Only async-signal-safe syscalls run between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = command;
}
