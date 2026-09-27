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

pub(crate) async fn probe_version(
    binary: &Path,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Duration,
) -> Result<String> {
    ensure!(!*shutdown.borrow(), "core version probe cancelled during shutdown");
    let mut command = Command::new(binary);
    crate::shutdown::bind_child_lifetime(&mut command);
    let mut child = command
        .arg("-v")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("candidate core version probe could not start")?;
    let mut stdout = capture(child.stdout.take().context("candidate stdout unavailable")?);
    let mut stderr = capture(child.stderr.take().context("candidate stderr unavailable")?);
    let result = tokio::select! { biased;
        _ = shutdown.changed() => Err(anyhow::anyhow!("core version probe cancelled during shutdown")),
        exit = timeout(deadline, child.wait()) => exit.context("core version probe timed out").and_then(|exit|exit.map_err(Into::into)),
    };
    let result = match result {
        Ok(exit) => Ok(exit),
        Err(error) => match timeout(Duration::from_secs(5), child.kill()).await {
            Ok(Ok(())) => Err(error),
            Ok(Err(cleanup)) => Err(error.context(format!("candidate probe cleanup failed: {cleanup}"))),
            Err(cleanup) => Err(error.context(format!("candidate probe reap timed out: {cleanup}"))),
        },
    };
    let output = collect(&mut stdout).await;
    let errors = collect(&mut stderr).await;
    ensure!(result?.success(), "candidate core version probe failed");
    errors?;
    let output = String::from_utf8(output?).context("invalid candidate version encoding")?;
    let mut words = output.split_whitespace();
    ensure!(
        words.next() == Some("Mihomo") && words.next() == Some("Meta"),
        "unexpected candidate version format"
    );
    Ok(words.next().context("candidate core version missing")?.to_owned())
}

pub(crate) async fn validate(
    binary: &Path,
    data_dir: &Path,
    config: &Path,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Duration,
) -> Result<()> {
    ensure!(!*shutdown.borrow(), "validation cancelled during shutdown");
    let mut command = Command::new(binary);
    crate::shutdown::bind_child_lifetime(&mut command);
    let mut child = command
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
