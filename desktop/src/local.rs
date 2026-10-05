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
    io::{AsyncRead, AsyncReadExt as _},
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

/// Forwards output to LOG as it arrives. A carriage return ends a progress
/// update (curl's bar), which replaces the previous update in place instead
/// of waiting for the newline that ends the whole bar.
async fn pump(mut reader: impl AsyncRead + Unpin, log: &Log) {
    let mut pending = Vec::new();
    let mut progress = None;
    let mut chunk = [0u8; 4096];
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &chunk[..read] {
            if byte != b'\r' && byte != b'\n' {
                pending.push(byte);
                continue;
            }
            let line = clean_line(&String::from_utf8_lossy(&pending));
            pending.clear();
            if !line.trim().is_empty() {
                let id = match progress {
                    Some(id) => log.replace(id, line),
                    None => log.push(line),
                };
                progress = (byte == b'\r').then_some(id);
            } else if byte == b'\n' {
                progress = None;
            }
        }
    }
    let line = clean_line(&String::from_utf8_lossy(&pending));
    if !line.trim().is_empty() {
        let _ = match progress {
            Some(id) => log.replace(id, line),
            None => log.push(line),
        };
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

/// A temporary proxy for the installer's downloads, normalized; empty is none.
/// `host:port` means an HTTP proxy.
pub fn parse_proxy(input: &str) -> Result<Option<String>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("http://{input}")
    };
    let url = reqwest::Url::parse(&with_scheme).context("not a URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h"),
        "unsupported proxy scheme {}",
        url.scheme()
    );
    ensure!(
        url.host_str().is_some_and(|host| !host.is_empty()),
        "missing proxy host"
    );
    ensure!(url.port_or_known_default().is_some(), "missing proxy port");
    ensure!(
        matches!(url.path(), "" | "/") && url.query().is_none() && url.fragment().is_none(),
        "a proxy address has no path"
    );
    Ok(Some(url.as_str().trim_end_matches('/').to_owned()))
}

/// The proxy without its password, for the output panel.
fn redact(proxy: &str) -> String {
    match reqwest::Url::parse(proxy) {
        Ok(mut url) if url.password().is_some() => {
            let _ = url.set_password(Some("***"));
            url.as_str().trim_end_matches('/').to_owned()
        }
        _ => proxy.to_owned(),
    }
}

/// Without a temporary proxy the environment's proxy settings apply.
fn http_client(proxy: Option<&str>, timeout: Duration) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("mihomo-server-desktop/{}", crate::VERSION))
        .connect_timeout(Duration::from_secs(20))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(5));
    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    Ok(builder.build()?)
}

/// The installer's own downloads (curl or wget) go through PROXY too; the
/// service's local API never does.
fn proxy_environment(command: &mut Command, proxy: &str) {
    for name in [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ] {
        command.env(name, proxy);
    }
    for name in ["no_proxy", "NO_PROXY"] {
        command.env(name, "localhost,127.0.0.1,::1");
    }
}

fn installer_source() -> String {
    environment("MIHOMO_SERVER_INSTALLER")
        .unwrap_or_else(|| format!("https://github.com/{REPOSITORY}/releases/latest/download/install.sh"))
}

/// Download what the installer downloads first, through PROXY (none: the
/// environment's settings); how long the response took.
pub async fn test_proxy(proxy: Option<&str>) -> Result<Duration> {
    let source = installer_source();
    let url = if source.starts_with("https://") || source.starts_with("http://") {
        source
    } else {
        format!("https://github.com/{REPOSITORY}/releases/latest/download/install.sh")
    };
    let started = std::time::Instant::now();
    http_client(proxy, Duration::from_secs(15))?
        .get(&url)
        .send()
        .await
        .map_err(|error| anyhow::anyhow!(describe_request_error(&error)))?
        .error_for_status()?;
    Ok(started.elapsed())
}

/// reqwest's top-level message names only the URL; the cause says why.
fn describe_request_error(error: &reqwest::Error) -> String {
    let mut cause: &dyn std::error::Error = error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    if error.is_timeout() {
        "timed out".into()
    } else {
        cause.to_string()
    }
}

async fn download_installer(url: &str, directory: &Path, proxy: Option<&str>) -> Result<PathBuf> {
    let script = http_client(proxy, Duration::from_secs(120))?
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
/// PROXY is the status page's temporary proxy for the downloads.
pub async fn install(log: &Log, proxy: Option<&str>) -> Result<()> {
    install_from(&installer_source(), log, proxy).await
}

async fn install_from(source: &str, log: &Log, proxy: Option<&str>) -> Result<()> {
    let directory = TempDir::create()?;
    if let Some(proxy) = proxy {
        log.push(format!("==> using temporary proxy {}", redact(proxy)));
    }
    let script = if source.starts_with("https://") || source.starts_with("http://") {
        log.push(format!("==> downloading {source}"));
        download_installer(source, &directory.0, proxy).await?
    } else {
        PathBuf::from(source)
    };
    let mut command = Command::new("bash");
    // Download progress as a bar narrow enough for the output panel.
    command
        .arg(&script)
        .env("MIHOMO_INSTALL_ELEVATE", "pkexec")
        .env("MIHOMO_INSTALL_PROGRESS", "1")
        .env("COLUMNS", "50");
    if let Some(proxy) = proxy {
        proxy_environment(&mut command, proxy);
    }
    run_logged(command, log).await.context("the installer failed")
}

/// Bounded output of install and service tasks and of failures, shown by the
/// status page as a small terminal. Never cleared: older lines scroll away.
#[derive(Default)]
pub struct Log(Mutex<Lines>);

#[derive(Default)]
struct Lines {
    /// Each line with its id, for updates in place.
    lines: VecDeque<(u64, String)>,
    next: u64,
}

const LOG_LINES: usize = 400;

impl Log {
    fn lock(&self) -> std::sync::MutexGuard<'_, Lines> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Returns the line's id.
    pub fn push(&self, line: String) -> u64 {
        let mut log = self.lock();
        if log.lines.len() == LOG_LINES {
            log.lines.pop_front();
        }
        let id = log.next;
        log.next += 1;
        log.lines.push_back((id, line));
        id
    }

    /// Update line ID in place, or append it once it has scrolled away;
    /// returns its id.
    pub fn replace(&self, id: u64, line: String) -> u64 {
        let mut log = self.lock();
        if let Some((_, text)) = log.lines.iter_mut().find(|(existing, _)| *existing == id) {
            *text = line;
            return id;
        }
        drop(log);
        self.push(line)
    }

    pub fn lines(&self) -> Vec<String> {
        self.lock().lines.iter().map(|(_, line)| line.clone()).collect()
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
    fn progress_updates_replace_each_other_and_lines_stream_as_they_arrive() {
        let log = Log::default();
        let output: &[u8] = b"==> downloading x\n\r#=#=#\r##   10.0%\r#### 100.0%\nnext\r\nlast";
        tauri::async_runtime::block_on(pump(output, &log));
        assert_eq!(log.lines(), ["==> downloading x", "#### 100.0%", "next", "last"]);

        // A line is visible before the command writes anything else.
        let (mut writer, reader) = tokio::io::duplex(64);
        let log = std::sync::Arc::new(Log::default());
        let pumping = tauri::async_runtime::spawn({
            let log = log.clone();
            async move { pump(reader, &log).await }
        });
        tauri::async_runtime::block_on(async {
            use tokio::io::AsyncWriteExt as _;
            writer.write_all(b"first\n##  5%\r").await.unwrap();
            for _ in 0..100 {
                if log.lines().len() == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(log.lines(), ["first", "##  5%"]);
            writer.write_all(b"### 50%\r").await.unwrap();
            drop(writer);
            pumping.await.unwrap();
        });
        assert_eq!(log.lines(), ["first", "### 50%"]);
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
        tauri::async_runtime::block_on(install_from(script.to_str().unwrap(), &log, None)).unwrap();
        let mut lines = log.lines();
        lines.sort();
        assert_eq!(lines, ["elevate=pkexec", "ok", "warn"]);

        std::fs::write(&script, "#!/bin/sh\necho broken\nexit 3\n").unwrap();
        let error = tauri::async_runtime::block_on(install_from(script.to_str().unwrap(), &log, None)).unwrap_err();
        assert!(format!("{error:#}").contains("the installer failed"), "{error:#}");
    }

    #[test]
    fn the_temporary_proxy_reaches_the_installer_without_its_password_in_the_log() {
        let directory = TempDir::create().unwrap();
        let script = directory.0.join("install.sh");
        std::fs::write(&script, "#!/bin/sh\necho \"$https_proxy $ALL_PROXY $no_proxy\"\n").unwrap();
        let log = Log::default();
        let proxy = "socks5h://user:secret@127.0.0.1:7891";
        tauri::async_runtime::block_on(install_from(script.to_str().unwrap(), &log, Some(proxy))).unwrap();
        assert_eq!(
            log.lines(),
            [
                "==> using temporary proxy socks5h://user:***@127.0.0.1:7891",
                &format!("{proxy} {proxy} localhost,127.0.0.1,::1"),
            ]
        );
    }

    #[test]
    fn proxy_addresses_are_normalized_or_refused() {
        let parse = |input| parse_proxy(input).ok().flatten();
        assert_eq!(parse_proxy("  ").unwrap(), None);
        assert_eq!(parse("127.0.0.1:7890").as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(
            parse("http://127.0.0.1:7890/").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(parse("socks5://[::1]:7891").as_deref(), Some("socks5://[::1]:7891"));
        assert_eq!(
            parse("socks5h://u:p@proxy.lan:1080").as_deref(),
            Some("socks5h://u:p@proxy.lan:1080")
        );
        for invalid in [
            "ftp://127.0.0.1:21",
            "http://127.0.0.1:7890/path",
            "socks5://127.0.0.1",
            "http://:80",
            "not a proxy",
        ] {
            assert!(parse_proxy(invalid).is_err(), "{invalid}");
        }
    }

    /// `MIHOMO_TEST_PROXY=http://127.0.0.1:7897 cargo test -- --ignored live_proxy`
    #[test]
    #[ignore = "needs network access and a local proxy in MIHOMO_TEST_PROXY"]
    fn live_proxy_test_downloads_the_installer() {
        let proxy = parse_proxy(&std::env::var("MIHOMO_TEST_PROXY").unwrap()).unwrap();
        let elapsed = tauri::async_runtime::block_on(test_proxy(proxy.as_deref())).unwrap();
        assert!(elapsed < Duration::from_secs(15));
        let refused = tauri::async_runtime::block_on(test_proxy(Some("http://127.0.0.1:9"))).unwrap_err();
        assert!(format!("{refused:#}").contains("refused"), "{refused:#}");
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
