use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context as _, Result, bail, ensure};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Command,
    sync::watch,
    task::JoinHandle,
    time::timeout,
};

const OUTPUT_LIMIT: usize = 64 * 1024;

pub(crate) async fn validate(
    binary: &Path,
    data_dir: &Path,
    config: &Path,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Duration,
) -> Result<()> {
    ensure!(!*shutdown.borrow(), "validation cancelled during shutdown");
    let mut child = Command::new(binary)
        .arg("-t")
        .arg("-d")
        .arg(data_dir)
        .arg("-f")
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn configuration validator")?;
    let mut stdout = capture(child.stdout.take().context("validator stdout unavailable")?);
    let mut stderr = capture(child.stderr.take().context("validator stderr unavailable")?);
    let outcome = tokio::select! {
        biased;
        _ = shutdown.changed() => Err(anyhow::anyhow!("validation cancelled during shutdown")),
        result = timeout(deadline, child.wait()) => result.context("Mihomo validation timeout").and_then(|result| result.map_err(Into::into)),
    };
    let outcome = match outcome {
        Ok(exit) => Ok(exit),
        Err(error) => {
            let cleanup = timeout(Duration::from_secs(5), child.kill()).await;
            match cleanup {
                Ok(Ok(())) => Err(error),
                Ok(Err(cleanup)) => Err(error.context(format!("validator cleanup failed: {cleanup}"))),
                Err(cleanup) => Err(error.context(format!("validator reap timeout: {cleanup}"))),
            }
        }
    };
    let output = collect(&mut stdout).await;
    let errors = collect(&mut stderr).await;
    let exit = outcome?;
    let output = String::from_utf8_lossy(&output?).into_owned();
    let errors = String::from_utf8_lossy(&errors?).into_owned();
    let fatal = ["FATA", "fatal", "Parse config error", "level=fatal"]
        .iter()
        .any(|word| errors.contains(word));
    if !exit.success() || fatal {
        bail!(
            "Mihomo rejected configuration ({exit}): {}",
            if output.is_empty() { errors } else { output }
        );
    }
    Ok(())
}

fn capture<R: AsyncRead + Unpin + Send + 'static>(mut reader: R) -> JoinHandle<Result<Vec<u8>>> {
    tokio::spawn(async move {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        let mut exceeded = false;
        loop {
            let count = reader.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            let remaining = OUTPUT_LIMIT - output.len();
            output.extend_from_slice(&buffer[..count.min(remaining)]);
            exceeded |= count > remaining;
        }
        ensure!(!exceeded, "validator output exceeds 64 KiB per stream");
        Ok(output)
    })
}

async fn collect(reader: &mut JoinHandle<Result<Vec<u8>>>) -> Result<Vec<u8>> {
    match timeout(Duration::from_secs(1), &mut *reader).await {
        Ok(result) => result?,
        Err(error) => {
            reader.abort();
            let _ = reader.await;
            Err(error).context("validator output did not close")
        }
    }
}
