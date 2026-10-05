//! Safe proxy connection summary and verification of core-reported listeners.
use anyhow::{Result, bail, ensure};
use mihomo_client::models::BaseConfig;
use serde_json::{Value, json};
use serde_yaml_ng::Mapping;
use std::time::Duration;
use tokio::time::timeout;

use crate::core_manager::{CoreManager, CorePhase};

pub(crate) fn ports(core: &BaseConfig) -> [(&'static str, u16); 5] {
    [
        ("mixed-port", core.mixed_port),
        ("port", core.port),
        ("socks-port", core.socks_port),
        ("redir-port", core.redir_port),
        ("tproxy-port", core.tproxy_port),
    ]
}

pub(crate) fn verify_ports(config: &Mapping, core: &BaseConfig) -> Result<()> {
    for (field, actual) in ports(core) {
        let configured = config.get(field).and_then(|value| value.as_u64()).unwrap_or(0);
        if configured == u64::from(actual) {
            continue;
        }
        let hint = u16::try_from(configured)
            .ok()
            .filter(|_| actual == 0)
            .and_then(listener_owner)
            .map_or_else(
                || "check port conflicts".to_owned(),
                |owner| format!("port {configured} is already in use by {owner}"),
            );
        bail!("{field} listener mismatch: configured {configured}, core reports {actual}; {hint}");
    }
    Ok(())
}

/// Describes the process listening on a TCP port, so a failed bind names its
/// cause. Sockets of other users' processes are listed but not attributable.
#[cfg(target_os = "linux")]
fn listener_owner(port: u16) -> Option<String> {
    let inodes: Vec<String> = ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|table| listening_inodes(&table, port))
        .collect();
    if inodes.is_empty() {
        return None;
    }
    let owner = std::fs::read_dir("/proc").ok()?.flatten().find_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        let fds = std::fs::read_dir(entry.path().join("fd")).ok()?;
        fds.flatten()
            .filter_map(|fd| std::fs::read_link(fd.path()).ok())
            .any(|target| {
                let target = target.to_string_lossy();
                inodes.iter().any(|inode| target == format!("socket:[{inode}]"))
            })
            .then(|| {
                let name = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
                format!("{} (pid {pid})", name.trim())
            })
    });
    Some(owner.unwrap_or_else(|| "another process".into()))
}

#[cfg(not(target_os = "linux"))]
fn listener_owner(_port: u16) -> Option<String> {
    None
}

/// Socket inodes of `/proc/net/tcp{,6}` rows listening on `port`.
#[cfg(target_os = "linux")]
fn listening_inodes(table: &str, port: u16) -> Vec<String> {
    const LISTEN: &str = "0A";
    table
        .lines()
        .skip(1)
        .filter_map(|row| {
            let fields: Vec<_> = row.split_whitespace().collect();
            let local_port = fields.get(1)?.rsplit_once(':')?.1;
            (u16::from_str_radix(local_port, 16).ok()? == port && *fields.get(3)? == LISTEN)
                .then(|| fields.get(9).map(|inode| (*inode).to_owned()))?
        })
        .collect()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn listening_inodes_match_only_listeners_on_the_port() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1ED2 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0 100 0 0 10 0
   1: 0100007F:1ED2 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 222 1 0 20 4 30 10 -1
   2: 00000000:1ED3 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 333 1 0 100 0 0 10 0";
        assert_eq!(listening_inodes(table, 7890), ["111"]);
        assert!(listening_inodes(table, 7892).is_empty());
    }

    #[test]
    fn listener_owner_names_this_process() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let owner = listener_owner(port).unwrap();
        assert!(owner.ends_with(&format!("(pid {})", std::process::id())), "{owner}");
    }
}

pub(crate) async fn inspect(manager: &CoreManager) -> Result<Value> {
    let before = manager.status();
    let config = if before.config_revision.is_some() {
        manager.runtime_config().await?
    } else {
        Mapping::new()
    };
    let settings = serde_yaml_ng::to_value(manager.settings().await?.runtime)?;
    let running = before.phase == CorePhase::Running;
    let (core, core_error) = if running {
        match timeout(Duration::from_secs(3), manager.client().get_base_config()).await {
            Ok(Ok(core)) => (Some(core), None),
            Ok(Err(error)) => (None, Some(error.to_string())),
            Err(_) => (None, Some("core query timed out".into())),
        }
    } else {
        (None, None)
    };
    let after = manager.status();
    ensure!(
        before.phase == after.phase
            && before.generation == after.generation
            && before.config_revision == after.config_revision,
        "proxy configuration changed during read; retry"
    );
    let reported = core.as_ref().map(ports).unwrap_or_default();
    let ports: Vec<_> = ["mixed-port", "port", "socks-port", "redir-port", "tproxy-port"]
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            json!({
                "key": field,
                "configured": config.get(field).and_then(|value| value.as_u64()).unwrap_or(0),
                "actual": core.as_ref().map(|_| reported[index].1),
                "setting": settings.get(field).and_then(|value| value.as_u64()),
            })
        })
        .collect();
    Ok(json!({
        "running": running,
        "core_error": core_error,
        "has_config": before.config_revision.is_some(),
        "ports": ports,
        "configured": {
            "allow_lan": config.get("allow-lan").and_then(|value| value.as_bool()).unwrap_or(false),
            "bind_address": config.get("bind-address").and_then(|value| value.as_str()).unwrap_or("*"),
            "mode": config.get("mode").and_then(|value| value.as_str()).unwrap_or("rule"),
            "ipv6": config.get("ipv6").and_then(|value| value.as_bool()).unwrap_or(false),
        },
        "reported": core.as_ref().map(|core| json!({
            "allow_lan": core.allow_lan,
            "bind_address": core.bind_address,
            "mode": core.mode,
            "ipv6": core.ipv6,
            "tun_enabled": core.tun.enable,
        })),
        "dns_enabled": config.get("dns").and_then(|value| value.get("enable")).and_then(|value| value.as_bool()).unwrap_or(false),
        "tun_enabled": config.get("tun").and_then(|value| value.get("enable")).and_then(|value| value.as_bool()).unwrap_or(false),
        "authentication_required": core.as_ref().map(|core| core.authentication.as_ref().is_some_and(|values| !values.is_empty())),
        "tun_holder": tun_holder(manager),
    }))
}

/// Who holds the host's one system-wide TUN, if this installation has one.
pub(crate) fn tun_holder(manager: &CoreManager) -> Value {
    match manager.tun_holder() {
        Some((uid, name)) => json!({
            "uid": uid,
            "name": name,
            "self": manager.multi_user().is_some_and(|user| user.uid == uid),
        }),
        None => Value::Null,
    }
}
