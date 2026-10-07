//! Node tests that reflect what a page open costs, in an isolated Mihomo.
//!
//! The running core's delay API reuses its pooled node sessions and, with
//! `unified-delay`, reports only a second request over an open connection. A
//! page opened after idling also pays for resolving the node, the handshake
//! with it, the remote dial and the target's TLS. The probe loads the same
//! nodes, DNS and dialer options into a fresh, listener-less process, so the
//! first HTTPS request through each node is genuinely cold; the following
//! requests show what every further new connection costs.
use anyhow::{Context as _, Result, bail, ensure};
use futures_util::{StreamExt as _, stream};
use headless_core::config::dns::is_own_listener;
use mihomo_client::{Builder, Mihomo, models::Protocol};
use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::Command,
};

pub const DEFAULT_URL: &str = "https://www.gstatic.com/generate_204";
pub const MAX_NODES: usize = 1024;
/// Concurrent nodes: tiny requests, so large groups finish in a few seconds
/// without the handshakes competing for bandwidth.
pub const PARALLEL: usize = 16;
/// Requests over the session the cold request opened.
const WARM_SAMPLES: u32 = 1;
/// Runtime fields that change how the core dials nodes.
const DIALER_FIELDS: &[&str] = &[
    "ipv6",
    "tcp-concurrent",
    "keep-alive-interval",
    "keep-alive-idle",
    "disable-keep-alive",
    "global-client-fingerprint",
    "interface-name",
    "routing-mark",
    "hosts",
];

/// Milliseconds; 0 when the request failed or timed out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct NodeProbe {
    /// First request through the node: its DNS, handshake, remote dial and TLS.
    pub cold: u32,
    /// A further new connection over the session the first one opened.
    pub warm: u32,
}

pub struct Prober {
    binary: PathBuf,
    data_dir: PathBuf,
    /// One probe process at a time; later tests wait their turn.
    admission: tokio::sync::Semaphore,
}

impl Prober {
    pub fn new(binary: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            binary,
            data_dir,
            admission: tokio::sync::Semaphore::new(1),
        }
    }

    /// Test `nodes` (proxy names of `runtime`) with `url`.
    pub async fn run(
        &self,
        runtime: &Mapping,
        nodes: &[String],
        url: &str,
        timeout_ms: u32,
    ) -> Result<BTreeMap<String, NodeProbe>> {
        ensure!(nodes.len() <= MAX_NODES, "at most {MAX_NODES} nodes per test");
        let target = url::Url::parse(url).context("invalid test URL")?;
        ensure!(
            matches!(target.scheme(), "http" | "https") && target.host_str().is_some(),
            "test URL must be an http or https URL"
        );
        let _permit = self.admission.acquire().await?;
        let interface = default_interface(&std::fs::read_to_string("/proc/net/route").unwrap_or_default());
        let config = probe_config(runtime, &self.data_dir, interface.as_deref())?;
        let directory = ProbeDir::create(&self.data_dir.join("run"))?;
        let config_path = directory.0.join("probe.yaml");
        write_private(&config_path, serde_yaml_ng::to_string(&config)?.as_bytes())?;
        let socket = directory.0.join("s");

        let mut command = Command::new(&self.binary);
        crate::shutdown::bind_child_lifetime(&mut command);
        let mut child = command
            .arg("-d")
            .arg(&directory.0)
            .arg("-f")
            .arg(&config_path)
            .arg("-ext-ctl-unix")
            .arg(&socket)
            // Provider caches stay in the service data directory.
            .env("SAFE_PATHS", &self.data_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start the probe core")?;
        let output = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
        for reader in [
            child
                .stdout
                .take()
                .map(|r| Box::new(r) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            child
                .stderr
                .take()
                .map(|r| Box::new(r) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ]
        .into_iter()
        .flatten()
        {
            let output = Arc::clone(&output);
            tokio::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut output = output.lock();
                    if output.len() == 8 {
                        output.pop_front();
                    }
                    output.push_back(line);
                }
            });
        }

        let client = Builder::new()
            .protocol(Protocol::LocalSocket)
            .socket_path(socket.to_string_lossy())
            .build()?;
        let mut ready = false;
        for _ in 0..50 {
            if let Some(status) = child.try_wait()? {
                bail!(
                    "probe core exited ({status}): {}",
                    output.lock().iter().cloned().collect::<Vec<_>>().join("; ")
                );
            }
            if tokio::time::timeout(Duration::from_millis(500), client.get_version())
                .await
                .is_ok_and(|version| version.is_ok())
            {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ensure!(ready, "probe core did not become ready");

        // The running core keeps its resolver connections open; so does the probe
        // before the first node is measured. A resolver slower than this is part
        // of what the first requests cost.
        if let Some(host) = target.host_str() {
            let _ = tokio::time::timeout(Duration::from_secs(1), warm_resolver(&client, host)).await;
        }
        let results = stream::iter(nodes.iter().cloned())
            .map(|node| {
                let client = &client;
                async move {
                    let result = probe_node(client, &node, url, timeout_ms).await;
                    (node, result)
                }
            })
            .buffer_unordered(PARALLEL)
            .collect::<BTreeMap<_, _>>()
            .await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        Ok(results)
    }
}

async fn warm_resolver(client: &Mihomo, host: &str) -> Result<()> {
    client
        .load_ctx()
        .build_request(reqwest::Method::GET, "/dns/query")?
        .query(&[("name", host), ("type", "A")])
        .send()
        .await?;
    Ok(())
}

async fn probe_node(client: &Mihomo, node: &str, url: &str, timeout_ms: u32) -> NodeProbe {
    let delay = || async {
        client
            .delay_proxy_by_name(node, url, timeout_ms)
            .await
            .map_or(0, |delay| delay.delay)
    };
    let cold = delay().await;
    if cold == 0 {
        return NodeProbe::default();
    }
    let mut total = 0;
    for _ in 0..WARM_SAMPLES {
        match delay().await {
            0 => return NodeProbe { cold, warm: 0 },
            sample => total += sample,
        }
    }
    NodeProbe {
        cold,
        warm: total / WARM_SAMPLES,
    }
}

/// The probe's configuration: the runtime's nodes, providers (read from their
/// caches, never refreshed), resolvers and dialer options, without listeners,
/// TUN, rules, groups or persistent state.
pub(crate) fn probe_config(runtime: &Mapping, data_dir: &Path, interface: Option<&str>) -> Result<Mapping> {
    let mut config = Mapping::new();
    for field in DIALER_FIELDS {
        if let Some(value) = runtime.get(*field) {
            config.insert((*field).into(), value.clone());
        }
    }
    // The core binds node connections to the physical interface so that they
    // bypass a TUN, including another user's system-wide one.
    if !config.contains_key("interface-name")
        && let Some(interface) = interface
    {
        config.insert("interface-name".into(), interface.into());
    }
    for (key, value) in [
        ("mode", Value::from("direct")),
        ("log-level", "warning".into()),
        ("unified-delay", false.into()),
        ("allow-lan", false.into()),
        ("geo-auto-update", false.into()),
    ] {
        config.insert(key.into(), value);
    }
    config.insert(
        "profile".into(),
        serde_yaml_ng::from_str("{store-selected: false, store-fake-ip: false}")?,
    );
    config.insert("tun".into(), serde_yaml_ng::from_str("{enable: false}")?);
    if let Some(dns) = probe_dns(runtime) {
        config.insert("dns".into(), dns.into());
    }
    config.insert(
        "proxies".into(),
        runtime
            .get("proxies")
            .cloned()
            .unwrap_or_else(|| Value::Sequence(vec![])),
    );
    if let Some(providers) = runtime.get("proxy-providers").and_then(Value::as_mapping) {
        config.insert("proxy-providers".into(), cached_providers(providers, data_dir).into());
    }
    config.insert("rules".into(), Value::Sequence(vec!["MATCH,DIRECT".into()]));
    Ok(config)
}

/// The resolvers the core uses for node addresses, as a plain resolver: no
/// listener, fake IPs, policies or resolution through other proxies.
fn probe_dns(runtime: &Mapping) -> Option<Mapping> {
    let dns = runtime.get("dns")?.as_mapping()?;
    if dns.get("enable").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    // The probe has no DNS listener: servers that name the running core's
    // listener are replaced by the upstreams that listener answers from.
    let listen = dns.get("listen").and_then(Value::as_str).unwrap_or_default();
    let own = |server: &str| is_own_listener(server, listen);
    let servers = |key: &str| {
        dns.get(key)
            .and_then(Value::as_sequence)
            .map(|servers| {
                servers
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|server| !server.contains('#'))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let upstreams = servers("nameserver")
        .into_iter()
        .filter(|server| !own(server))
        .collect::<Vec<_>>();
    let resolve = |servers: Vec<String>| {
        let mut resolved = Vec::<Value>::new();
        for server in servers {
            let replacement = if own(&server) { upstreams.clone() } else { vec![server] };
            for server in replacement {
                let server = Value::from(server);
                if !resolved.contains(&server) {
                    resolved.push(server);
                }
            }
        }
        resolved
    };
    let defaults = servers("default-nameserver")
        .into_iter()
        .filter(|server| !own(server))
        .map(Value::from)
        .collect::<Vec<_>>();
    let mut nameservers = resolve(servers("proxy-server-nameserver"));
    if nameservers.is_empty() {
        nameservers = resolve(servers("nameserver"));
    }
    if nameservers.is_empty() {
        nameservers = defaults.clone();
    }
    if nameservers.is_empty() {
        return None;
    }
    let mut probe = Mapping::new();
    probe.insert("enable".into(), true.into());
    probe.insert("enhanced-mode".into(), "normal".into());
    for key in ["ipv6", "use-hosts", "use-system-hosts", "prefer-h3"] {
        if let Some(value) = dns.get(key) {
            probe.insert(key.into(), value.clone());
        }
    }
    if !defaults.is_empty() {
        probe.insert("default-nameserver".into(), defaults.into());
    }
    probe.insert("nameserver".into(), nameservers.into());
    Some(probe)
}

/// Providers read from the caches the running core maintains.
fn cached_providers(providers: &Mapping, data_dir: &Path) -> Mapping {
    let mut cached = Mapping::new();
    for (name, provider) in providers {
        let Some(provider) = provider.as_mapping() else {
            continue;
        };
        let mut provider = provider.clone();
        provider.remove("health-check");
        provider.remove("interval");
        match provider.get("type").and_then(Value::as_str) {
            Some("inline") => {}
            Some("http" | "file") => {
                let Some(path) = provider.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let path = data_dir.join(path);
                if !path.is_file() {
                    continue;
                }
                provider.remove("url");
                provider.remove("proxy");
                provider.remove("header");
                provider.insert("type".into(), "file".into());
                provider.insert("path".into(), path.to_string_lossy().into_owned().into());
            }
            _ => continue,
        }
        cached.insert(name.clone(), provider.into());
    }
    cached
}

/// The interface of the main table's preferred IPv4 default route.
pub(crate) fn default_interface(routes: &str) -> Option<String> {
    routes
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let (interface, destination, flags, metric, mask) = (
                fields.first()?,
                fields.get(1)?,
                fields.get(3)?,
                fields.get(6)?,
                fields.get(7)?,
            );
            let up = u32::from_str_radix(flags, 16).ok()? & 1 == 1;
            (up && *destination == "00000000" && *mask == "00000000")
                .then(|| Some((metric.parse::<u32>().ok()?, (*interface).to_owned())))?
        })
        .min()
        .map(|(_, interface)| interface)
}

struct ProbeDir(PathBuf);

impl ProbeDir {
    /// A short private directory: the controller socket path must stay within
    /// the Unix socket limit.
    fn create(parent: &Path) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        let mut random = [0u8; 4];
        getrandom::fill(&mut random).context("generate probe directory name")?;
        let path = parent.join(format!("p{}", u32::from_ne_bytes(random)));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ProbeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> Mapping {
        serde_yaml_ng::from_str(text).unwrap()
    }

    #[test]
    fn probe_keeps_nodes_and_dialer_but_drops_listeners_tun_and_fake_ip() {
        let runtime = yaml(
            "mixed-port: 7890\nunified-delay: true\ntcp-concurrent: true\nglobal-client-fingerprint: chrome\n\
             tun: {enable: true, stack: mixed}\nproxy-groups: [{name: G, type: url-test, proxies: [a]}]\n\
             rules: ['MATCH,G']\nproxies: [{name: a, type: ss, server: example.com, port: 1, cipher: none, password: p}]\n\
             dns: {enable: true, listen: '0.0.0.0:53', enhanced-mode: fake-ip, fake-ip-range: 198.18.0.1/16, \
             default-nameserver: [223.5.5.5], nameserver: ['https://doh.example/dns-query', 'https://dns.google/dns-query#G'], \
             nameserver-policy: {'geosite:cn': 223.5.5.5}}",
        );
        let probe = probe_config(&runtime, Path::new("/data"), Some("eth0")).unwrap();
        assert_eq!(probe["unified-delay"], Value::from(false));
        assert_eq!(probe["tcp-concurrent"], Value::from(true));
        assert_eq!(probe["global-client-fingerprint"], Value::from("chrome"));
        assert_eq!(probe["interface-name"], Value::from("eth0"));
        assert_eq!(probe["tun"]["enable"], Value::from(false));
        assert_eq!(probe["proxies"], runtime["proxies"]);
        for absent in ["mixed-port", "proxy-groups", "listeners"] {
            assert!(!probe.contains_key(absent), "{absent}");
        }
        assert_eq!(
            probe["dns"],
            Value::from(yaml(
                "enable: true\nenhanced-mode: normal\ndefault-nameserver: [223.5.5.5]\nnameserver: ['https://doh.example/dns-query']"
            ))
        );
    }

    #[test]
    fn probe_prefers_node_resolvers_and_explicit_interface() {
        let runtime =
            yaml("interface-name: wg0\ndns: {nameserver: [1.1.1.1], proxy-server-nameserver: [9.9.9.9]}\nproxies: []");
        let probe = probe_config(&runtime, Path::new("/data"), Some("eth0")).unwrap();
        assert_eq!(probe["interface-name"], Value::from("wg0"));
        assert_eq!(probe["dns"]["nameserver"], Value::from(vec![Value::from("9.9.9.9")]));
        let disabled = probe_config(
            &yaml("dns: {enable: false, nameserver: [1.1.1.1]}"),
            Path::new("/d"),
            None,
        )
        .unwrap();
        assert!(!disabled.contains_key("dns") && !disabled.contains_key("interface-name"));
    }

    #[test]
    fn node_resolution_through_the_core_listener_uses_its_upstreams() {
        let runtime = yaml(
            "dns: {listen: 127.0.0.1:1053, default-nameserver: [223.5.5.5, '127.0.0.1:1053'], \
             nameserver: ['https://doh.example/dns-query', 'udp://127.0.0.1:1053'], \
             proxy-server-nameserver: ['udp://127.0.0.1:1053', 'https://doh.example/dns-query', 1.1.1.1]}",
        );
        let probe = probe_config(&runtime, Path::new("/data"), None).unwrap();
        assert_eq!(
            probe["dns"]["default-nameserver"],
            Value::from(vec![Value::from("223.5.5.5")])
        );
        assert_eq!(
            probe["dns"]["nameserver"],
            Value::from(vec![
                Value::from("https://doh.example/dns-query"),
                Value::from("1.1.1.1")
            ])
        );
    }

    #[test]
    fn providers_are_read_from_their_caches() {
        let data = std::env::temp_dir().join(format!("ms-probe-test-{}", std::process::id()));
        std::fs::create_dir_all(data.join("provider-cache")).unwrap();
        std::fs::write(data.join("provider-cache/a.yaml"), "proxies: []").unwrap();
        let runtime = yaml(
            "proxy-providers:\n  remote: {type: http, url: 'https://x.invalid', path: provider-cache/a.yaml, interval: 3600, \
             health-check: {enable: true, url: 'https://x.invalid'}, filter: HK}\n  missing: {type: http, url: 'https://y.invalid', path: provider-cache/b.yaml}\n  \
             inline: {type: inline, payload: []}",
        );
        let probe = probe_config(&runtime, &data, None).unwrap();
        let providers = probe["proxy-providers"].as_mapping().unwrap();
        assert_eq!(
            providers["remote"],
            Value::from(yaml(&format!(
                "type: file\npath: {}\nfilter: HK",
                data.join("provider-cache/a.yaml").display()
            )))
        );
        assert!(!providers.contains_key("missing"));
        assert!(providers.contains_key("inline"));
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn default_interface_uses_the_lowest_metric_default_route() {
        let routes = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                      wlo1\t00000000\t011FA8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                      eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                      down\t00000000\t0100A8C0\t0002\t0\t0\t1\t00000000\t0\t0\t0\n\
                      wlo1\t001FA8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(default_interface(routes).as_deref(), Some("eth0"));
        assert_eq!(default_interface("Iface\n"), None);
    }
}
