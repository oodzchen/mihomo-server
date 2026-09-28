use std::future::Future;
use tokio::signal::unix::{SignalKind, signal};

use crate::{SHUTDOWN_LATCH, ShutdownOutcome};

pub struct UnixSignals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

impl UnixSignals {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }

    pub async fn recv(&mut self) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
            _ = self.hangup.recv() => "SIGHUP",
        }
    }
}

pub fn register<F, Fut>(f: F)
where
    F: Fn(&'static str) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ShutdownOutcome> + Send + 'static,
{
    tokio::spawn(async move {
        let mut signals = match UnixSignals::new() {
            Ok(signals) => signals,
            Err(err) => {
                eprintln!("[signal] failed to register unix signals: {err}");
                return;
            }
        };

        loop {
            let signal_name = signals.recv().await;
            if !SHUTDOWN_LATCH.try_begin() {
                eprintln!("[signal] already shutting down, ignoring repeated signal: {signal_name}");
                continue;
            }
            let outcome = f(signal_name).await;
            SHUTDOWN_LATCH.finish(outcome);
            if outcome == ShutdownOutcome::Committed {
                break;
            }
        }
    });
}
