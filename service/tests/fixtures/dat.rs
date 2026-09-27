//! Small independent encodings of the referenced Mihomo router protobuf schema.
#![allow(dead_code)]
pub fn varint(mut n: u64) -> Vec<u8> {
    let mut result = Vec::new();
    while n > 127 {
        result.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    result.push(n as u8);
    result
}
pub fn scalar(field: u32, n: u64) -> Vec<u8> {
    [varint(u64::from(field) << 3), varint(n)].concat()
}
pub fn bytes(field: u32, data: &[u8]) -> Vec<u8> {
    [
        varint((u64::from(field) << 3) | 2),
        varint(data.len() as u64),
        data.to_vec(),
    ]
    .concat()
}
pub fn cidr(ip: &[u8], prefix: u64) -> Vec<u8> {
    [bytes(1, ip), scalar(2, prefix)].concat()
}
pub fn domain(kind: u64, value: &[u8]) -> Vec<u8> {
    [scalar(1, kind), bytes(2, value)].concat()
}
pub fn group(code: &[u8], records: &[Vec<u8>]) -> Vec<u8> {
    let mut group = bytes(1, code);
    for record in records {
        group.extend(bytes(2, record));
    }
    bytes(1, &group)
}
pub fn geoip() -> Vec<u8> {
    let mut data = group(
        b"ms-dat",
        &[
            cidr(&[192, 0, 2, 0], 24),
            cidr(&[0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 32),
        ],
    );
    data.extend(group(b"CN", &[cidr(&[127, 0, 0, 0], 8)]));
    data
}

pub fn geosite() -> Vec<u8> {
    let mut full = domain(3, b"exact.dat.test");
    full.extend(bytes(3, &[bytes(1, b"test"), scalar(2, 1)].concat()));
    full.extend(bytes(3, &[bytes(1, b"rank"), scalar(3, u64::MAX)].concat()));
    let mut data = group(
        b"ms-dat",
        &[
            full,
            domain(2, b"suffix.dat.test"),
            domain(1, br"^regex\.dat\.test$"),
            domain(0, b"keyword"),
        ],
    );
    data.extend(group(b"CN", &[domain(3, b"bootstrap.invalid")]));
    data
}
