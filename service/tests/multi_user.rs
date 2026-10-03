#![cfg(target_os = "linux")]
//! Multi-user mode with a real core: the shared bundle core runs in place and
//! every committed runtime is moved into the user's slot.
use anyhow::{Context as _, Result, ensure};
use headless_core::{
    config::settings::{RuntimeSettings, TunSettings},
    enhance::isolation::{Isolation, SLOTS},
};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions, CorePhase},
    resources::{Resources, TARGET},
};
use ring::digest::{SHA256, digest};
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

const SUBSCRIPTION: &str = "
mixed-port: 7890
socks-port: 7891
mode: rule
dns:
  enable: true
  listen: 0.0.0.0:1053
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
  nameserver: [223.5.5.5]
tun:
  enable: false
  device: Meta
  auto-route: true
proxies: []
proxy-groups:
  - {name: Main, type: select, proxies: [DIRECT]}
rules: ['MATCH,Main']
";

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        // Short: the core controller socket lives under <data>/run.
        let path = std::env::temp_dir().join(format!("msmu-{stamp:x}"));
        let core = fs::read(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?)?;
        fs::create_dir_all(path.join("bundle/core"))?;
        fs::write(path.join("bundle/core/verge-mihomo"), &core)?;
        fs::set_permissions(path.join("bundle/core/verge-mihomo"), fs::Permissions::from_mode(0o755))?;
        let hash: String = digest(&SHA256, &core)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fs::write(
            path.join("bundle/manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "target": TARGET, "core": {"version": "v1.19.31", "sha256": hash}
            }))?,
        )?;
        fs::write(
            path.join("bundle/minimal.yaml"),
            include_str!("../../examples/minimal.yaml"),
        )?;
        fs::write(path.join("subscription.yaml"), SUBSCRIPTION)?;
        fs::create_dir(path.join("data"))?;
        Ok(Self(path))
    }

    fn options(&self, isolation: Option<Isolation>, bootstrap: &Path) -> Result<CoreOptions> {
        // The single-user phase runs the same binary externally, so it never
        // creates a managed copy under <data>/core.
        let binary = self.0.join("bundle/core/verge-mihomo");
        let mut options = CoreOptions::new(binary, self.0.join("data"), bootstrap.into());
        if isolation.is_some() {
            options.resources = Some(Resources::open(&self.0.join("bundle"))?);
            options.isolation = isolation;
        }
        options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
        Ok(options)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// A slot whose ports are free on this host.
fn isolation() -> Result<Isolation> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    for slot in (0..SLOTS).rev() {
        let isolation = Isolation::new(uid, slot)?;
        if [
            isolation.management_port(),
            isolation.mixed_port(),
            isolation.dns_listen().rsplit(':').next().unwrap().parse()?,
        ]
        .into_iter()
        .all(free)
        {
            return Ok(isolation);
        }
    }
    anyhow::bail!("no free multi-user slot ports")
}

async fn mixed_port(manager: &CoreManager) -> Result<u64> {
    manager.runtime_config().await?["mixed-port"]
        .as_u64()
        .context("runtime mixed-port missing")
}

#[tokio::test]
#[ignore = "requires real Mihomo binary and local sockets"]
async fn shared_core_restages_old_runtime_and_isolates_subscriptions() -> Result<()> {
    let directory = Directory::new()?;
    let isolation = isolation()?;

    // A single-user installation commits a runtime on an ordinary port.
    let single = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let bootstrap = directory.0.join("bootstrap.yaml");
    fs::write(
        &bootstrap,
        format!("mixed-port: {single}\nmode: direct\nrules: ['MATCH,DIRECT']\n"),
    )?;
    let manager = CoreManager::spawn(directory.options(None, &bootstrap)?)?;
    manager.start().await?;
    assert_eq!(mixed_port(&manager).await?, u64::from(single));
    let single_revision = manager.status().config_revision;
    manager.shutdown().await?;

    // Multi-user mode runs the bundle core in place and re-stages that runtime.
    let manager = CoreManager::spawn(directory.options(Some(isolation), &bootstrap)?)?;
    let user = manager.multi_user().context("multi-user state")?.clone();
    assert_eq!((user.slot, user.mixed_port), (isolation.slot(), isolation.mixed_port()));
    assert!(!user.tun_capable);
    let status = manager.start().await?;
    assert_eq!(status.phase, CorePhase::Running);
    assert_ne!(status.config_revision, single_revision);
    assert_eq!(mixed_port(&manager).await?, u64::from(isolation.mixed_port()));
    ensure!(!free(isolation.mixed_port()), "slot mixed port is not listening");
    let error = manager.core_installation().await.unwrap_err();
    assert!(format!("{error:#}").contains("system administrator"), "{error:#}");

    // A subscription's fixed listeners and TUN routing move into the slot.
    let profile = manager
        .import_profile(directory.0.join("subscription.yaml"), Some("fixed ports".into()))
        .await?;
    manager
        .select_profile(profile.uid.context("profile UID")?.to_string())
        .await?;
    let runtime = manager.runtime_config().await?;
    assert_eq!(runtime["mixed-port"].as_u64(), Some(u64::from(isolation.mixed_port())));
    assert!(runtime.get("socks-port").is_none());
    assert_eq!(runtime["dns"]["listen"].as_str(), Some(isolation.dns_listen().as_str()));
    assert_eq!(
        runtime["dns"]["fake-ip-range"].as_str(),
        Some(isolation.fake_ip_range().as_str())
    );
    assert_eq!(runtime["tun"]["device"].as_str(), Some(isolation.tun_device().as_str()));
    assert_eq!(
        runtime["tun"]["include-uid"][0].as_u64(),
        Some(u64::from(isolation.uid()))
    );
    assert_eq!(
        runtime["tun"]["iproute2-rule-index"].as_u64(),
        Some(u64::from(isolation.rule_index()))
    );
    ensure!(
        !free(isolation.mixed_port()),
        "slot mixed port is not listening after reload"
    );

    // Without the TUN group's capable core, enabling TUN fails before reload.
    let revision = manager.status().config_revision;
    let error = manager
        .set_settings(RuntimeSettings {
            tun: Some(TunSettings {
                enable: Some(true),
                ..TunSettings::default()
            }),
            ..RuntimeSettings::default()
        })
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("TUN group"), "{error:#}");
    assert_eq!(manager.status().phase, CorePhase::Running);
    assert_eq!(manager.status().config_revision, revision);

    // An explicitly chosen settings-page port is kept.
    let explicit = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    manager
        .set_settings(RuntimeSettings {
            mixed_port: Some(explicit),
            ..RuntimeSettings::default()
        })
        .await?;
    assert_eq!(mixed_port(&manager).await?, u64::from(explicit));
    manager.shutdown().await?;
    assert!(
        !directory.0.join("data/core").exists(),
        "shared core must not be copied"
    );
    Ok(())
}
