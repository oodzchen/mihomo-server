//! Per-user network isolation for one shared, multi-user installation.
//!
//! Each user owns one slot. The slot selects private listener ports, a TUN
//! device, policy-routing indexes and fake-IP ranges, so several Mihomo
//! instances on one host never collide; `include-uid` limits each TUN to its
//! owner's traffic, with sniffing enabled for the plain-IP connections a shared
//! system resolver produces. An installed host instead has one system-wide TUN
//! at a time (see [`TunScope`]). Runs after finalization, so it wins over
//! subscriptions, scripts and manual enhancements.
use std::net::Ipv4Addr;

use anyhow::{Result, ensure};
use serde_yaml_ng::{Mapping, Value};

use crate::config::settings::RuntimeSettings;

/// Number of users one host can serve.
pub const SLOTS: u16 = 64;
/// Base for slots 1–63; slot 0 keeps the familiar single-user ports.
pub const PORT_BASE: u16 = 20000;
pub const PORTS_PER_SLOT: u16 = 10;
// Below the main (32766) and default (32767) rules; sing-tun uses about ten
// consecutive priorities from its rule index.
const RULE_INDEX_BASE: u32 = 10000;
const RULES_PER_SLOT: u32 = 32;
const TABLE_INDEX_BASE: u32 = 10000;
// 198.19.0.0/16 split into /22 blocks: 1024 fake IPs per user. 198.18.0.0/16
// is avoided because it is the default of every other Clash/Mihomo, and the
// route to a TUN's own /30 is not limited to its owner's UID.
const FAKE_IP_BASE: Ipv4Addr = Ipv4Addr::new(198, 19, 0, 0);
const FAKE_IP_BLOCK: u32 = 1024;
const FAKE_IP_PREFIX: u8 = 22;

/// Listener fields a user may still choose explicitly on the settings page.
const LISTENERS: [&str; 5] = ["mixed-port", "port", "socks-port", "redir-port", "tproxy-port"];

/// Whose traffic a user's TUN captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunScope {
    /// Only this user's traffic (`include-uid`); the shared resolver is left alone.
    Own,
    /// The whole host, like a single-user client. Mihomo also points
    /// systemd-resolved at the TUN, so every account's fake IPs must be routed
    /// through it; the service lets only one user hold such a TUN at a time.
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Isolation {
    uid: u32,
    slot: u16,
    tun: TunScope,
}

impl Isolation {
    pub fn new(uid: u32, slot: u16) -> Result<Self> {
        ensure!(slot < SLOTS, "multi-user slot must be below {SLOTS}");
        Ok(Self {
            uid,
            slot,
            tun: TunScope::Own,
        })
    }

    /// Installed hosts (with the system TUN lock) give each TUN the whole host.
    pub fn with_system_tun(mut self, system: bool) -> Self {
        self.tun = if system { TunScope::System } else { TunScope::Own };
        self
    }

    pub fn tun_scope(&self) -> TunScope {
        self.tun
    }

    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn slot(&self) -> u16 {
        self.slot
    }

    pub fn management_port(&self) -> u16 {
        if self.slot == 0 {
            9090
        } else {
            PORT_BASE + self.slot * PORTS_PER_SLOT
        }
    }

    pub fn mixed_port(&self) -> u16 {
        if self.slot == 0 {
            7890
        } else {
            self.management_port() + 1
        }
    }

    pub fn dns_listen(&self) -> String {
        let port = if self.slot == 0 {
            1053
        } else {
            self.management_port() + 2
        };
        format!("127.0.0.1:{port}")
    }

    /// "ms" plus at most ten UID digits stays within the 15-byte interface limit.
    pub fn tun_device(&self) -> String {
        format!("ms{}", self.uid)
    }

    pub fn rule_index(&self) -> u32 {
        RULE_INDEX_BASE + u32::from(self.slot) * RULES_PER_SLOT
    }

    pub fn table_index(&self) -> u32 {
        TABLE_INDEX_BASE + u32::from(self.slot)
    }

    /// Mihomo derives the TUN IPv4 address from this range, so it must be unique.
    pub fn fake_ip_range(&self) -> String {
        let first = u32::from(FAKE_IP_BASE) + u32::from(self.slot) * FAKE_IP_BLOCK + 1;
        format!("{}/{FAKE_IP_PREFIX}", Ipv4Addr::from(first))
    }

    pub fn fake_ip_range6(&self) -> String {
        format!("2001:2:0:{:x}::1/64", self.slot)
    }

    pub fn tun_inet6_address(&self) -> String {
        format!("fdfe:dcba:9876:{:x}::1/126", self.slot)
    }

    /// Rewrites shared host resources into this user's slot and returns the
    /// changed field paths. Listener ports and the DNS listener chosen on the
    /// settings page are kept; TUN routing and fake-IP ranges always follow the slot.
    pub fn apply(&self, mut config: Mapping, runtime: &RuntimeSettings) -> (Mapping, Vec<String>) {
        let mut changed = Vec::new();
        let explicit = [
            runtime.mixed_port,
            runtime.port,
            runtime.socks_port,
            runtime.redir_port,
            runtime.tproxy_port,
        ];
        for (field, explicit) in LISTENERS.into_iter().zip(explicit) {
            if explicit.is_some() {
                continue;
            }
            if field == "mixed-port" {
                set(&mut config, field, self.mixed_port().into(), &mut changed, field);
            } else if config.get(field).is_some_and(|value| value.as_u64() != Some(0)) {
                config.remove(field);
                changed.push(field.into());
            }
        }

        let has_tun = config.get("tun").is_some_and(Value::is_mapping);
        if has_tun || config.get("dns").is_some_and(Value::is_mapping) {
            let mut dns = section(&mut config, "dns");
            let explicit_listen = runtime.dns.as_ref().is_some_and(|dns| dns.listen.is_some());
            if !explicit_listen
                && dns
                    .get("listen")
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.trim().is_empty())
            {
                set(&mut dns, "listen", self.dns_listen().into(), &mut changed, "dns.listen");
            }
            set(
                &mut dns,
                "fake-ip-range",
                self.fake_ip_range().into(),
                &mut changed,
                "dns.fake-ip-range",
            );
            set(
                &mut dns,
                "fake-ip-range6",
                self.fake_ip_range6().into(),
                &mut changed,
                "dns.fake-ip-range6",
            );
            config.insert("dns".into(), dns.into());
        }

        if has_tun {
            let mut tun = section(&mut config, "tun");
            set(&mut tun, "device", self.tun_device().into(), &mut changed, "tun.device");
            if self.tun == TunScope::System {
                if tun.remove("include-uid").is_some() {
                    changed.push("tun.include-uid".into());
                }
            } else {
                let uid = Value::Sequence(vec![u64::from(self.uid).into()]);
                set(&mut tun, "include-uid", uid, &mut changed, "tun.include-uid");
            }
            if tun.remove("include-uid-range").is_some() {
                changed.push("tun.include-uid-range".into());
            }
            let table = u64::from(self.table_index()).into();
            set(
                &mut tun,
                "iproute2-table-index",
                table,
                &mut changed,
                "tun.iproute2-table-index",
            );
            let rule = u64::from(self.rule_index()).into();
            set(
                &mut tun,
                "iproute2-rule-index",
                rule,
                &mut changed,
                "tun.iproute2-rule-index",
            );
            let inet6 = Value::Sequence(vec![self.tun_inet6_address().into()]);
            set(&mut tun, "inet6-address", inet6, &mut changed, "tun.inet6-address");
            // auto-redirect installs host-wide nftables tables with fixed names.
            if tun.get("auto-redirect").and_then(Value::as_bool) == Some(true) {
                tun.insert("auto-redirect".into(), false.into());
                changed.push("tun.auto-redirect".into());
            }
            let enabled = tun.get("enable").and_then(Value::as_bool) == Some(true);
            config.insert("tun".into(), tun.into());
            if enabled {
                sniff_pure_ip(&mut config, &mut changed);
            }
        }
        (config, changed)
    }
}

/// A per-user TUN cannot hijack DNS answered by a shared resolver
/// (systemd-resolved, nscd), so its connections arrive as plain IPs. Sniffing
/// them recovers the domain for rules and remote resolution. An explicitly
/// disabled sniffer is kept.
fn sniff_pure_ip(config: &mut Mapping, changed: &mut Vec<String>) {
    let mut sniffer = section(config, "sniffer");
    if sniffer.get("enable").and_then(Value::as_bool) != Some(false) {
        if sniffer.is_empty() {
            sniffer = serde_yaml_ng::from_str(
                "{enable: true, override-destination: true, sniff: {HTTP: {ports: [80, 8080-8880]}, \
                 TLS: {ports: [443, 8443]}, QUIC: {ports: [443, 8443]}}}",
            )
            .expect("static sniffer mapping");
            changed.push("sniffer".into());
        }
        set(&mut sniffer, "enable", true.into(), changed, "sniffer.enable");
        set(
            &mut sniffer,
            "parse-pure-ip",
            true.into(),
            changed,
            "sniffer.parse-pure-ip",
        );
    }
    config.insert("sniffer".into(), sniffer.into());
}

fn section(config: &mut Mapping, name: &str) -> Mapping {
    config
        .remove(name)
        .and_then(|value| value.as_mapping().cloned())
        .unwrap_or_default()
}

fn set(map: &mut Mapping, key: &str, value: Value, changed: &mut Vec<String>, path: &str) {
    if map.get(key) != Some(&value) {
        map.insert(key.into(), value);
        changed.push(path.into());
    }
}
