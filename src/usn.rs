//! Execution-free, checked decoding of the V2 records returned by NTFS IOCTLs.
use anyhow::{Result, bail, ensure};

pub const CREATE: u32 = 0x100;
pub const DELETE: u32 = 0x200;
pub const OLD_NAME: u32 = 0x1000;
pub const NEW_NAME: u32 = 0x2000;
pub const DIRECTORY: u32 = 0x10;

#[derive(Debug, Clone)]
pub struct UsnRecord {
    pub frn: u64,
    pub parent: u64,
    pub usn: i64,
    pub reason: u32,
    pub attributes: u32,
    pub name: Vec<u16>,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("validated field"))
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("validated field"))
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("validated field"))
}

/// Parse an entire batch before allowing its records to change the index.
pub fn decode_batch(bytes: &[u8]) -> Result<(u64, Vec<UsnRecord>)> {
    ensure!(bytes.len() >= 8, "NTFS response is missing its cursor");
    let cursor = u64_at(bytes, 0);
    let mut records = Vec::new();
    let mut at = 8;
    while at < bytes.len() {
        let remaining = &bytes[at..];
        ensure!(remaining.len() >= 60, "Truncated USN header at byte {at}");
        let length = u32_at(remaining, 0) as usize;
        ensure!(
            length >= 60 && length <= remaining.len() && length.is_multiple_of(8),
            "Invalid USN record length {length} at byte {at}"
        );
        let row = &remaining[..length];
        if u16_at(row, 4) != 2 {
            bail!(
                "Unsupported USN major version {}; this index preserves 64-bit NTFS V2 identity",
                u16_at(row, 4)
            );
        }
        let name_length = u16_at(row, 56) as usize;
        let name_offset = u16_at(row, 58) as usize;
        ensure!(
            name_length.is_multiple_of(2) && name_offset >= 60 && name_offset.is_multiple_of(2),
            "Invalid UTF-16 name extent at byte {at}"
        );
        let end = name_offset
            .checked_add(name_length)
            .ok_or_else(|| anyhow::anyhow!("Name overflow"))?;
        ensure!(end <= length, "USN name extends beyond its record");
        let name = row[name_offset..end]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        records.push(UsnRecord {
            frn: u64_at(row, 8),
            parent: u64_at(row, 16),
            usn: u64_at(row, 24) as i64,
            reason: u32_at(row, 40),
            attributes: u32_at(row, 52),
            name,
        });
        at += length;
    }
    Ok((cursor, records))
}

#[cfg(test)]
pub(crate) fn fixture(frn: u64, parent: u64, usn: i64, reason: u32, name: &[u16]) -> Vec<u8> {
    let len = (60 + name.len() * 2).next_multiple_of(8);
    let mut row = vec![0; len];
    row[..4].copy_from_slice(&(len as u32).to_le_bytes());
    row[4..6].copy_from_slice(&2u16.to_le_bytes());
    row[8..16].copy_from_slice(&frn.to_le_bytes());
    row[16..24].copy_from_slice(&parent.to_le_bytes());
    row[24..32].copy_from_slice(&usn.to_le_bytes());
    row[40..44].copy_from_slice(&reason.to_le_bytes());
    row[56..58].copy_from_slice(&((name.len() * 2) as u16).to_le_bytes());
    row[58..60].copy_from_slice(&60u16.to_le_bytes());
    for (i, value) in name.iter().enumerate() {
        row[60 + i * 2..62 + i * 2].copy_from_slice(&value.to_le_bytes());
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_boundary_and_raw_utf16_are_preserved() {
        let mut data = 99u64.to_le_bytes().to_vec();
        data.extend(fixture(0x1234000000000001, 5, 7, CREATE, &[0xd800, 65]));
        let (cursor, rows) = decode_batch(&data).unwrap();
        assert_eq!(cursor, 99);
        assert_eq!(rows[0].frn, 0x1234000000000001);
        assert_eq!(rows[0].name, [0xd800, 65]);
        for end in 9..data.len() {
            assert!(decode_batch(&data[..end]).is_err());
        }
        data[8 + 4..8 + 6].copy_from_slice(&3u16.to_le_bytes());
        assert!(
            decode_batch(&data)
                .unwrap_err()
                .to_string()
                .contains("version")
        );
    }
    #[test]
    fn rejects_cross_record_name_and_zero_length() {
        let mut data = 0u64.to_le_bytes().to_vec();
        data.extend(fixture(1, 5, 1, 0, &[65]));
        data[8 + 58..8 + 60].copy_from_slice(&100u16.to_le_bytes());
        assert!(decode_batch(&data).is_err());
        data[8..12].fill(0);
        assert!(decode_batch(&data).is_err());
    }
}
