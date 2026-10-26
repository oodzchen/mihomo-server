//! Android (netd) policy routing for a container sharing the host's network
//! namespace, such as Droidspaces with host networking.
//!
//! netd never adds Linux's catch-all `from all lookup main` rule: the default
//! route lives in a per-network table and everything unmatched ends in
//! `unreachable`. Mihomo's Linux TUN route has a gateway in the TUN's own /30,
//! which the kernel validates through those rules, and the core's own traffic
//! would follow the TUN rules back into the TUN. The TUN launcher therefore
//! adds three IPv4 rules in the slot's priority block (see
//! `Isolation::with_android_routing`):
//!
//! - `rule`:     `to <TUN /30> lookup main`, so the gateway is reachable;
//! - `rule + 1`: the core's mark `goto last`, skipping the slot's TUN rules;
//! - `last`:     `nop`, from where netd's own rules route the marked traffic.
//!
//! They only match the TUN's own network and the core's mark, so they are left
//! in place when the core stops.
use std::{
    ffi::CStr,
    io,
    net::Ipv4Addr,
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
};

use anyhow::{Context as _, Result, bail, ensure};

/// The launcher's input from the service: `RULE LAST NETWORK/30`.
pub(crate) const RULES_ENV: &str = "MIHOMO_TUN_ANDROID_RULES";
/// netd's mark bits checked against the core's mark: network, "explicitly
/// selected" and "protected from VPN".
const MARK_MASK: u32 = 0x3_ffff;

const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;
const RTM_NEWRULE: u16 = 32;
const RTM_GETRULE: u16 = 34;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_ACK: u16 = 0x4;
const NLM_F_EXCL: u16 = 0x200;
const NLM_F_CREATE: u16 = 0x400;
const NLM_F_DUMP: u16 = 0x300;
const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_MARK: u16 = 16;
const FRA_DST: u16 = 1;
const FRA_GOTO: u16 = 4;
const FRA_PRIORITY: u16 = 6;
const FRA_FWMARK: u16 = 10;
const FRA_SUPPRESS_IFGROUP: u16 = 13;
const FRA_SUPPRESS_PREFIXLEN: u16 = 14;
const FRA_TABLE: u16 = 15;
const FRA_FWMASK: u16 = 16;
const FRA_PROTOCOL: u16 = 21;
const FR_ACT_TO_TBL: u8 = 1;
const FR_ACT_GOTO: u8 = 2;
const FR_ACT_NOP: u8 = 3;
const FIB_RULE_INVERT: u32 = 0x2;
const RT_TABLE_MAIN: u8 = 254;
const HEADER: usize = 16;
const RULE_HEADER: usize = 12;
const ROUTE_HEADER: usize = 12;

/// Whether IPv4 policy routing has no catch-all `lookup main` rule, as with
/// Android's netd. Unknown (no netlink) counts as ordinary Linux.
pub fn android_style() -> bool {
    match dump_rules() {
        Ok(rules) => !rules.is_empty() && !rules.iter().any(Rule::catch_all_main),
        Err(_) => false,
    }
}

/// The interface the kernel routes `destination` through for traffic carrying
/// `mark`, as `ip route get DESTINATION mark MARK` shows it. No privileges.
pub(crate) fn route_interface(destination: Ipv4Addr, mark: Option<u32>) -> Result<String> {
    let socket = Socket::open()?;
    let mut request = header(RTM_GETROUTE, NLM_F_REQUEST, 1);
    request.extend_from_slice(&[libc::AF_INET as u8, 32, 0, 0, 0, 0, 0, 0]);
    request.extend_from_slice(&0_u32.to_ne_bytes());
    attribute(&mut request, RTA_DST, &destination.octets());
    if let Some(mark) = mark {
        attribute(&mut request, RTA_MARK, &mark.to_ne_bytes());
    }
    socket.send(&finish(request))?;
    let index = output_interface(&socket.receive()?)?;
    let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: the buffer holds IF_NAMESIZE bytes, as if_indextoname requires.
    let found = unsafe { libc::if_indextoname(index, name.as_mut_ptr()) };
    if found.is_null() {
        return Err(io::Error::last_os_error()).context("name the route's interface");
    }
    // SAFETY: if_indextoname wrote a NUL-terminated name into the buffer.
    Ok(unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned())
}

/// The output interface index of an RTM_GETROUTE reply.
fn output_interface(bytes: &[u8]) -> Result<u32> {
    ensure!(bytes.len() >= HEADER, "truncated netlink message");
    let length = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
    ensure!((HEADER..=bytes.len()).contains(&length), "truncated netlink message");
    match u16::from_ne_bytes([bytes[4], bytes[5]]) {
        NLMSG_ERROR => bail!(io::Error::from_raw_os_error(-acknowledgement(&bytes[..length])?)),
        RTM_NEWROUTE if length >= HEADER + ROUTE_HEADER => {}
        _ => bail!("unexpected netlink reply"),
    }
    let mut attributes = &bytes[HEADER + ROUTE_HEADER..length];
    while attributes.len() >= 4 {
        let size = usize::from(u16::from_ne_bytes([attributes[0], attributes[1]]));
        ensure!((4..=attributes.len()).contains(&size), "truncated route attribute");
        if u16::from_ne_bytes([attributes[2], attributes[3]]) & 0x3fff == RTA_OIF && size >= 8 {
            return Ok(u32::from_ne_bytes(attributes[4..8].try_into().unwrap()));
        }
        attributes = &attributes[align(size).min(attributes.len())..];
    }
    bail!("the route has no output interface")
}

/// The launcher's rules; idempotent, needs CAP_NET_ADMIN.
pub(crate) fn ensure_tun_rules(spec: &str) -> Result<()> {
    let (rule, last, network) = parse_spec(spec)?;
    let mark = headless_core::enhance::isolation::ANDROID_ROUTING_MARK;
    // The jump target first, so the goto never points at a missing priority.
    add_rule(&NewRule {
        priority: last,
        action: FR_ACT_NOP,
        ..NewRule::default()
    })?;
    add_rule(&NewRule {
        priority: rule,
        action: FR_ACT_TO_TBL,
        table: RT_TABLE_MAIN,
        dst: Some(network),
        ..NewRule::default()
    })?;
    add_rule(&NewRule {
        priority: rule + 1,
        action: FR_ACT_GOTO,
        fwmark: Some((mark, MARK_MASK)),
        goto: Some(last),
        ..NewRule::default()
    })
}

pub(crate) fn spec(rule: u32, last: u32, (address, prefix): (Ipv4Addr, u8)) -> String {
    format!("{rule} {last} {address}/{prefix}")
}

fn parse_spec(spec: &str) -> Result<(u32, u32, (Ipv4Addr, u8))> {
    let mut parts = spec.split(' ');
    let (Some(rule), Some(last), Some(network), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        bail!("invalid {RULES_ENV}");
    };
    let (rule, last): (u32, u32) = (rule.parse()?, last.parse()?);
    let (address, prefix) = network.split_once('/').context("invalid TUN network")?;
    let (address, prefix): (Ipv4Addr, u8) = (address.parse()?, prefix.parse()?);
    // Within the slots' priority range, before netd's catch-all, and a /30 inside
    // the slots' fake-IP block.
    ensure!(
        (10_000..32_000).contains(&rule) && rule + 1 < last && last < 32_000,
        "TUN rule priorities out of range"
    );
    ensure!(
        prefix == 30 && address.octets()[..2] == [198, 19] && u32::from(address) % 4 == 0,
        "unexpected TUN network"
    );
    Ok((rule, last, (address, prefix)))
}

#[derive(Default)]
struct NewRule {
    priority: u32,
    action: u8,
    table: u8,
    dst: Option<(Ipv4Addr, u8)>,
    fwmark: Option<(u32, u32)>,
    goto: Option<u32>,
}

fn rule_message(rule: &NewRule, sequence: u32) -> Vec<u8> {
    let dst_len = rule.dst.map_or(0, |(_, prefix)| prefix);
    let mut message = header(
        RTM_NEWRULE,
        NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL,
        sequence,
    );
    message.extend_from_slice(&[libc::AF_INET as u8, dst_len, 0, 0, rule.table, 0, 0, rule.action]);
    message.extend_from_slice(&0_u32.to_ne_bytes());
    attribute(&mut message, FRA_PRIORITY, &rule.priority.to_ne_bytes());
    if rule.table != 0 {
        attribute(&mut message, FRA_TABLE, &u32::from(rule.table).to_ne_bytes());
    }
    if let Some((address, _)) = rule.dst {
        attribute(&mut message, FRA_DST, &address.octets());
    }
    if let Some((mark, mask)) = rule.fwmark {
        attribute(&mut message, FRA_FWMARK, &mark.to_ne_bytes());
        attribute(&mut message, FRA_FWMASK, &mask.to_ne_bytes());
    }
    if let Some(target) = rule.goto {
        attribute(&mut message, FRA_GOTO, &target.to_ne_bytes());
    }
    finish(message)
}

fn add_rule(rule: &NewRule) -> Result<()> {
    let socket = Socket::open()?;
    socket.send(&rule_message(rule, 1))?;
    let reply = socket.receive()?;
    match acknowledgement(&reply)? {
        0 => Ok(()),
        // NLM_F_EXCL: an identical rule already exists.
        code if code == -libc::EEXIST => Ok(()),
        code => Err(io::Error::from_raw_os_error(-code))
            .with_context(|| format!("add policy rule at priority {}", rule.priority)),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Rule {
    dst_len: u8,
    src_len: u8,
    tos: u8,
    table: u32,
    action: u8,
    flags: u32,
    attributes: Vec<u16>,
}

impl Rule {
    /// `from all lookup main` without any selector, as on ordinary Linux (32766).
    fn catch_all_main(&self) -> bool {
        self.action == FR_ACT_TO_TBL
            && self.table == u32::from(RT_TABLE_MAIN)
            && (self.dst_len, self.src_len, self.tos) == (0, 0, 0)
            && self.flags & FIB_RULE_INVERT == 0
            && self
                .attributes
                .iter()
                .all(|kind| matches!(*kind, FRA_PRIORITY | FRA_TABLE | FRA_PROTOCOL))
    }
}

fn dump_rules() -> Result<Vec<Rule>> {
    let socket = Socket::open()?;
    let mut request = header(RTM_GETRULE, NLM_F_REQUEST | NLM_F_DUMP, 1);
    request.extend_from_slice(&[libc::AF_INET as u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    socket.send(&finish(request))?;
    let mut rules = Vec::new();
    loop {
        let reply = socket.receive()?;
        if parse_dump(&reply, &mut rules)? {
            return Ok(rules);
        }
    }
}

/// Appends the rules in one dump reply; true once the dump is done.
fn parse_dump(mut bytes: &[u8], rules: &mut Vec<Rule>) -> Result<bool> {
    while bytes.len() >= HEADER {
        let length = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
        ensure!((HEADER..=bytes.len()).contains(&length), "truncated netlink message");
        let kind = u16::from_ne_bytes([bytes[4], bytes[5]]);
        let body = &bytes[HEADER..length];
        match kind {
            NLMSG_DONE => return Ok(true),
            NLMSG_ERROR => {
                let code = acknowledgement(&bytes[..length])?;
                bail!(io::Error::from_raw_os_error(-code));
            }
            RTM_NEWRULE if body.len() >= RULE_HEADER => {
                let mut rule = Rule {
                    dst_len: body[1],
                    src_len: body[2],
                    tos: body[3],
                    table: u32::from(body[4]),
                    action: body[7],
                    flags: u32::from_ne_bytes(body[8..12].try_into().unwrap()),
                    attributes: Vec::new(),
                };
                let mut attributes = &body[RULE_HEADER..];
                while attributes.len() >= 4 {
                    let size = usize::from(u16::from_ne_bytes([attributes[0], attributes[1]]));
                    ensure!((4..=attributes.len()).contains(&size), "truncated rule attribute");
                    let kind = u16::from_ne_bytes([attributes[2], attributes[3]]) & 0x3fff;
                    let value = (size >= 8).then(|| u32::from_ne_bytes(attributes[4..8].try_into().unwrap()));
                    if kind == FRA_TABLE
                        && let Some(table) = value
                    {
                        rule.table = table;
                    }
                    // The kernel always reports both suppressors, -1 when unset.
                    let unset =
                        matches!(kind, FRA_SUPPRESS_PREFIXLEN | FRA_SUPPRESS_IFGROUP) && value == Some(u32::MAX);
                    if !unset {
                        rule.attributes.push(kind);
                    }
                    attributes = &attributes[align(size).min(attributes.len())..];
                }
                rules.push(rule);
            }
            _ => {}
        }
        bytes = &bytes[align(length).min(bytes.len())..];
    }
    Ok(false)
}

/// The (negative errno or zero) code of an NLMSG_ERROR reply.
fn acknowledgement(bytes: &[u8]) -> Result<i32> {
    ensure!(
        bytes.len() >= HEADER + 4 && u16::from_ne_bytes([bytes[4], bytes[5]]) == NLMSG_ERROR,
        "unexpected netlink reply"
    );
    Ok(i32::from_ne_bytes(bytes[HEADER..HEADER + 4].try_into().unwrap()))
}

fn header(kind: u16, flags: u16, sequence: u32) -> Vec<u8> {
    let mut message = Vec::with_capacity(64);
    message.extend_from_slice(&0_u32.to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.extend_from_slice(&flags.to_ne_bytes());
    message.extend_from_slice(&sequence.to_ne_bytes());
    message.extend_from_slice(&0_u32.to_ne_bytes());
    message
}

fn attribute(message: &mut Vec<u8>, kind: u16, payload: &[u8]) {
    let size = 4 + payload.len();
    message.extend_from_slice(&(size as u16).to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.extend_from_slice(payload);
    message.resize(align(message.len()), 0);
}

fn finish(mut message: Vec<u8>) -> Vec<u8> {
    let length = (message.len() as u32).to_ne_bytes();
    message[..4].copy_from_slice(&length);
    message
}

fn align(length: usize) -> usize {
    (length + 3) & !3
}

struct Socket(OwnedFd);

impl Socket {
    fn open() -> Result<Self> {
        // SAFETY: plain socket call; the descriptor is owned below.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error()).context("open netlink socket");
        }
        // SAFETY: fd is a new descriptor owned by nobody else.
        let socket = Self(unsafe { OwnedFd::from_raw_fd(fd) });
        let timeout = libc::timeval { tv_sec: 2, tv_usec: 0 };
        // SAFETY: valid descriptor, option and value size.
        let result = unsafe {
            libc::setsockopt(
                socket.0.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&timeout as *const libc::timeval).cast(),
                size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error()).context("set netlink timeout");
        }
        Ok(socket)
    }

    fn send(&self, message: &[u8]) -> Result<()> {
        // SAFETY: zeroed sockaddr_nl addresses the kernel.
        let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        kernel.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        // SAFETY: valid buffer, descriptor and address.
        let sent = unsafe {
            libc::sendto(
                self.0.as_raw_fd(),
                message.as_ptr().cast(),
                message.len(),
                0,
                (&kernel as *const libc::sockaddr_nl).cast(),
                size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        ensure!(
            sent == message.len() as isize,
            "send netlink request: {}",
            io::Error::last_os_error()
        );
        Ok(())
    }

    fn receive(&self) -> Result<Vec<u8>> {
        let mut buffer = vec![0_u8; 64 * 1024];
        // SAFETY: valid descriptor and buffer.
        let received = unsafe { libc::recv(self.0.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if received < 0 {
            return Err(io::Error::last_os_error()).context("receive netlink reply");
        }
        buffer.truncate(received as usize);
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Action, destination length, flags and attributes of one dumped rule.
    type Fixture<'a> = (u8, u8, u32, &'a [(u16, Vec<u8>)]);

    fn dump(rules: &[Fixture]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (action, dst_len, flags, attributes) in rules {
            let mut message = header(RTM_NEWRULE, 0x2, 1);
            message.extend_from_slice(&[libc::AF_INET as u8, *dst_len, 0, 0, RT_TABLE_MAIN, 0, 0, *action]);
            message.extend_from_slice(&flags.to_ne_bytes());
            for (kind, payload) in *attributes {
                attribute(&mut message, *kind, payload);
            }
            bytes.extend(finish(message));
        }
        bytes.extend(finish(header(NLMSG_DONE, 0x2, 1)));
        bytes
    }

    #[test]
    fn only_a_selector_free_main_lookup_is_the_catch_all() -> Result<()> {
        let priority = (FRA_PRIORITY, 32766_u32.to_ne_bytes().to_vec());
        let protocol = (FRA_PROTOCOL, vec![2]);
        let unset = (FRA_SUPPRESS_PREFIXLEN, u32::MAX.to_ne_bytes().to_vec());
        for (rule, catch_all) in [
            ((FR_ACT_TO_TBL, 0, 0, vec![priority.clone(), protocol.clone()]), true),
            (
                (
                    FR_ACT_TO_TBL,
                    0,
                    0,
                    vec![unset.clone(), priority.clone(), protocol.clone()],
                ),
                true,
            ),
            // Android: `to 192.168.31.0/24 lookup main`, fwmark or suppressing lookups.
            (
                (
                    FR_ACT_TO_TBL,
                    24,
                    0,
                    vec![priority.clone(), (FRA_DST, vec![192, 168, 31, 0])],
                ),
                false,
            ),
            (
                (FR_ACT_TO_TBL, 0, 0, vec![priority.clone(), (FRA_FWMARK, vec![0; 4])]),
                false,
            ),
            (
                (
                    FR_ACT_TO_TBL,
                    0,
                    0,
                    vec![priority.clone(), (FRA_SUPPRESS_PREFIXLEN, vec![0; 4])],
                ),
                false,
            ),
            ((FR_ACT_TO_TBL, 0, FIB_RULE_INVERT, vec![priority.clone()]), false),
            ((7, 0, 0, vec![priority.clone()]), false),
        ] {
            let (action, dst_len, flags, attributes) = rule;
            let mut rules = Vec::new();
            assert!(parse_dump(&dump(&[(action, dst_len, flags, &attributes)]), &mut rules)?);
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].catch_all_main(), catch_all, "{:?}", rules[0]);
        }
        Ok(())
    }

    #[test]
    fn launcher_spec_round_trips_and_rejects_foreign_networks() -> Result<()> {
        let spec = spec(10000, 10031, ("198.19.0.0".parse()?, 30));
        assert_eq!(spec, "10000 10031 198.19.0.0/30");
        assert_eq!(parse_spec(&spec)?, (10000, 10031, ("198.19.0.0".parse()?, 30)));
        for invalid in [
            "10000 10031 10.0.0.0/30",
            "10000 10031 198.19.0.1/30",
            "10000 10031 198.19.0.0/24",
            "10000 10001 198.19.0.0/30",
            "100 10031 198.19.0.0/30",
            "10000 10031",
            "10000 10031 198.19.0.0/30 extra",
        ] {
            assert!(parse_spec(invalid).is_err(), "{invalid}");
        }
        Ok(())
    }

    #[test]
    fn rule_messages_carry_the_selector_and_target() {
        let message = rule_message(
            &NewRule {
                priority: 10001,
                action: FR_ACT_GOTO,
                fwmark: Some((0x3_0000, MARK_MASK)),
                goto: Some(10031),
                ..NewRule::default()
            },
            1,
        );
        assert_eq!(
            u32::from_ne_bytes(message[..4].try_into().unwrap()) as usize,
            message.len()
        );
        assert_eq!(message[HEADER + 7], FR_ACT_GOTO);
        let mut rules = Vec::new();
        let mut reply = message.clone();
        reply.extend(finish(header(NLMSG_DONE, 0, 1)));
        assert!(parse_dump(&reply, &mut rules).unwrap());
        assert_eq!(
            rules[0].attributes,
            vec![FRA_PRIORITY, FRA_FWMARK, FRA_FWMASK, FRA_GOTO]
        );
    }

    #[test]
    fn ordinary_linux_hosts_are_not_android() {
        // Development and CI hosts keep `32766: from all lookup main`.
        assert!(!android_style());
    }

    #[test]
    fn route_replies_name_their_output_interface() -> Result<()> {
        let mut reply = header(RTM_NEWROUTE, 0, 1);
        reply.extend_from_slice(&[libc::AF_INET as u8, 32, 0, 0, 254, 0, 0, 1]);
        reply.extend_from_slice(&0_u32.to_ne_bytes());
        attribute(&mut reply, RTA_DST, &[1, 1, 1, 1]);
        attribute(&mut reply, RTA_OIF, &7_u32.to_ne_bytes());
        assert_eq!(output_interface(&finish(reply))?, 7);

        let mut error = header(NLMSG_ERROR, 0, 1);
        error.extend_from_slice(&(-libc::ENETUNREACH).to_ne_bytes());
        assert!(output_interface(&finish(error)).is_err());
        Ok(())
    }

    #[test]
    fn loopback_is_routed_through_lo() -> Result<()> {
        assert_eq!(route_interface(Ipv4Addr::LOCALHOST, None)?, "lo");
        Ok(())
    }

    /// Needs root in a private network namespace (e.g. a privileged container).
    #[test]
    #[ignore = "adds policy rules: run as root in a scratch network namespace"]
    fn tun_rules_are_added_once() -> Result<()> {
        let spec = spec(10000, 10031, ("198.19.0.0".parse()?, 30));
        ensure_tun_rules(&spec)?;
        ensure_tun_rules(&spec)?;
        let rules = dump_rules()?;
        let ours = rules
            .iter()
            .filter(|rule| rule.attributes.contains(&FRA_GOTO) || rule.action == FR_ACT_NOP || rule.dst_len == 30);
        assert_eq!(ours.count(), 3);
        Ok(())
    }
}
