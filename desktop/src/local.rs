//! This user's mihomo-server instance: detecting it, starting it through the
//! installed helper, and installing it with the published installer.
use anyhow::{Context as _, Result, bail, ensure};
use management_client::{Endpoint, ServiceState};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncRead, BufReader},
    process::Command,
};

/// Shared installation root written by the installer.
const INSTALL_ROOT: &str = "/opt/mihomo-server/current";

/// Repository the installer is fetched from (set by release builds).
pub const REPOSITORY: &str = match option_env!("MIHOMO_SERVER_REPOSITORY") {
    Some(repository) => repository,
    None => "oodzchen/mihomo-server",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detected {
    /// `version` is the running service's own version, when it can be read.
    Running {
        endpoint: Endpoint,
        version: Option<String>,
    },
    /// Installed but this user's instance is not running.
    Inactive {
        enabled: bool,
    },
    NotInstalled,
}

fn environment(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn helper() -> PathBuf {
    environment("MIHOMO_SERVER_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(INSTALL_ROOT).join("mihomo-server-user"))
}

/// Classify a systemd reading; `installed` is whether the shared helper exists.
pub fn classify(state: &ServiceState, installed: bool) -> Option<Detected> {
    if state.running() {
        return None;
    }
    Some(if installed || state.load == "loaded" {
        Detected::Inactive {
            enabled: state.enabled == "enabled",
        }
    } else {
        Detected::NotInstalled
    })
}

/// `MIHOMO_SERVER_API`/`MIHOMO_SERVER_TOKEN_FILE` select a service outside
/// systemd (development), as they do for the command line.
pub fn detect() -> Result<Detected> {
    let token_file = environment("MIHOMO_SERVER_TOKEN_FILE").map(PathBuf::from);
    if let Some(api) = environment("MIHOMO_SERVER_API") {
        return management_client::locate(Some(&api), token_file).map(|endpoint| Detected::Running {
            endpoint,
            version: None,
        });
    }
    let state = management_client::service_state()?;
    if let Some(detected) = classify(&state, helper().is_file()) {
        return Ok(detected);
    }
    let mut endpoint = management_client::discover_running(&state)?;
    if let Some(token_file) = token_file {
        endpoint.token_file = token_file;
    }
    Ok(Detected::Running {
        endpoint,
        version: running_version(state.main_pid),
    })
}

/// The service's version from its own binary (the management API reports only
/// the core's). The process may run a release that was since replaced.
fn running_version(pid: u32) -> Option<String> {
    let output = std::process::Command::new(format!("/proc/{pid}/exe"))
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// `mihomo-server 0.1.9` → `v0.1.9`.
fn parse_version(output: &str) -> Option<String> {
    let version = output.strip_prefix("mihomo-server ")?.trim();
    (!version.is_empty() && version.len() <= 32 && !version.contains(char::is_whitespace))
        .then(|| format!("v{}", version.trim_start_matches('v')))
}

/// The token is re-read for every connection: reinstalling can rotate it.
pub fn read_token(endpoint: &Endpoint) -> Result<String> {
    let token = std::fs::read_to_string(&endpoint.token_file)
        .with_context(|| format!("cannot read management token {}", endpoint.token_file.display()))?;
    let token = token.trim();
    ensure!(
        token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid management token in {}",
        endpoint.token_file.display()
    );
    Ok(token.to_owned())
}

/// Run a lifecycle command (`enable`, `start`, `stop`, `restart`) of the
/// installed `mihomo-server-user` helper, which owns the systemd side.
pub async fn run_helper(verb: &str, log: &Log) -> Result<()> {
    let helper = helper();
    log.push(format!("$ {} {verb}", helper.display()));
    let mut command = Command::new(&helper);
    command.arg(verb);
    run_logged(command, log)
        .await
        .with_context(|| format!("mihomo-server-user {verb} failed"))
}

/// One output line without carriage-return progress or terminal escapes.
pub fn clean_line(line: &str) -> String {
    let line = line.rsplit('\r').find(|part| !part.is_empty()).unwrap_or("");
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

async fn pump(reader: impl AsyncRead + Unpin, log: &Log) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = clean_line(&line);
        if !line.trim().is_empty() {
            log.push(line);
        }
    }
}

async fn run_logged(mut command: Command, log: &Log) -> Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("cannot start the command")?;
    let stdout = child.stdout.take().context("stdout")?;
    let stderr = child.stderr.take().context("stderr")?;
    let (status, (), ()) = tokio::join!(child.wait(), pump(stdout, log), pump(stderr, log));
    let status = status?;
    if !status.success() {
        bail!("exited with {status}");
    }
    Ok(())
}

/// A private directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn create() -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mihomo-server-desktop-{}-{nanos}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        // create (not create_all) refuses an existing path planted by someone else.
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn download_installer(url: &str, directory: &Path) -> Result<PathBuf> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("mihomo-server-desktop/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    let script = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("cannot download {url}"))?
        .error_for_status()?
        .bytes()
        .await?;
    ensure!(script.starts_with(b"#!"), "downloaded installer is not a script: {url}");
    let path = directory.join("install.sh");
    std::fs::write(&path, &script)?;
    Ok(path)
}

/// Run the published installer (`MIHOMO_SERVER_INSTALLER`: a path or URL
/// instead). Its root step asks for authorization through polkit.
pub async fn install(log: &Log) -> Result<()> {
    let source = environment("MIHOMO_SERVER_INSTALLER")
        .unwrap_or_else(|| format!("https://github.com/{REPOSITORY}/releases/latest/download/install.sh"));
    install_from(&source, log).await
}

async fn install_from(source: &str, log: &Log) -> Result<()> {
    let directory = TempDir::create()?;
    let script = if source.starts_with("https://") || source.starts_with("http://") {
        log.push(format!("==> downloading {source}"));
        download_installer(source, &directory.0).await?
    } else {
        PathBuf::from(source)
    };
    let mut command = Command::new("bash");
    command.arg(&script).env("MIHOMO_INSTALL_ELEVATE", "pkexec");
    run_logged(command, log).await.context("the installer failed")
}

/// Bounded output of install and service tasks and of failures, shown by the
/// status page as a small terminal. Never cleared: older lines scroll away.
#[derive(Default)]
pub struct Log(Mutex<VecDeque<String>>);

const LOG_LINES: usize = 400;

impl Log {
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<String>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn push(&self, line: String) {
        let mut lines = self.lock();
        if lines.len() == LOG_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub fn lines(&self) -> Vec<String> {
        self.lock().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(load: &str, active: &str, enabled: &str, pid: u32) -> ServiceState {
        ServiceState {
            load: load.into(),
            active: active.into(),
            sub: String::new(),
            enabled: enabled.into(),
            main_pid: pid,
        }
    }

    #[test]
    fn systemd_state_classifies_installation() {
        assert_eq!(classify(&state("loaded", "active", "enabled", 7), true), None);
        assert_eq!(
            classify(&state("loaded", "inactive", "enabled", 0), true),
            Some(Detected::Inactive { enabled: true })
        );
        assert_eq!(
            classify(&state("loaded", "failed", "disabled", 0), false),
            Some(Detected::Inactive { enabled: false })
        );
        assert_eq!(
            classify(&state("not-found", "inactive", "", 0), false),
            Some(Detected::NotInstalled)
        );
        // Program installed by root, instance never enabled by this user.
        assert_eq!(
            classify(&state("not-found", "inactive", "", 0), true),
            Some(Detected::Inactive { enabled: false })
        );
    }

    #[test]
    fn service_versions_come_from_the_version_flag() {
        assert_eq!(parse_version("mihomo-server 0.1.9\n").as_deref(), Some("v0.1.9"));
        assert_eq!(parse_version("other 0.1.9\n"), None);
        assert_eq!(parse_version("mihomo-server \n"), None);
    }

    #[test]
    fn output_lines_drop_progress_and_escapes() {
        assert_eq!(clean_line("\u{1b}[1;32m==> done\u{1b}[0m"), "==> done");
        assert_eq!(clean_line("#  10%\r###  50%\r##### 100%"), "##### 100%");
        assert_eq!(clean_line("a\u{7}b\tc"), "ab\tc");
    }

    #[test]
    fn log_keeps_the_latest_lines() {
        let log = Log::default();
        for index in 0..450 {
            log.push(index.to_string());
        }
        let lines = log.lines();
        assert_eq!((lines.len(), lines[0].as_str()), (400, "50"));
    }

    #[test]
    fn installer_runs_elevated_by_polkit_and_streams_output() {
        let directory = TempDir::create().unwrap();
        let script = directory.0.join("install.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"elevate=$MIHOMO_INSTALL_ELEVATE\"\nprintf '\\033[32mok\\033[0m\\n'\necho warn >&2\nexit ${FAIL:-0}\n",
        )
        .unwrap();
        let log = Log::default();
        tauri::async_runtime::block_on(install_from(script.to_str().unwrap(), &log)).unwrap();
        let mut lines = log.lines();
        lines.sort();
        assert_eq!(lines, ["elevate=pkexec", "ok", "warn"]);

        std::fs::write(&script, "#!/bin/sh\necho broken\nexit 3\n").unwrap();
        let error = tauri::async_runtime::block_on(install_from(script.to_str().unwrap(), &log)).unwrap_err();
        assert!(format!("{error:#}").contains("the installer failed"), "{error:#}");
    }

    #[test]
    fn tokens_must_be_private_hex() {
        let directory = TempDir::create().unwrap();
        let file = directory.0.join("token");
        let endpoint = Endpoint::explicit("http://127.0.0.1:9090", file.clone()).unwrap();
        std::fs::write(&file, format!("{}\n", "a".repeat(64))).unwrap();
        assert_eq!(read_token(&endpoint).unwrap(), "a".repeat(64));
        std::fs::write(&file, "short").unwrap();
        assert!(read_token(&endpoint).is_err());
    }
}
