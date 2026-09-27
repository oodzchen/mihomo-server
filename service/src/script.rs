//! Disposable Boa worker: bounded IPC, Linux resource limits, cancellation and reaping.
use anyhow::{Context as _, Result, ensure};
use headless_core::enhance::script::{MAX_IPC_BYTES, ScriptRequest, ScriptResponse, evaluate};
use std::{
    io::{Read, Write},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    sync::watch,
};

pub fn worker_stdio() -> Result<()> {
    ensure!(
        cfg!(target_os = "linux"),
        "script worker resource limits are supported on Linux only"
    );
    #[cfg(target_os = "linux")]
    for (resource, limit) in [
        (libc::RLIMIT_AS, 512 * 1024 * 1024),
        (libc::RLIMIT_CPU, 5),
        (libc::RLIMIT_CORE, 0),
    ] {
        let limit = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        // SAFETY: valid resource constants and a live rlimit pointer in this worker only.
        ensure!(
            unsafe { libc::setrlimit(resource, &limit) } == 0,
            "script worker resource limit failed"
        );
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_IPC_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_IPC_BYTES, "script request exceeds IPC limit");
    let response = evaluate(serde_json::from_slice(&bytes)?);
    let bytes = serde_json::to_vec(&response)?;
    ensure!(bytes.len() <= MAX_IPC_BYTES, "script response exceeds IPC limit");
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}

async fn read_bounded(pipe: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.take((limit + 1) as u64).read_to_end(&mut bytes).await?;
    ensure!(bytes.len() <= limit, "script worker output exceeds IPC limit");
    Ok(bytes)
}

async fn closing(shutdown: &mut watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
}

pub async fn execute(
    binary: &Path,
    request: ScriptRequest,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Duration,
) -> Result<ScriptResponse> {
    ensure!(
        cfg!(target_os = "linux"),
        "script enhancements currently require Linux resource limits"
    );
    ensure!(!*shutdown.borrow(), "script cancelled during shutdown");
    let bytes = serde_json::to_vec(&request)?;
    ensure!(bytes.len() <= MAX_IPC_BYTES, "script request exceeds IPC limit");
    let mut child = Command::new(binary)
        .arg("--script-worker")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("start script worker")?;
    let mut stdin = child.stdin.take().context("script worker stdin missing")?;
    let stdout = child.stdout.take().context("script worker stdout missing")?;
    let stderr = child.stderr.take().context("script worker stderr missing")?;
    let result = {
        let exchange = async {
            let write = async {
                stdin.write_all(&bytes).await?;
                drop(stdin);
                Ok::<_, anyhow::Error>(())
            };
            let ((), stdout, _stderr, status) = tokio::try_join!(
                write,
                read_bounded(stdout, MAX_IPC_BYTES),
                read_bounded(stderr, 16 * 1024),
                async { Ok::<_, anyhow::Error>(child.wait().await?) }
            )?;
            ensure!(
                status.success(),
                "script worker exited unsuccessfully (resource limit or evaluation failure)"
            );
            serde_json::from_slice::<ScriptResponse>(&stdout).context("invalid script worker response")
        };
        tokio::select! {
            biased;
            _ = closing(shutdown) => Err(anyhow::anyhow!("script cancelled during shutdown")),
            result = tokio::time::timeout(deadline, exchange) => result.unwrap_or_else(|_| Err(anyhow::anyhow!("script execution timed out"))),
        }
    };
    if child.try_wait()?.is_none() {
        // Cancellation/timeout must not leave a detached blocking evaluation behind.
        child.kill().await.context("kill script worker")?;
    }
    child.wait().await.context("reap script worker")?;
    result
}
