//! Extracts the one expected file from the engine archive.
//!
//! The archive's own SHA-256 is checked before this runs, so this reader only
//! has to be strict, not general: exactly one central-directory entry with the
//! expected name, stored or deflated, no encryption, no ZIP64, sizes within the
//! limit, local header consistent with the central one, CRC-32 and size checked
//! after inflating. The caller then checks the extracted file's SHA-256.

use std::io::Read;

use flate2::read::DeflateDecoder;
use flate2::Crc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// The bytes are not a ZIP this reader accepts.
    Malformed(&'static str),
    /// Not exactly one entry, or not the expected one.
    UnexpectedEntries,
    /// Larger than the limit, or the sizes disagree.
    Size,
    /// The inflated bytes do not match the recorded CRC-32.
    Checksum,
}

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, ArchiveError> {
    bytes
        .get(at..at + 2)
        .map(|value| u16::from_le_bytes([value[0], value[1]]))
        .ok_or(ArchiveError::Malformed("truncated"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, ArchiveError> {
    bytes
        .get(at..at + 4)
        .map(|value| u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .ok_or(ArchiveError::Malformed("truncated"))
}

/// Returns the bytes of `name`, the archive's only entry, at most `limit` long.
pub fn extract_single(archive: &[u8], name: &str, limit: u64) -> Result<Vec<u8>, ArchiveError> {
    // The end record is within the last 22 + 65535 bytes (comment length).
    let search_from = archive.len().saturating_sub(22 + 0xFFFF);
    let eocd = (search_from..archive.len().saturating_sub(21))
        .rev()
        .find(|&at| u32_at(archive, at) == Ok(EOCD))
        .ok_or(ArchiveError::Malformed("no end record"))?;
    if u16_at(archive, eocd + 4)? != 0 || u16_at(archive, eocd + 6)? != 0 {
        return Err(ArchiveError::Malformed("multi-disk"));
    }
    let entries_here = u16_at(archive, eocd + 8)?;
    let entries_total = u16_at(archive, eocd + 10)?;
    if entries_here != 1 || entries_total != 1 {
        return Err(ArchiveError::UnexpectedEntries);
    }
    let central = u32_at(archive, eocd + 16)? as usize;

    if u32_at(archive, central)? != CENTRAL {
        return Err(ArchiveError::Malformed("central header"));
    }
    let flags = u16_at(archive, central + 8)?;
    let method = u16_at(archive, central + 10)?;
    let crc = u32_at(archive, central + 16)?;
    let compressed = u32_at(archive, central + 20)?;
    let uncompressed = u32_at(archive, central + 24)?;
    let name_length = u16_at(archive, central + 28)? as usize;
    let local = u32_at(archive, central + 42)? as usize;
    let central_name = archive
        .get(central + 46..central + 46 + name_length)
        .ok_or(ArchiveError::Malformed("truncated"))?;
    if central_name != name.as_bytes() {
        return Err(ArchiveError::UnexpectedEntries);
    }
    if flags & 0x0001 != 0 {
        return Err(ArchiveError::Malformed("encrypted"));
    }
    if compressed == u32::MAX || uncompressed == u32::MAX {
        return Err(ArchiveError::Malformed("zip64"));
    }
    if u64::from(uncompressed) > limit {
        return Err(ArchiveError::Size);
    }

    if u32_at(archive, local)? != LOCAL {
        return Err(ArchiveError::Malformed("local header"));
    }
    let local_method = u16_at(archive, local + 8)?;
    let local_name_length = u16_at(archive, local + 26)? as usize;
    let local_extra = u16_at(archive, local + 28)? as usize;
    let local_name = archive
        .get(local + 30..local + 30 + local_name_length)
        .ok_or(ArchiveError::Malformed("truncated"))?;
    if local_method != method || local_name != name.as_bytes() {
        return Err(ArchiveError::Malformed("local header disagrees"));
    }
    let data_start = local + 30 + local_name_length + local_extra;
    let data = archive
        .get(data_start..data_start + compressed as usize)
        .ok_or(ArchiveError::Malformed("truncated data"))?;
    if data_start + compressed as usize > central {
        return Err(ArchiveError::Malformed("data overlaps directory"));
    }

    let output = match method {
        0 => data.to_vec(),
        8 => {
            let mut output = Vec::with_capacity(uncompressed as usize);
            DeflateDecoder::new(data)
                .take(limit + 1)
                .read_to_end(&mut output)
                .map_err(|_| ArchiveError::Malformed("deflate"))?;
            output
        }
        _ => return Err(ArchiveError::Malformed("method")),
    };
    if output.len() as u64 != u64::from(uncompressed) || output.len() as u64 > limit {
        return Err(ArchiveError::Size);
    }
    let mut check = Crc::new();
    check.update(&output);
    if check.sum() != crc {
        return Err(ArchiveError::Checksum);
    }
    Ok(output)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    /// Builds a ZIP with the given (name, content, deflate) entries.
    pub(crate) fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, content, deflate) in entries {
            let data = if *deflate {
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(content).unwrap();
                encoder.finish().unwrap()
            } else {
                content.to_vec()
            };
            let mut crc = Crc::new();
            crc.update(content);
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend_from_slice(&LOCAL.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&crc.sum().to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(content.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&data);
            central.extend_from_slice(&CENTRAL.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&crc.sum().to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(content.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let directory = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&directory.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn the_expected_entry_is_extracted_stored_or_deflated() {
        let content = b"MZ engine bytes".repeat(100);
        for deflate in [false, true] {
            let archive = zip(&[("tectonic.exe", &content, deflate)]);
            assert_eq!(
                extract_single(&archive, "tectonic.exe", 1 << 20).unwrap(),
                content
            );
        }
    }

    #[test]
    fn anything_but_exactly_the_expected_entry_is_refused() {
        let two = zip(&[("tectonic.exe", b"a", false), ("evil.dll", b"b", false)]);
        assert_eq!(
            extract_single(&two, "tectonic.exe", 100),
            Err(ArchiveError::UnexpectedEntries)
        );
        let renamed = zip(&[("../tectonic.exe", b"a", false)]);
        assert_eq!(
            extract_single(&renamed, "tectonic.exe", 100),
            Err(ArchiveError::UnexpectedEntries)
        );
    }

    #[test]
    fn oversize_corrupt_and_truncated_archives_are_refused() {
        let archive = zip(&[("tectonic.exe", &[7u8; 4096], true)]);
        assert_eq!(
            extract_single(&archive, "tectonic.exe", 100),
            Err(ArchiveError::Size)
        );
        let mut corrupt = archive.clone();
        // Flip a byte of the stored CRC in the central directory.
        let central = u32_at(&corrupt, corrupt.len() - 6).unwrap() as usize;
        corrupt[central + 16] ^= 0xFF;
        assert_eq!(
            extract_single(&corrupt, "tectonic.exe", 1 << 20),
            Err(ArchiveError::Checksum)
        );
        assert!(extract_single(&archive[..archive.len() - 10], "tectonic.exe", 1 << 20).is_err());
        assert!(extract_single(b"not a zip", "tectonic.exe", 1 << 20).is_err());
    }
}
