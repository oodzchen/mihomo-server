//! CN address ranges from the core's MMDB, for the domestic TUN bypass.
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
};

use anyhow::{Context as _, Result};
use serde::Deserialize;

use super::validation;

/// The MMDB Mihomo reads for `GEOIP` rules: `Country.mmdb` when present.
const FILES: [&str; 2] = ["Country.mmdb", "geoip.metadb"];

/// Records of MetaDB (`"CN"` or `["CN", …]`) and MaxMind country databases.
#[derive(Deserialize)]
#[serde(untagged)]
enum Record {
    Code(String),
    Codes(Vec<String>),
    Country { country: Option<Country> },
}

#[derive(Deserialize)]
struct Country {
    iso_code: Option<String>,
}

impl Record {
    fn is_cn(&self) -> bool {
        let cn = |code: &str| code.eq_ignore_ascii_case("CN");
        match self {
            Record::Code(code) => cn(code),
            Record::Codes(codes) => codes.iter().any(|code| cn(code)),
            Record::Country { country } => country
                .as_ref()
                .and_then(|country| country.iso_code.as_deref())
                .is_some_and(cn),
        }
    }
}

/// The source file's SHA-256 and its CN ranges, merged into the fewest CIDRs
/// (IPv4 first).
#[cfg(unix)]
pub(crate) fn ranges(data: &Path) -> Result<(String, Vec<String>)> {
    let (name, bytes) = FILES
        .iter()
        .find_map(|name| validation::snapshot(data, name).transpose().map(|bytes| (*name, bytes)))
        .context("domestic bypass needs a GeoIP MMDB (Country.mmdb or geoip.metadb); update Geo data first")?;
    let bytes = bytes?;
    let hash = validation::sha256(&bytes);
    let reader = maxminddb::Reader::from_source(bytes).with_context(|| format!("invalid {name}"))?;
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for lookup in reader.networks(Default::default())? {
        let lookup = lookup?;
        if !lookup
            .decode::<Record>()
            .ok()
            .flatten()
            .is_some_and(|record| record.is_cn())
        {
            continue;
        }
        let network = lookup.network()?;
        let prefix = u32::from(network.prefix());
        match network.network() {
            IpAddr::V4(ip) => v4.push(span(u128::from(u32::from(ip)), prefix, 32)),
            IpAddr::V6(ip) => v6.push(span(u128::from(ip), prefix, 128)),
        }
    }
    let mut ranges: Vec<String> = cidrs(v4, 32)
        .map(|(start, prefix)| format!("{}/{prefix}", Ipv4Addr::from(start as u32)))
        .collect();
    ranges.extend(cidrs(v6, 128).map(|(start, prefix)| format!("{}/{prefix}", Ipv6Addr::from(start))));
    anyhow::ensure!(!ranges.is_empty(), "{name} has no CN ranges");
    Ok((hash, ranges))
}

/// Host mask of a block of `2^size` addresses.
fn mask(size: u32) -> u128 {
    1u128.checked_shl(size).unwrap_or(0).wrapping_sub(1)
}

/// Inclusive address span of `start/prefix` in a `bits`-wide family.
fn span(start: u128, prefix: u32, bits: u32) -> (u128, u128) {
    (start, start + mask(bits - prefix))
}

/// Merge overlapping or adjacent spans, then cover each with maximal CIDRs.
fn cidrs(mut spans: Vec<(u128, u128)>, bits: u32) -> impl Iterator<Item = (u128, u32)> {
    spans.sort_unstable();
    let mut merged: Vec<(u128, u128)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if last.1.checked_add(1).is_none_or(|next| start <= next) => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged.into_iter().flat_map(move |(mut start, end)| {
        let mut out = Vec::new();
        loop {
            // The largest block aligned at `start` that does not pass `end`.
            let mut size = start.trailing_zeros().min(bits);
            while size > 0 && start + mask(size) > end {
                size -= 1;
            }
            let last = start + mask(size);
            out.push((start, bits - size));
            if last >= end {
                break out;
            }
            start = last + 1;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(spans: &[(&str, u32)]) -> Vec<String> {
        let spans = spans
            .iter()
            .map(|(ip, prefix)| span(u128::from(u32::from(ip.parse::<Ipv4Addr>().unwrap())), *prefix, 32))
            .collect();
        cidrs(spans, 32)
            .map(|(start, prefix)| format!("{}/{prefix}", Ipv4Addr::from(start as u32)))
            .collect()
    }

    #[test]
    fn merges_adjacent_and_nested_networks_into_minimal_cidrs() {
        assert_eq!(v4(&[("1.0.1.0", 24), ("1.0.0.0", 24)]), ["1.0.0.0/23"]);
        assert_eq!(v4(&[("10.0.0.0", 8), ("10.1.0.0", 16)]), ["10.0.0.0/8"]);
        assert_eq!(v4(&[("1.0.1.0", 24), ("1.0.2.0", 24)]), ["1.0.1.0/24", "1.0.2.0/24"]);
        assert_eq!(v4(&[("0.0.0.0", 1), ("128.0.0.0", 1)]), ["0.0.0.0/0"]);
        assert_eq!(v4(&[("255.255.255.255", 32)]), ["255.255.255.255/32"]);
    }

    #[test]
    fn reads_metadb_and_maxmind_records() {
        let cn = |yaml: &str| serde_yaml_ng::from_str::<Record>(yaml).unwrap().is_cn();
        assert!(cn("CN") && cn("[HK, CN]") && cn("{country: {iso_code: CN}}"));
        assert!(!cn("US") && !cn("[]") && !cn("{country: {iso_code: JP}}") && !cn("{continent: {}}"));
    }

    /// `MIHOMO_TEST_GEO_DIR=<dir with geoip.metadb>`.
    #[cfg(unix)]
    #[test]
    #[ignore = "needs a real GeoIP MMDB"]
    fn real_database_has_cn_ranges() -> Result<()> {
        let dir = std::env::var_os("MIHOMO_TEST_GEO_DIR").context("MIHOMO_TEST_GEO_DIR")?;
        let started = std::time::Instant::now();
        let (_, ranges) = ranges(Path::new(&dir))?;
        let v6 = ranges.iter().filter(|range| range.contains(':')).count();
        println!(
            "{} IPv4 + {v6} IPv6 ranges in {:?}",
            ranges.len() - v6,
            started.elapsed()
        );
        for known in ["223.5.5.0/24", "114.114.114.0/24"] {
            let ip: Ipv4Addr = known.split('/').next().unwrap().parse()?;
            assert!(
                ranges.iter().filter(|range| !range.contains(':')).any(|range| {
                    let (start, prefix) = range.split_once('/').unwrap();
                    let (start, end) = span(
                        u128::from(u32::from(start.parse::<Ipv4Addr>().unwrap())),
                        prefix.parse().unwrap(),
                        32,
                    );
                    (start..=end).contains(&u128::from(u32::from(ip)))
                }),
                "{known} is not covered"
            );
        }
        assert!(!ranges.iter().any(|range| range.starts_with("8.8.8.")));
        Ok(())
    }
}
