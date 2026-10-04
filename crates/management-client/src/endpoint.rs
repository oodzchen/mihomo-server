//! Find a running instance: its address, accepted Host and token file.
use anyhow::{Context as _, Result, bail};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Command,
};
use url::Url;

pub const UNIT: &str = "mihomo-server";
const SLOT_REGISTRY: &str = "/var/lib/mihomo-server/slots";

/// Where the management API answers and which Host it accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Address actually connected to; wildcard listeners are reached via loopback.
    pub base: String,
    /// The authority the service authorizes (its public origin, else its listener).
    pub host: String,
    /// Browser address: the public origin when configured.
    pub management_url: String,
    pub token_file: PathBuf,
}

impl Endpoint {
    pub fn new(listen: SocketAddr, public_origin: Option<&str>, token_file: PathBuf) -> Result<Self> {
        let mut connect = listen;
        if listen.ip().is_unspecified() {
            connect.set_ip(match listen.ip() {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            });
        }
        let base = format!("http://{connect}");
        let (host, management_url) = match public_origin.filter(|origin| !origin.is_empty()) {
            Some(origin) => {
                let origin = Url::parse(origin)
                    .with_context(|| format!("invalid public origin {origin}"))?
                    .origin()
                    .ascii_serialization();
                let host = origin.split_once("://").context("invalid public origin")?.1.to_owned();
                (host, origin)
            }
            None => (listen.to_string(), base.clone()),
        };
        Ok(Self {
            base,
            host,
            management_url,
            token_file,
        })
    }

    /// An explicitly named API (`--api`), e.g. a foreground or remote service.
    pub fn explicit(api: &str, token_file: PathBuf) -> Result<Self> {
        let url = Url::parse(api).with_context(|| format!("invalid API address {api}"))?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            "API address must be an http(s) URL"
        );
        let origin = url.origin().ascii_serialization();
        Ok(Self {
            host: origin.split_once("://").context("invalid API address")?.1.to_owned(),
            base: origin.clone(),
            management_url: origin,
            token_file,
        })
    }
}

/// The value of `--key VALUE` or `--key=VALUE` in a process's arguments.
pub fn argument<'a>(arguments: &'a [String], key: &str) -> Option<&'a str> {
    let mut iter = arguments.iter();
    while let Some(argument) = iter.next() {
        if argument == key {
            return iter.next().map(String::as_str);
        }
        if let Some(value) = argument.strip_prefix(key).and_then(|rest| rest.strip_prefix('=')) {
            return Some(value);
        }
    }
    None
}

/// Same port plan as the service (`Isolation::management_port`).
pub fn management_port(slot: u16) -> u16 {
    if slot == 0 { 9090 } else { 20000 + 10 * slot }
}

/// Reconstruct the endpoint from the service's own command line, as the
/// launcher always passes explicit listener, origin and data directory values.
pub fn from_arguments(arguments: &[String], cwd: &Path, slot: impl FnOnce() -> Option<u16>) -> Result<Endpoint> {
    let listen = match argument(arguments, "--listen") {
        Some(listen) => listen.parse().with_context(|| format!("invalid --listen {listen}"))?,
        None if arguments.iter().any(|argument| argument == "--multi-user") => {
            let slot = match argument(arguments, "--slot") {
                Some(slot) => slot.parse().context("invalid --slot")?,
                None => slot().context("cannot find this user's slot claim")?,
            };
            SocketAddr::from((Ipv4Addr::LOCALHOST, management_port(slot)))
        }
        None => SocketAddr::from((Ipv4Addr::LOCALHOST, 9090)),
    };
    let data_dir = cwd.join(argument(arguments, "--data-dir").unwrap_or("data"));
    Endpoint::new(
        listen,
        argument(arguments, "--public-origin"),
        data_dir.join("management-token"),
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceState {
    /// `loaded` once the unit file is installed; `not-found` before.
    pub load: String,
    pub active: String,
    pub sub: String,
    pub enabled: String,
    pub main_pid: u32,
}

impl ServiceState {
    pub fn running(&self) -> bool {
        self.active == "active" && self.main_pid != 0
    }
}

pub fn parse_show(output: &str) -> ServiceState {
    let mut state = ServiceState::default();
    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "LoadState" => state.load = value.to_owned(),
            "ActiveState" => state.active = value.to_owned(),
            "SubState" => state.sub = value.to_owned(),
            "UnitFileState" => state.enabled = value.to_owned(),
            "MainPID" => state.main_pid = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    state
}

/// This user's systemd instance of the shared unit.
pub fn service_state() -> Result<ServiceState> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            UNIT,
            "--property=LoadState,ActiveState,SubState,UnitFileState,MainPID",
        ])
        .output()
        .context("cannot run systemctl; use --api for a service outside systemd")?;
    if !output.status.success() {
        bail!(
            "cannot query your systemd user manager: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_show(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(target_os = "linux")]
fn registered_slot() -> Option<u16> {
    use std::os::unix::fs::MetadataExt as _;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    std::fs::read_dir(SLOT_REGISTRY)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .metadata()
                .is_ok_and(|metadata| metadata.is_file() && metadata.uid() == uid)
        })
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .min()
}

#[cfg(not(target_os = "linux"))]
fn registered_slot() -> Option<u16> {
    None
}

pub fn discover_running(state: &ServiceState) -> Result<Endpoint> {
    if !state.running() {
        bail!(
            "your mihomo-server instance is not running ({}); start it with: mihomo-server start",
            if state.active.is_empty() {
                "unknown"
            } else {
                &state.active
            }
        );
    }
    let proc = PathBuf::from(format!("/proc/{}", state.main_pid));
    let raw = std::fs::read(proc.join("cmdline")).context("cannot read the service command line")?;
    let arguments: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    let cwd = std::fs::read_link(proc.join("cwd")).unwrap_or_else(|_| PathBuf::from("/"));
    from_arguments(&arguments, &cwd, registered_slot)
}

/// `$XDG_DATA_HOME/mihomo-server/management-token`, as the launcher defaults.
pub fn default_token_file() -> PathBuf {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data_home.join("mihomo-server/management-token")
}

/// The instance a client manages: an explicit API address, else this user's
/// systemd instance. `token_file` overrides where the token is read.
pub fn locate(api: Option<&str>, token_file: Option<PathBuf>) -> Result<Endpoint> {
    if let Some(api) = api {
        return Endpoint::explicit(api, token_file.unwrap_or_else(default_token_file));
    }
    let mut endpoint = discover_running(&service_state()?)?;
    if let Some(token) = token_file {
        endpoint.token_file = token;
    }
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split(' ').map(String::from).collect()
    }

    #[test]
    fn launcher_command_line_yields_slot_address_and_token() {
        let endpoint = from_arguments(
            &args("/opt/mihomo-server/current/bin/mihomo-server --resource-dir /r --data-dir /home/a/.local/share/mihomo-server --multi-user"),
            Path::new("/"),
            || Some(5),
        )
        .unwrap();
        assert_eq!(endpoint.base, "http://127.0.0.1:20050");
        assert_eq!(endpoint.host, "127.0.0.1:20050");
        assert_eq!(
            endpoint.token_file,
            Path::new("/home/a/.local/share/mihomo-server/management-token")
        );
        let first = from_arguments(&args("x --multi-user --data-dir=/d"), Path::new("/"), || Some(0)).unwrap();
        assert_eq!(first.base, "http://127.0.0.1:9090");
        let fixed = from_arguments(&args("x --multi-user --slot 2 --data-dir /d"), Path::new("/"), || None).unwrap();
        assert_eq!(fixed.base, "http://127.0.0.1:20020");
        assert!(from_arguments(&args("x --multi-user --data-dir /d"), Path::new("/"), || None).is_err());
    }

    #[test]
    fn wildcard_listener_uses_loopback_and_public_origin_host() {
        let endpoint = from_arguments(
            &args("x --data-dir d --listen 0.0.0.0:9191 --public-origin https://proxy.example/"),
            Path::new("/srv"),
            || None,
        )
        .unwrap();
        assert_eq!(endpoint.base, "http://127.0.0.1:9191");
        assert_eq!(endpoint.host, "proxy.example");
        assert_eq!(endpoint.management_url, "https://proxy.example");
        assert_eq!(endpoint.token_file, Path::new("/srv/d/management-token"));
        let v6 = Endpoint::new("[::]:9090".parse().unwrap(), Some("http://[::1]:9090"), "t".into()).unwrap();
        assert_eq!(
            (v6.base.as_str(), v6.host.as_str()),
            ("http://[::1]:9090", "[::1]:9090")
        );
    }

    #[test]
    fn explicit_api_and_systemctl_output() {
        let endpoint = Endpoint::explicit("http://127.0.0.1:20050/", "t".into()).unwrap();
        assert_eq!(
            (endpoint.base.as_str(), endpoint.host.as_str()),
            ("http://127.0.0.1:20050", "127.0.0.1:20050")
        );
        assert!(Endpoint::explicit("ftp://x", "t".into()).is_err());
        let state =
            parse_show("LoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nMainPID=42\n");
        assert!(state.running());
        assert_eq!(state.load, "loaded");
        assert_eq!(state.enabled, "enabled");
        assert!(!parse_show("ActiveState=inactive\nMainPID=0").running());
    }
}
