//! Bounded validation of Mihomo GeoIPList/GeoSiteList protobuf messages.
//! No record text/IP data is retained or returned; matching compatibility is separate.
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeSet;

pub const DAT_FILES: [&str; 2] = ["geoip.dat", "geosite.dat"];
const MAX_GROUPS: u64 = 20_000;
const MAX_RECORDS: u64 = 2_000_000;
const MAX_FIELDS: u64 = 20_000_000;

#[derive(Debug, Serialize)]
pub struct Statistics {
    pub kind: &'static str,
    pub group_count: u64,
    pub record_count: u64,
    pub ipv4_count: u64,
    pub ipv6_count: u64,
    pub regex_count: u64,
    pub attribute_count: u64,
    pub empty_group_count: u64,
    pub unknown_field_count: u64,
    pub has_cn_group: bool,
    pub core_matching_verified: bool,
    #[serde(skip)]
    pub(crate) group_codes: Vec<String>,
}

enum Data<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Fixed,
}
struct Field<'a> {
    number: u32,
    data: Data<'a>,
}
struct Message<'a> {
    bytes: &'a [u8],
}
fn varint(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    for i in 0..10 {
        let (&byte, rest) = bytes
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("truncated DAT varint"))?;
        *bytes = rest;
        ensure!(i != 9 || byte <= 1, "DAT varint overflow");
        value |= u64::from(byte & 127) << (i * 7);
        if byte < 128 {
            return Ok(value);
        }
    }
    anyhow::bail!("DAT varint overflow")
}
impl<'a> Message<'a> {
    fn next(&mut self, budget: &mut u64) -> Result<Option<Field<'a>>> {
        if self.bytes.is_empty() {
            return Ok(None);
        }
        ensure!(*budget > 0, "DAT field count exceeds validation limits");
        *budget -= 1;
        let tag = varint(&mut self.bytes)?;
        ensure!((1..=0x1fff_ffff).contains(&(tag >> 3)), "invalid DAT field number");
        let data = match tag & 7 {
            0 => Data::Number(varint(&mut self.bytes)?),
            1 | 5 => {
                let size = if tag & 7 == 1 { 8 } else { 4 };
                ensure!(self.bytes.len() >= size, "truncated DAT fixed field");
                self.bytes = &self.bytes[size..];
                Data::Fixed
            }
            2 => {
                let size = usize::try_from(varint(&mut self.bytes)?)?;
                ensure!(size <= self.bytes.len(), "truncated DAT length-delimited field");
                let (data, remaining) = self.bytes.split_at(size);
                self.bytes = remaining;
                Data::Bytes(data)
            }
            _ => anyhow::bail!("unsupported DAT wire type"),
        };
        Ok(Some(Field {
            number: (tag >> 3) as u32,
            data,
        }))
    }
}
fn number(data: Data<'_>) -> Result<u64> {
    match data {
        Data::Number(v) => Ok(v),
        _ => anyhow::bail!("incorrect DAT scalar wire type"),
    }
}
fn bytes(data: Data<'_>) -> Result<&[u8]> {
    match data {
        Data::Bytes(v) => Ok(v),
        _ => anyhow::bail!("incorrect DAT message/string wire type"),
    }
}
fn text(data: Data<'_>, max: usize) -> Result<&str> {
    let value = bytes(data)?;
    ensure!(
        !value.is_empty() && value.len() <= max && !value.contains(&0),
        "empty or oversized DAT text"
    );
    std::str::from_utf8(value).map_err(|_| anyhow::anyhow!("invalid DAT UTF-8"))
}
fn once(seen: &mut u8, bit: u8) -> Result<()> {
    ensure!(*seen & bit == 0, "duplicate DAT singular field");
    *seen |= bit;
    Ok(())
}
fn attribute(data: &[u8], stats: &mut Statistics, budget: &mut u64) -> Result<()> {
    let mut message = Message { bytes: data };
    let mut seen = 0;
    while let Some(field) = message.next(budget)? {
        match field.number {
            1 => {
                once(&mut seen, 1)?;
                text(field.data, 128)?;
            }
            2 => {
                once(&mut seen, 2)?;
                ensure!(number(field.data)? <= 1, "invalid DAT boolean attribute");
            }
            3 => {
                once(&mut seen, 2)?;
                number(field.data)?;
            } // int64 uses two's complement varints.
            _ => stats.unknown_field_count += 1,
        }
    }
    ensure!(seen & 1 != 0, "DAT attribute key missing");
    stats.attribute_count += 1;
    Ok(())
}
fn domain(data: &[u8], stats: &mut Statistics, budget: &mut u64) -> Result<()> {
    let mut message = Message { bytes: data };
    let mut seen = 0;
    let mut kind = 0;
    while let Some(field) = message.next(budget)? {
        match field.number {
            1 => {
                once(&mut seen, 1)?;
                kind = number(field.data)?;
                ensure!(kind <= 3, "unsupported DAT domain type");
            }
            2 => {
                once(&mut seen, 2)?;
                text(field.data, 4096)?;
            }
            3 => attribute(bytes(field.data)?, stats, budget)?,
            _ => stats.unknown_field_count += 1,
        }
    }
    ensure!(seen & 2 != 0, "DAT domain value missing");
    if kind == 1 {
        stats.regex_count += 1;
    }
    Ok(())
}
fn cidr(data: &[u8], stats: &mut Statistics, budget: &mut u64) -> Result<()> {
    let mut message = Message { bytes: data };
    let mut seen = 0;
    let mut size = 0;
    let mut prefix = 0;
    while let Some(field) = message.next(budget)? {
        match field.number {
            1 => {
                once(&mut seen, 1)?;
                size = bytes(field.data)?.len();
                ensure!(size == 4 || size == 16, "invalid DAT CIDR address length");
            }
            2 => {
                once(&mut seen, 2)?;
                prefix = number(field.data)?;
            }
            _ => stats.unknown_field_count += 1,
        }
    }
    ensure!(
        size != 0 && prefix <= size as u64 * 8,
        "invalid DAT CIDR prefix or missing address"
    );
    if size == 4 {
        stats.ipv4_count += 1;
    } else {
        stats.ipv6_count += 1;
    }
    Ok(())
}

pub(crate) fn validate(data: &[u8], name: &str) -> Result<Statistics> {
    ensure!(DAT_FILES.contains(&name), "unsupported DAT filename");
    let geoip = name == "geoip.dat";
    let mut stats = Statistics {
        kind: if geoip { "geoip" } else { "geosite" },
        group_count: 0,
        record_count: 0,
        ipv4_count: 0,
        ipv6_count: 0,
        regex_count: 0,
        attribute_count: 0,
        empty_group_count: 0,
        unknown_field_count: 0,
        has_cn_group: false,
        core_matching_verified: false,
        group_codes: Vec::new(),
    };
    let mut budget = MAX_FIELDS;
    let mut list = Message { bytes: data };
    let mut codes = BTreeSet::new();
    while let Some(field) = list.next(&mut budget)? {
        if field.number != 1 {
            stats.unknown_field_count += 1;
            continue;
        }
        stats.group_count += 1;
        ensure!(
            stats.group_count <= MAX_GROUPS,
            "DAT group count exceeds validation limits"
        );
        let mut group = Message {
            bytes: bytes(field.data)?,
        };
        let mut seen = 0;
        let mut count = 0;
        while let Some(field) = group.next(&mut budget)? {
            match field.number {
                1 => {
                    once(&mut seen, 1)?;
                    let code = text(field.data, 128)?;
                    stats.has_cn_group |= code.eq_ignore_ascii_case("cn");
                    ensure!(
                        code.is_ascii() && !code.chars().any(|c| c.is_control() || c.is_whitespace()),
                        "invalid DAT group identifier"
                    );
                    ensure!(
                        codes.insert(code.to_ascii_lowercase()),
                        "duplicate DAT group identifier"
                    );
                    stats.group_codes.push(code.to_owned());
                }
                2 => {
                    stats.record_count += 1;
                    count += 1;
                    ensure!(
                        stats.record_count <= MAX_RECORDS,
                        "DAT record count exceeds validation limits"
                    );
                    let value = bytes(field.data)?;
                    if geoip {
                        cidr(value, &mut stats, &mut budget)?;
                    } else {
                        domain(value, &mut stats, &mut budget)?;
                    }
                }
                3 if geoip => {
                    once(&mut seen, 2)?;
                    ensure!(number(field.data)? <= 1, "invalid DAT reverse-match boolean");
                }
                _ => stats.unknown_field_count += 1,
            }
        }
        ensure!(seen & 1 != 0, "DAT group identifier missing");
        if count == 0 {
            stats.empty_group_count += 1;
        }
    }
    ensure!(stats.group_count > 0, "DAT contains no groups");
    Ok(stats)
}

#[cfg(test)]
#[path = "../tests/fixtures/dat.rs"]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::{MAX_GROUPS, Message, validate};
    use anyhow::Result;
    #[test]
    fn verifies_all_record_kinds_and_attributes_without_exposing_records() -> Result<()> {
        let ip = validate(&geoip(), "geoip.dat")?;
        assert_eq!(
            (ip.group_count, ip.record_count, ip.ipv4_count, ip.ipv6_count),
            (2, 3, 2, 1)
        );
        let site = validate(&geosite(), "geosite.dat")?;
        assert_eq!(
            (
                site.group_count,
                site.record_count,
                site.regex_count,
                site.attribute_count
            ),
            (2, 5, 1, 2)
        );
        assert!(!site.core_matching_verified);
        let json = serde_json::to_string(&site)?;
        for record in ["ms-dat", "exact.dat.test", "suffix.dat.test", "keyword", "rank"] {
            assert!(!json.contains(record));
        }
        // Proto3 defaults: omitted domain type means Plain; omitted prefix means /0.
        assert_eq!(
            validate(&group(b"default", &[bytes(2, b"plain")]), "geosite.dat")?.record_count,
            1
        );
        assert_eq!(
            validate(&group(b"default", &[bytes(1, &[0; 4])]), "geoip.dat")?.record_count,
            1
        );
        Ok(())
    }
    #[test]
    fn rejects_invalid_schema_values_and_ambiguous_singular_fields() {
        for record in [
            cidr(&[1, 2, 3], 24),
            cidr(&[1, 2, 3, 4], 33),
            cidr(&[0; 16], 129),
            scalar(2, 1),
            [cidr(&[1, 2, 3, 4], 1), scalar(2, 2)].concat(),
        ] {
            assert!(validate(&group(b"x", &[record]), "geoip.dat").is_err());
        }
        for record in [
            domain(4, b"x"),
            domain(3, b""),
            domain(3, &[255]),
            domain(3, b"a\0b"),
            scalar(1, 2),
            [domain(1, b"x"), bytes(3, &scalar(2, 1))].concat(),
            [domain(1, b"x"), bytes(3, &[bytes(1, b"key"), scalar(2, 2)].concat())].concat(),
            [
                domain(1, b"x"),
                bytes(3, &[bytes(1, b"key"), scalar(2, 1), scalar(3, 1)].concat()),
            ]
            .concat(),
        ] {
            assert!(validate(&group(b"x", &[record]), "geosite.dat").is_err());
        }
        for data in [
            group(b"", &[]),
            group(b"bad code", &[]),
            [group(b"x", &[]), group(b"X", &[])].concat(),
            bytes(1, &bytes(2, &domain(0, b"x"))),
        ] {
            assert!(validate(&data, "geosite.dat").is_err());
        }
        assert!(validate(&geoip(), "geosite.dat").is_err());
        assert!(validate(&geosite(), "geoip.dat").is_err());
        assert!(validate(&geosite(), "../geosite.dat").is_err());
    }
    #[test]
    fn rejects_truncation_overflow_invalid_tags_wire_types_and_limits() {
        let fixture = group(b"single", &[cidr(&[192, 0, 2, 0], 24)]);
        for n in 0..fixture.len() {
            assert!(validate(&fixture[..n], "geoip.dat").is_err(), "offset {n}");
        }
        for data in [
            vec![0],
            vec![0x0b],
            vec![0x0f],
            vec![0x0a, 0xff],
            vec![0x09, 0],
            [vec![0x80; 9], vec![2]].concat(),
            varint(0x2000_0000 << 3),
        ] {
            assert!(validate(&data, "geoip.dat").is_err());
        }
        let mut message = Message { bytes: &[8, 1] };
        assert!(message.next(&mut 0).is_err());
        assert!(validate(&group(b"x", &[domain(0, &vec![b'a'; 4097])]), "geosite.dat").is_err());
        let groups: Vec<_> = (0..=MAX_GROUPS)
            .map(|n| group(format!("g{n}").as_bytes(), &[]))
            .collect();
        assert!(validate(&groups.concat(), "geosite.dat").is_err());
    }
    #[test]
    fn unknown_fields_and_empty_groups_are_visible_compatibility_diagnostics() -> Result<()> {
        let mut data = group(b"empty", &[]);
        data.extend(group(b"nonempty", &[domain(0, b"text")]));
        data.extend(scalar(8, 1));
        data.extend([varint((9 << 3) | 1), vec![0; 8]].concat());
        data.extend([varint((10 << 3) | 5), vec![0; 4]].concat());
        data.extend(bytes(11, b"opaque"));
        let stats = validate(&data, "geosite.dat")?;
        assert_eq!(stats.empty_group_count, 1);
        assert_eq!(stats.unknown_field_count, 4);
        // Matching/regexp compilation deliberately is not claimed by wire validation.
        let stats = validate(&group(b"syntax", &[domain(1, b"[")]), "geosite.dat")?;
        assert_eq!(stats.regex_count, 1);
        assert!(!stats.core_matching_verified);
        Ok(())
    }
}
