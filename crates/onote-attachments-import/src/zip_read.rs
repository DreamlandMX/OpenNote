//! F24: reads a ZIP the user chose, one entry at a time.
//!
//! Only the central directory stays in memory, so a large archive does not have to
//! fit in RAM. Sizes in the directory are claims: every entry is inflated under a
//! limit, then its length and CRC-32 are checked against those claims, so a
//! compression bomb or a damaged archive fails instead of filling memory or the
//! vault. Nothing here writes; deciding where an entry may go is the planner's job.

use std::io::{Read, Seek, SeekFrom};

use flate2::read::DeflateDecoder;
use flate2::Crc;

const EOCD: u32 = 0x0605_4b50;
const EOCD64: u32 = 0x0606_4b50;
const EOCD64_LOCATOR: u32 = 0x0706_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;
const ZIP64_FIELD: u16 = 0x0001;
const UTF8_FLAG: u16 = 1 << 11;
/// Unix `st_mode` type bits, kept in the top half of the external attributes.
const UNIX_FILE_TYPE: u32 = 0o170000 << 16;
const UNIX_SYMLINK: u32 = 0o120000 << 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipError {
    /// Not a ZIP this reader accepts, or a damaged one.
    Malformed(&'static str),
    /// More entries than the caller allows.
    TooManyEntries,
    /// Spanned, encrypted or an unsupported compression method.
    Unsupported(&'static str),
    /// Larger than the limit, or not the size the archive claimed.
    Size,
    /// The inflated bytes do not match the recorded CRC-32.
    Checksum,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    /// The stored name, with `/` separators. Lossy when `name_is_utf8` is false.
    pub name: String,
    /// False for a name in a legacy code page, which this build will not guess at.
    pub name_is_utf8: bool,
    pub size: u64,
    pub compressed: u64,
    pub crc: u32,
    pub method: u16,
    pub local_offset: u64,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub encrypted: bool,
}

pub struct ZipReader<R> {
    source: R,
    length: u64,
    entries: Vec<ZipEntry>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    let slice = bytes.get(at..at.checked_add(8)?)?;
    let mut value = [0u8; 8];
    value.copy_from_slice(slice);
    Some(u64::from_le_bytes(value))
}

const TRUNCATED: ZipError = ZipError::Malformed("truncated");

fn read_at<R: Read + Seek>(source: &mut R, at: u64, buffer: &mut [u8]) -> Result<(), ZipError> {
    source.seek(SeekFrom::Start(at)).map_err(|_| ZipError::Io)?;
    source.read_exact(buffer).map_err(|_| TRUNCATED)
}

impl<R: Read + Seek> ZipReader<R> {
    /// Reads the central directory. At most `max_entries` entries are accepted.
    pub fn open(mut source: R, max_entries: usize) -> Result<Self, ZipError> {
        let length = source.seek(SeekFrom::End(0)).map_err(|_| ZipError::Io)?;
        // The end record sits within the last 22 + 65535 bytes (comment length).
        let tail_length = length.min(22 + 0xFFFF) as usize;
        let tail_start = length - tail_length as u64;
        let mut tail = vec![0u8; tail_length];
        read_at(&mut source, tail_start, &mut tail)?;
        let at = (0..tail_length.saturating_sub(21))
            .rev()
            .find(|&at| u32_at(&tail, at) == Some(EOCD))
            .ok_or(ZipError::Malformed("no end record"))?;
        if u16_at(&tail, at + 4) != Some(0) || u16_at(&tail, at + 6) != Some(0) {
            return Err(ZipError::Unsupported("spanned"));
        }
        let mut count = u64::from(u16_at(&tail, at + 10).ok_or(TRUNCATED)?);
        let mut directory_size = u64::from(u32_at(&tail, at + 12).ok_or(TRUNCATED)?);
        let mut directory_at = u64::from(u32_at(&tail, at + 16).ok_or(TRUNCATED)?);
        if count == 0xFFFF || directory_size == 0xFFFF_FFFF || directory_at == 0xFFFF_FFFF {
            // ZIP64: the locator sits just before the end record.
            let locator_at = (tail_start + at as u64)
                .checked_sub(20)
                .ok_or(ZipError::Malformed("no zip64 locator"))?;
            let mut locator = [0u8; 20];
            read_at(&mut source, locator_at, &mut locator)?;
            if u32_at(&locator, 0) != Some(EOCD64_LOCATOR) {
                return Err(ZipError::Malformed("no zip64 locator"));
            }
            let record_at = u64_at(&locator, 8).ok_or(TRUNCATED)?;
            let mut record = [0u8; 56];
            read_at(&mut source, record_at, &mut record)?;
            if u32_at(&record, 0) != Some(EOCD64) {
                return Err(ZipError::Malformed("no zip64 record"));
            }
            count = u64_at(&record, 32).ok_or(TRUNCATED)?;
            directory_size = u64_at(&record, 40).ok_or(TRUNCATED)?;
            directory_at = u64_at(&record, 48).ok_or(TRUNCATED)?;
        }
        if count > max_entries as u64 {
            return Err(ZipError::TooManyEntries);
        }
        if directory_at
            .checked_add(directory_size)
            .is_none_or(|end| end > length)
        {
            return Err(ZipError::Malformed("directory outside the file"));
        }
        let mut directory = vec![0u8; directory_size as usize];
        read_at(&mut source, directory_at, &mut directory)?;
        let entries = parse_directory(&directory, count as usize, directory_at)?;
        Ok(Self {
            source,
            length,
            entries,
        })
    }

    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// Inflates one entry, at most `limit` bytes, and verifies it.
    pub fn read(&mut self, index: usize, limit: u64) -> Result<Vec<u8>, ZipError> {
        let entry = self
            .entries
            .get(index)
            .ok_or(ZipError::Malformed("entry"))?
            .clone();
        if entry.encrypted {
            return Err(ZipError::Unsupported("encrypted"));
        }
        if entry.size > limit {
            return Err(ZipError::Size);
        }
        let mut header = [0u8; 30];
        read_at(&mut self.source, entry.local_offset, &mut header)?;
        if u32_at(&header, 0) != Some(LOCAL) {
            return Err(ZipError::Malformed("local header"));
        }
        let name_length = u64::from(u16_at(&header, 26).ok_or(TRUNCATED)?);
        let extra_length = u64::from(u16_at(&header, 28).ok_or(TRUNCATED)?);
        let start = entry.local_offset + 30 + name_length + extra_length;
        if start
            .checked_add(entry.compressed)
            .is_none_or(|end| end > self.length)
        {
            return Err(ZipError::Malformed("data outside the file"));
        }
        self.source
            .seek(SeekFrom::Start(start))
            .map_err(|_| ZipError::Io)?;
        let packed = (&mut self.source).take(entry.compressed);
        let mut output = Vec::with_capacity(entry.size.min(1 << 24) as usize);
        match entry.method {
            0 => {
                packed
                    .take(limit + 1)
                    .read_to_end(&mut output)
                    .map_err(|_| ZipError::Io)?;
            }
            8 => {
                DeflateDecoder::new(packed)
                    .take(limit + 1)
                    .read_to_end(&mut output)
                    .map_err(|_| ZipError::Malformed("deflate"))?;
            }
            _ => return Err(ZipError::Unsupported("compression method")),
        }
        if output.len() as u64 != entry.size {
            return Err(ZipError::Size);
        }
        let mut check = Crc::new();
        check.update(&output);
        if check.sum() != entry.crc {
            return Err(ZipError::Checksum);
        }
        Ok(output)
    }
}

fn parse_directory(
    directory: &[u8],
    count: usize,
    directory_at: u64,
) -> Result<Vec<ZipEntry>, ZipError> {
    let mut entries = Vec::with_capacity(count.min(1 << 16));
    let mut at = 0usize;
    for _ in 0..count {
        if u32_at(directory, at) != Some(CENTRAL) {
            return Err(ZipError::Malformed("central header"));
        }
        let flags = u16_at(directory, at + 8).ok_or(TRUNCATED)?;
        let method = u16_at(directory, at + 10).ok_or(TRUNCATED)?;
        let crc = u32_at(directory, at + 16).ok_or(TRUNCATED)?;
        let mut compressed = u64::from(u32_at(directory, at + 20).ok_or(TRUNCATED)?);
        let mut size = u64::from(u32_at(directory, at + 24).ok_or(TRUNCATED)?);
        let name_length = usize::from(u16_at(directory, at + 28).ok_or(TRUNCATED)?);
        let extra_length = usize::from(u16_at(directory, at + 30).ok_or(TRUNCATED)?);
        let comment_length = usize::from(u16_at(directory, at + 32).ok_or(TRUNCATED)?);
        let attributes = u32_at(directory, at + 38).ok_or(TRUNCATED)?;
        let mut local_offset = u64::from(u32_at(directory, at + 42).ok_or(TRUNCATED)?);
        let name_start = at + 46;
        let extra_start = name_start + name_length;
        let raw_name = directory.get(name_start..extra_start).ok_or(TRUNCATED)?;
        let extra = directory
            .get(extra_start..extra_start + extra_length)
            .ok_or(TRUNCATED)?;

        // ZIP64 values appear in the extra field, only for fields marked as full.
        let mut cursor = 0usize;
        while let (Some(id), Some(length)) = (u16_at(extra, cursor), u16_at(extra, cursor + 2)) {
            let body = cursor + 4;
            if id == ZIP64_FIELD {
                let mut next = body;
                for field in [&mut size, &mut compressed, &mut local_offset] {
                    if *field == u64::from(u32::MAX) {
                        *field = u64_at(extra, next).ok_or(TRUNCATED)?;
                        next += 8;
                    }
                }
            }
            cursor = body + usize::from(length);
        }
        if local_offset >= directory_at {
            return Err(ZipError::Malformed("entry after directory"));
        }

        let (name, valid) = match std::str::from_utf8(raw_name) {
            Ok(text) => (text.to_owned(), true),
            Err(_) => (String::from_utf8_lossy(raw_name).into_owned(), false),
        };
        let name = name.replace('\\', "/");
        entries.push(ZipEntry {
            is_directory: name.ends_with('/'),
            is_symlink: attributes & UNIX_FILE_TYPE == UNIX_SYMLINK,
            encrypted: flags & 1 != 0,
            name_is_utf8: valid && (flags & UTF8_FLAG != 0 || raw_name.is_ascii() || valid),
            name,
            size,
            compressed,
            crc,
            method,
            local_offset,
        });
        at = extra_start + extra_length + comment_length;
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    use flate2::write::DeflateEncoder;
    use flate2::Compression;

    struct Item<'a> {
        name: &'a [u8],
        data: &'a [u8],
        deflate: bool,
        flags: u16,
        attributes: u32,
    }

    fn item<'a>(name: &'a str, data: &'a [u8]) -> Item<'a> {
        Item {
            name: name.as_bytes(),
            data,
            deflate: true,
            flags: UTF8_FLAG,
            attributes: 0,
        }
    }

    fn build(items: &[Item]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for item in items {
            let packed = if item.deflate {
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(item.data).unwrap();
                encoder.finish().unwrap()
            } else {
                item.data.to_vec()
            };
            let mut crc = Crc::new();
            crc.update(item.data);
            let method: u16 = if item.deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend_from_slice(&LOCAL.to_le_bytes());
            out.extend_from_slice(&[20, 0]);
            out.extend_from_slice(&item.flags.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&crc.sum().to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(item.data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(item.name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(item.name);
            out.extend_from_slice(&packed);

            central.extend_from_slice(&CENTRAL.to_le_bytes());
            central.extend_from_slice(&[20, 3, 20, 0]);
            central.extend_from_slice(&item.flags.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&crc.sum().to_le_bytes());
            central.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            central.extend_from_slice(&(item.data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(item.name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 6]);
            central.extend_from_slice(&[0; 2]);
            central.extend_from_slice(&item.attributes.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(item.name);
        }
        let directory_at = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(items.len() as u16).to_le_bytes());
        out.extend_from_slice(&(items.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&directory_at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn stored_and_deflated_entries_round_trip() {
        let body = "你好 ".repeat(1000);
        let bytes = build(&[
            item("notes/甲.md", body.as_bytes()),
            Item {
                deflate: false,
                ..item("a.png", b"\x89PNG")
            },
            item("empty/", b""),
        ]);
        let mut zip = ZipReader::open(Cursor::new(bytes), 10).unwrap();
        assert_eq!(zip.entries()[0].name, "notes/甲.md");
        assert!(zip.entries()[2].is_directory);
        assert_eq!(zip.read(0, 1 << 20).unwrap(), body.as_bytes());
        assert_eq!(zip.read(1, 16).unwrap(), b"\x89PNG");
    }

    #[test]
    fn a_comment_after_the_end_record_is_tolerated() {
        let mut bytes = build(&[item("a.md", b"x")]);
        let length = bytes.len();
        bytes[length - 2..].copy_from_slice(&5u16.to_le_bytes());
        bytes.extend_from_slice(b"hello");
        assert_eq!(
            ZipReader::open(Cursor::new(bytes), 10)
                .unwrap()
                .entries()
                .len(),
            1
        );
    }

    #[test]
    fn an_entry_over_the_limit_is_refused_before_inflating() {
        let data = vec![0u8; 1 << 20];
        let mut zip = ZipReader::open(Cursor::new(build(&[item("bomb.md", &data)])), 10).unwrap();
        assert_eq!(zip.read(0, 1024), Err(ZipError::Size));
    }

    #[test]
    fn a_size_that_lies_cannot_inflate_past_the_limit() {
        let data = vec![0u8; 1 << 20];
        let mut bytes = build(&[item("bomb.md", &data)]);
        // Rewrite the directory's claim to look harmless.
        let directory = bytes
            .windows(4)
            .rposition(|window| window == CENTRAL.to_le_bytes())
            .unwrap();
        bytes[directory + 24..directory + 28].copy_from_slice(&16u32.to_le_bytes());
        let mut zip = ZipReader::open(Cursor::new(bytes), 10).unwrap();
        assert_eq!(zip.read(0, 1024), Err(ZipError::Size));
    }

    #[test]
    fn a_damaged_entry_fails_its_checksum() {
        let mut bytes = build(&[Item {
            deflate: false,
            ..item("a.md", b"hello")
        }]);
        let at = bytes
            .windows(5)
            .position(|window| window == b"hello")
            .unwrap();
        bytes[at] = b'j';
        let mut zip = ZipReader::open(Cursor::new(bytes), 10).unwrap();
        assert_eq!(zip.read(0, 64), Err(ZipError::Checksum));
    }

    #[test]
    fn too_many_entries_are_refused() {
        let bytes = build(&[item("a.md", b"x"), item("b.md", b"y")]);
        assert_eq!(
            ZipReader::open(Cursor::new(bytes), 1).err(),
            Some(ZipError::TooManyEntries)
        );
    }

    #[test]
    fn links_encryption_and_legacy_names_are_marked() {
        let bytes = build(&[
            Item {
                attributes: 0o120777 << 16,
                ..item("link.md", b"/etc/passwd")
            },
            Item {
                flags: 1,
                ..item("secret.md", b"x")
            },
            Item {
                name: b"\xc4\xe3\xba\xc3.md",
                flags: 0,
                ..item("", b"x")
            },
        ]);
        let mut zip = ZipReader::open(Cursor::new(bytes), 10).unwrap();
        assert!(zip.entries()[0].is_symlink);
        assert!(zip.entries()[1].encrypted);
        assert!(!zip.entries()[2].name_is_utf8);
        assert_eq!(zip.read(1, 64), Err(ZipError::Unsupported("encrypted")));
    }

    #[test]
    fn every_truncated_prefix_fails_without_panicking() {
        let bytes = build(&[item("a.md", b"hello world"), item("b.md", b"x")]);
        for length in 0..bytes.len() {
            let prefix = bytes[..length].to_vec();
            if let Ok(mut zip) = ZipReader::open(Cursor::new(prefix), 10) {
                let _ = zip.read(0, 64);
            }
        }
    }

    #[test]
    fn zip64_directory_values_are_read() {
        // A ZIP64 record pointing at an ordinary directory, as large exports have.
        let mut bytes = build(&[item("a.md", b"hello")]);
        let eocd = bytes
            .windows(4)
            .rposition(|window| window == EOCD.to_le_bytes())
            .unwrap();
        let directory_size = u32_at(&bytes, eocd + 12).unwrap();
        let directory_at = u32_at(&bytes, eocd + 16).unwrap();
        bytes.truncate(eocd);
        let record_at = bytes.len() as u64;
        bytes.extend_from_slice(&EOCD64.to_le_bytes());
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&u64::from(directory_size).to_le_bytes());
        bytes.extend_from_slice(&u64::from(directory_at).to_le_bytes());
        bytes.extend_from_slice(&EOCD64_LOCATOR.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&record_at.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&EOCD.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&0xFFFFu16.to_le_bytes());
        bytes.extend_from_slice(&0xFFFFu16.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        let mut zip = ZipReader::open(Cursor::new(bytes), 10).unwrap();
        assert_eq!(zip.read(0, 64).unwrap(), b"hello");
    }
}
