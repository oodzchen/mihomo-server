use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Command,
    sync::watch,
    task::JoinHandle,
    time::timeout,
};

const OUTPUT_LIMIT: usize = 64 * 1024;

pub(crate) fn protected_paths(data_dir: &Path, config: &Path, binary: &Path) -> Vec<PathBuf> {
    let mut paths = vec![config.to_path_buf(), binary.to_path_buf()];
    if let Some(parent) = binary
        .parent()
        .filter(|parent| parent.starts_with(data_dir) && *parent != data_dir)
    {
        paths.push(parent.to_path_buf());
    }
    paths
}

pub(crate) async fn resource_paths(data_dir: &Path, config: &Path, binary: &Path) -> Result<()> {
    let contents: serde_yaml_ng::Mapping = serde_yaml_ng::from_slice(&tokio::fs::read(config).await?)?;
    headless_core::config::resource_paths::validate_owned(
        &contents,
        data_dir,
        &protected_paths(data_dir, config, binary),
    )
}

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
    resource_paths(data_dir, config, binary).await?;
    let unmarked = unmarked_copy(config).await?;
    let mut command = Command::new(binary);
    crate::shutdown::bind_child_lifetime(&mut command);
    let mut child = command
        .arg("-t")
        .arg("-d")
        .arg(data_dir)
        .arg("-f")
        .arg(unmarked.as_ref().map_or(config, |copy| copy.file.as_path()))
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

/// A private copy of a configuration, removed with its directory on drop.
struct CheckCopy {
    directory: PathBuf,
    file: PathBuf,
}

impl Drop for CheckCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// An unprivileged validator cannot mark its sockets (SO_MARK needs
/// CAP_NET_ADMIN, which the TUN launcher grants only the running core), so
/// with `routing-mark` every dial fails, including the downloads the check
/// makes for missing Geo databases. It then checks a copy without the mark,
/// which changes nothing but how sockets are created. Relative paths resolve
/// against `-d`, not the file, so the copy may live elsewhere.
async fn unmarked_copy(config: &Path) -> Result<Option<CheckCopy>> {
    let mut contents: serde_yaml_ng::Mapping = serde_yaml_ng::from_slice(&tokio::fs::read(config).await?)?;
    if contents.remove("routing-mark").is_none() || crate::native_tun::has_net_admin_capability(None) {
        return Ok(None);
    }
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).context("generate validation directory name")?;
    let directory = std::env::temp_dir().join(format!("ms-check-{}", crate::secure_fs::hex(&random)));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&directory)?;
    let copy = CheckCopy {
        file: directory.join("config.yaml"),
        directory,
    };
    let bytes = serde_yaml_ng::to_string(&contents)?;
    std::io::Write::write_all(&mut crate::secure_fs::create_private(&copy.file)?, bytes.as_bytes())?;
    Ok(Some(copy))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unprivileged_checks_drop_only_the_routing_mark() -> Result<()> {
        let directory = std::env::temp_dir().join(format!("ms-validation-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory)?;
        let marked = directory.join("marked.yaml");
        std::fs::write(&marked, "routing-mark: 196608\nmixed-port: 7890\n")?;
        let plain = directory.join("plain.yaml");
        std::fs::write(&plain, "mixed-port: 7890\n")?;
        let result = async {
            assert!(unmarked_copy(&plain).await?.is_none());
            let copy = unmarked_copy(&marked).await?;
            if crate::native_tun::has_net_admin_capability(None) {
                assert!(copy.is_none());
                return Ok(());
            }
            let copy = copy.context("unprivileged check keeps the mark")?;
            let contents: serde_yaml_ng::Mapping = serde_yaml_ng::from_slice(&std::fs::read(&copy.file)?)?;
            assert_eq!(
                contents,
                serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>("mixed-port: 7890")?
            );
            let copied = copy.directory.clone();
            drop(copy);
            assert!(!copied.exists());
            Ok(())
        }
        .await;
        std::fs::remove_dir_all(&directory)?;
        result
    }
}
