#![cfg(target_os = "linux")]
use anyhow::{Context as _, Result, ensure};
use ring::digest::{SHA256, digest};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    time::{sleep, timeout},
};

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn hash(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct Service {
    child: Child,
    url: String,
    token: String,
    client: reqwest::Client,
}
impl Service {
    async fn start(bundle: &Path, data: &Path, cwd: &Path, listen: &str, config: Option<&Path>) -> Result<Self> {
        let mut command = Command::new(bundle.join("launch"));
        command
            .env("MIHOMO_SERVER_DATA_DIR", data)
            .args(["--listen", listen])
            .current_dir(cwd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(config) = config {
            command.arg("--config").arg(config);
        }
        let child = command.spawn()?;
        let mut service = Self {
            child,
            url: format!("http://{listen}"),
            token: String::new(),
            client: reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?,
        };
        timeout(Duration::from_secs(10), async {
            loop {
                ensure!(
                    service.child.try_wait()?.is_none(),
                    "bundle launcher exited before readiness"
                );
                if let Ok(token) = fs::read_to_string(data.join("management-token")) {
                    service.token = token.trim().to_owned();
                    if service.get("/api/status").await.is_ok() {
                        return Ok::<_, anyhow::Error>(());
                    }
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        Ok(service)
    }
    async fn get(&self, path: &str) -> Result<Value> {
        Ok(self
            .client
            .get(format!("{}{path}", self.url))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    async fn command(&self, command: Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("{}/api/commands", self.url))
            .bearer_auth(&self.token)
            .json(&command)
            .send()
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        ensure!(
            status.is_success(),
            "command {} failed: {status}: {body}",
            command["command"]
        );
        Ok(body)
    }
    async fn phase(&self, phase: &str) -> Result<Value> {
        timeout(Duration::from_secs(10), async {
            loop {
                let status = self.get("/api/status").await?;
                if status["phase"] == phase {
                    return Ok::<_, anyhow::Error>(status);
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    async fn stop(&mut self) -> Result<()> {
        let pid = self.child.id().context("owned service already exited")?;
        // PID belongs to this unreaped test child; launcher must exec the Rust service.
        ensure!(
            unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0,
            "send owned service SIGTERM"
        );
        let status = timeout(Duration::from_secs(15), self.child.wait()).await??;
        ensure!(status.success(), "bundle service did not shut down successfully");
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and local process/socket permissions"]
async fn bundle_launch_repairs_failed_bootstrap_and_retains_state_and_managed_core() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_owned();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = Directory(std::env::temp_dir().join(format!("ms-deploy-{}-{stamp:x}", std::process::id())));
    fs::create_dir_all(directory.0.join("web/assets"))?;
    fs::create_dir(directory.0.join("unrelated-cwd"))?;
    fs::write(
        directory.0.join("web/index.html"),
        "<!doctype html><title>Bundle UI</title>",
    )?;
    fs::write(directory.0.join("web/assets/app.js"), "console.log('local bundle')")?;
    let core =
        PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?).canonicalize()?;
    let version = Command::new(&core).arg("-v").output().await?;
    ensure!(version.status.success(), "read core version");
    let version = String::from_utf8(version.stdout)?;
    let version = version
        .split_whitespace()
        .find(|part| part.starts_with('v'))
        .context("core version absent")?;
    let bundle = directory.0.join("bundle");
    let web = directory.0.join("web");
    let arguments = [
        "--target",
        mihomo_server::resources::TARGET,
        "--core-version",
        version,
        "--core-sha256",
        &hash(&fs::read(&core)?),
        "--mihomo",
        core.to_str().unwrap(),
        "--service",
        env!("CARGO_BIN_EXE_mihomo-server"),
        "--web-dir",
        web.to_str().unwrap(),
        "--output",
        bundle.to_str().unwrap(),
    ];
    let package = Command::new("python3")
        .arg(root.join("scripts/package_bundle.py"))
        .args(arguments)
        .output()
        .await?;
    ensure!(
        package.status.success(),
        "package bundle: {}",
        String::from_utf8_lossy(&package.stderr)
    );
    let checks = Command::new("sha256sum")
        .args(["-c", "checksums.sha256"])
        .current_dir(&bundle)
        .output()
        .await?;
    ensure!(checks.status.success(), "bundle integrity check failed");
    let data = directory.0.join("data");
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let listen = listener.local_addr()?.to_string();
    drop(listener);
    let mut service = Service::start(
        &bundle,
        &data,
        &directory.0.join("unrelated-cwd"),
        &listen,
        Some(&directory.0.join("missing.yaml")),
    )
    .await?;
    let result = async {
        service.phase("failed").await?;
        let page = service.client.get(&service.url).send().await?;
        ensure!(page.status().is_success() && page.text().await?.contains("Bundle UI"), "page unavailable during failed startup");
        let yaml = "mixed-port: 0\nmode: rule\nlog-level: debug\ndns: {enable: false}\ntun: {enable: false}\nprofile: {store-selected: false}\nproxy-groups:\n  - {name: Main, type: select, proxies: [DIRECT, REJECT]}\nrules: ['MATCH,Main']\n";
        let item = service.command(json!({"command":"import_profile", "name":"Deployed", "yaml":yaml})).await?;
        service.command(json!({"command":"select_profile", "uid":item["uid"]})).await?;
        service.command(json!({"command":"start"})).await?;
        service.phase("running").await?;
        service.command(json!({"command":"select_node", "group":"Main", "node":"REJECT"})).await?;
        let previous = service.get("/api/status").await?;
        let invalid = service.client.post(format!("{}/api/commands", service.url)).bearer_auth(&service.token)
            .json(&json!({"command":"edit_config", "yaml":"rules: ['INVALID,DIRECT']\n"})).send().await?;
        ensure!(!invalid.status().is_success(), "invalid configuration accepted");
        let status = service.get("/api/status").await?;
        ensure!(status["config_revision"] == previous["config_revision"] && status["pid"] == previous["pid"], "invalid config changed running state");
        service.command(json!({"command":"edit_config", "yaml":yaml.replace("mode: rule", "mode: direct")})).await?;
        ensure!(service.get("/api/status").await?["active_profile"] == item["uid"], "edit detached current profile");
        service.command(json!({"command":"stop"})).await?;
        service.phase("stopped").await?;
        service.command(json!({"command":"restart"})).await?;
        let status = service.phase("running").await?;
        let child = status["pid"].as_i64().context("owned core PID missing")?;
        let token = service.token.clone();
        service.stop().await?;
        ensure!(unsafe { libc::kill(child as i32, 0) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH), "core was not reaped");
        let managed = data.join("core/verge-mihomo");
        // Simulate an independently replaced core with a different, still executable ELF.
        fs::OpenOptions::new().append(true).open(&managed)?.write_all(b"\nretained-core-upgrade\n")?;
        let managed_hash = hash(&fs::read(&managed)?);
        // A service update replaces the bundle, not its persistent core/data.
        let bundle_updated = directory.0.join("bundle-updated");
        fs::rename(&bundle, &bundle_updated)?;
        service = Service::start(&bundle_updated, &data, &directory.0.join("unrelated-cwd"), &listen, None).await?;
        ensure!(service.token == token, "service restart replaced credential");
        let status = service.phase("running").await?;
        ensure!(status["active_profile"] == item["uid"], "profile was not restored");
        let config = service.get("/api/config").await?;
        ensure!(config["yaml"].as_str().context("config YAML missing")?.contains("mode: direct"), "config not restored");
        timeout(Duration::from_secs(10), async {
            loop {
                let proxies = service.get("/api/proxies").await?;
                if proxies["proxies"]["Main"]["now"] == "REJECT" { break Ok::<_, anyhow::Error>(()); }
                sleep(Duration::from_millis(20)).await;
            }
        }).await??;
        ensure!(hash(&fs::read(managed)?) == managed_hash, "bundle replaced independently upgraded core");
        let logs = service.get("/api/logs").await?;
        ensure!(!logs.as_array().context("logs should be an array")?.is_empty(), "core logs missing");
        let child = status["pid"].as_i64().context("restored core PID missing")?;
        service.stop().await?;
        ensure!(unsafe { libc::kill(child as i32, 0) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH), "restored core was not reaped");
        Ok::<_, anyhow::Error>(())
    }.await;
    if service.child.try_wait()?.is_none() {
        service.stop().await?;
    }
    result
}
