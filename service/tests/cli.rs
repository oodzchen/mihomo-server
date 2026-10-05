#![cfg(unix)]
//! The `mihomo-server COMMAND` interface of the service executable.

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

const BINARY: &str = env!("CARGO_BIN_EXE_mihomo-server");

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        // Short: the core socket lives below it and Unix socket paths are limited.
        let dir = Self(std::env::temp_dir().join(format!("ms-cli-{}-{stamp:x}", std::process::id())));
        std::fs::create_dir_all(&dir.0)?;
        Ok(dir)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(arguments: &[&str]) -> Result<Output> {
    Ok(Command::new(BINARY)
        .args(arguments)
        .env_remove("MIHOMO_SERVER_API")
        .env_remove("MIHOMO_SERVER_TOKEN_FILE")
        .stdin(Stdio::null())
        .output()?)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn usage_help_and_version_follow_utility_conventions() -> Result<()> {
    let bare = run(&[])?;
    assert_eq!(bare.status.code(), Some(2));
    assert!(text(&bare.stderr).contains("Usage: mihomo-server"));

    let help = run(&["--help"])?;
    assert!(help.status.success());
    for command in ["status", "sub", "proxy", "mode", "tun", "update", "serve"] {
        assert!(text(&help.stdout).contains(command), "help lacks {command}");
    }
    let version = run(&["--version"])?;
    assert_eq!(
        text(&version.stdout).trim(),
        format!("mihomo-server {}", mihomo_server::VERSION)
    );

    // The service keeps its own options, behind `serve`.
    let service = run(&["serve", "--help"])?;
    assert!(service.status.success());
    assert!(text(&service.stdout).contains("--data-dir"));
    assert!(text(&service.stdout).contains("mihomo-server serve"));

    let unknown = run(&["frobnicate"])?;
    assert_eq!(unknown.status.code(), Some(2));
    Ok(())
}

#[test]
fn operand_and_connection_errors_have_distinct_statuses() -> Result<()> {
    let directory = Directory::new()?;
    let token = directory.0.join("management-token");
    std::fs::write(&token, "a".repeat(64))?;
    let token = token.to_str().context("token path")?;
    // Port 9 (discard) on loopback is closed in test environments.
    let api = "http://127.0.0.1:9";

    let mode = run(&["--api", api, "--token-file", token, "mode", "sideways"])?;
    assert_eq!(mode.status.code(), Some(2));
    assert!(text(&mode.stderr).contains("use rule, global or direct"));

    let status = run(&["--api", api, "--token-file", token, "status"])?;
    assert_eq!(status.status.code(), Some(1));
    assert!(text(&status.stderr).contains("cannot reach the management API"));

    let missing = run(&["--api", api, "--token-file", "/nonexistent/token", "status"])?;
    assert_eq!(missing.status.code(), Some(1));
    assert!(text(&missing.stderr).contains("management token"));
    Ok(())
}

struct Service(Child);
impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: signals only the owned child process.
        unsafe { libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.0.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.kill();
    }
}

fn source(directory: &Path) -> Result<PathBuf> {
    let path = directory.join("source.yaml");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "mixed-port": 0, "mode": "rule", "external-controller": "",
            "dns": {"enable": false}, "tun": {"enable": false},
            "proxy-groups": [
                {"name": "Main", "type": "select", "proxies": ["DIRECT", "REJECT"]},
                {"name": "Other", "type": "select", "proxies": ["REJECT", "DIRECT"]}
            ], "rules": ["MATCH,Main"]
        }))?,
    )?;
    Ok(path)
}

#[test]
#[ignore = "requires a real core (MIHOMO_TEST_BINARY)"]
fn live_commands_control_a_foreground_service() -> Result<()> {
    let core = std::env::var("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
    let directory = Directory::new()?;
    let source = source(&directory.0)?;
    let listen = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    let mut child = Command::new(BINARY)
        .args(["serve", "--mihomo", &core, "--listen", &listen.to_string()])
        .arg("--data-dir")
        .arg(&directory.0)
        .arg("--import-profile")
        .arg(&source)
        .args(["--profile-name", "fixture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().context("stdout")?;
    let service = Service(child);
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = lines.next().context("service exited before readiness")??;
        if serde_json::from_str::<Value>(&line)?["phase"] == "running" {
            break;
        }
    }
    std::thread::spawn(move || lines.for_each(drop));

    let api = format!("http://{listen}");
    let token = directory.0.join("management-token");
    let token = token.to_str().context("token path")?;
    let cli = |arguments: &[&str]| -> Result<String> {
        let mut all = vec!["--api", &api, "--token-file", token];
        all.extend_from_slice(arguments);
        let output = run(&all)?;
        ensure!(
            output.status.success(),
            "{arguments:?} failed: {}",
            text(&output.stderr)
        );
        Ok(text(&output.stdout))
    };

    let status = cli(&["status"])?;
    assert!(status.contains("core:         running"), "{status}");
    assert!(status.contains("subscription: fixture"), "{status}");
    assert!(status.contains("mode:         rule"), "{status}");
    assert!(status.contains("tun:          off"), "{status}");

    let subscriptions = cli(&["sub"])?;
    assert!(
        subscriptions
            .lines()
            .any(|line| line.starts_with('*') && line.ends_with("fixture")),
        "{subscriptions}"
    );

    let groups = cli(&["proxy", "list"])?;
    assert!(groups.contains("Main -> DIRECT"), "{groups}");
    assert!(cli(&["proxy", "select", "rej"])?.contains("Main -> REJECT"));
    let main: Value = serde_json::from_str(&cli(&["--json", "proxy", "list", "Main"])?)?;
    assert_eq!(main["now"], "REJECT");
    assert!(cli(&["proxy", "select", "Other", "2"])?.contains("Other -> DIRECT"));
    let ambiguous = run(&["--api", &api, "--token-file", token, "proxy", "select", "Main", "E"])?;
    assert_eq!(ambiguous.status.code(), Some(1));
    assert!(text(&ambiguous.stderr).contains("matches 2 nodes"));

    cli(&["mode", "global"])?;
    assert_eq!(cli(&["mode"])?.trim(), "global");
    assert!(
        cli(&["proxy", "list"])?
            .lines()
            .any(|line| line.starts_with('*') && line.contains("GLOBAL"))
    );
    cli(&["mode", "direct"])?;
    assert_eq!(cli(&["mode"])?.trim(), "direct");

    // Without systemd, lifecycle commands control the core.
    assert!(cli(&["stop"])?.contains("core: stopped"));
    assert!(cli(&["status"])?.contains("core:         stopped"));
    assert!(cli(&["start"])?.contains("core: running"));
    assert!(cli(&["core"])?.starts_with("Mihomo "));
    drop(service);
    Ok(())
}
