//! Safe proxy connection summary and verification of core-reported listeners.
use anyhow::{Result, ensure};
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
        ensure!(
            configured == u64::from(actual),
            "{field} listener mismatch: configured {configured}, core reports {actual}; check port conflicts"
        );
    }
    Ok(())
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
