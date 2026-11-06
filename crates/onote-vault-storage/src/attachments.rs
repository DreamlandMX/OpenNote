//! F19 attachment bytes on disk.
//!
//! Attachments are ordinary vault files (no YAML identity, no schema record), so
//! the vault stays readable by other Markdown tools. What this module guarantees:
//!
//! * the destination passes the same structural path rules as notes, may not be
//!   a Markdown name, and may not sit under `.opennote` or behind a reparse point;
//! * bytes are written to a staging file next to the target, flushed, read back
//!   and compared with the caller's SHA-256 *before* publication;
//! * publication never replaces an existing file (hard link, which fails when
//!   the name exists), so a concurrent or earlier file is never overwritten;
//! * a retry with the same bytes finds the published file and reports it as
//!   already stored instead of writing a copy.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use onote_domain::ErrorCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::errors::{err, SeaRes};
use crate::{pathcheck, scan};

/// What happened to one attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttachmentWrite {
    /// New bytes were published at the path.
    Stored,
    /// The path already held exactly these bytes (a retry, or a duplicate).
    AlreadyPresent,
}

/// A vault file and its content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentFile {
    pub relative_path: String,
    pub sha256: String,
    pub size: u64,
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn io_err(key: &str) -> onote_domain::ServiceError {
    err(ErrorCode::IoDenied, key, &[])
}

fn map_write(error: &std::io::Error) -> onote_domain::ServiceError {
    // StorageFull is the stable kind for ENOSPC / ERROR_DISK_FULL.
    if error.kind() == std::io::ErrorKind::StorageFull {
        return err(ErrorCode::DiskFull, "error.diskFull", &[]);
    }
    io_err("error.attachmentWriteFailed")
}

/// Every existing ancestor between the root and `target` must be a real directory.
pub(crate) fn refuse_reparse_ancestors(root: &Path, relative: &str) -> SeaRes<()> {
    let mut current = root.to_path_buf();
    if scan::is_reparse(&current) {
        return Err(io_err("error.attachmentPathReparse"));
    }
    let parts: Vec<&str> = relative.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if scan::is_reparse(&current) || !meta.is_dir() {
                    return Err(io_err("error.attachmentPathReparse"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(io_err("error.attachmentWriteFailed")),
        }
    }
    Ok(())
}

pub(crate) fn read_regular(path: &Path, limit: u64) -> SeaRes<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if scan::is_reparse(path) || !meta.is_file() {
                return Err(err(
                    ErrorCode::PathCollision,
                    "error.attachmentPathOccupied",
                    &[],
                ));
            }
            if meta.len() > limit {
                return Err(err(
                    ErrorCode::PathCollision,
                    "error.attachmentPathOccupied",
                    &[],
                ));
            }
            let mut bytes = Vec::new();
            fs::File::open(path)
                .map_err(|_| io_err("error.attachmentWriteFailed"))?
                .take(limit.saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|_| io_err("error.attachmentWriteFailed"))?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(io_err("error.attachmentWriteFailed")),
    }
}

fn staging_path(target: &Path, operation_id: &str) -> PathBuf {
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.{operation_id}.opennote-partial"))
}

/// Writes one attachment. See the module docs for the guarantees.
pub(crate) fn store(
    root: &Path,
    relative_path: &str,
    bytes: &[u8],
    expected_sha256: &str,
    operation_id: &str,
) -> SeaRes<(String, AttachmentWrite)> {
    let canonical = pathcheck::validate_attachment_path(relative_path)?;
    let expected = expected_sha256.to_ascii_lowercase();
    if hex_sha256(bytes) != expected {
        // The caller's bytes changed between hashing and writing.
        return Err(err(
            ErrorCode::InvalidInput,
            "error.attachmentSourceChanged",
            &[],
        ));
    }
    refuse_reparse_ancestors(root, &canonical)?;
    let target = root.join(&canonical);
    let limit = bytes.len() as u64;
    if let Some(existing) = read_regular(&target, limit)? {
        if hex_sha256(&existing) == expected {
            return Ok((canonical, AttachmentWrite::AlreadyPresent));
        }
        return Err(err(
            ErrorCode::PathCollision,
            "error.attachmentPathOccupied",
            &[],
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| map_write(&error))?;
    }
    // Re-check after creating folders: nothing may have been swapped in.
    refuse_reparse_ancestors(root, &canonical)?;

    let staging = staging_path(&target, operation_id);
    let _ = fs::remove_file(&staging);
    let outcome = (|| -> SeaRes<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| map_write(&error))?;
        file.write_all(bytes).map_err(|error| map_write(&error))?;
        file.sync_all().map_err(|error| map_write(&error))?;
        drop(file);
        let written = read_regular(&staging, limit)?.unwrap_or_default();
        if hex_sha256(&written) != expected {
            return Err(io_err("error.attachmentVerifyFailed"));
        }
        // A hard link publishes atomically and fails if the name exists, so a
        // file that appeared meanwhile is never replaced.
        fs::hard_link(&staging, &target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                err(
                    ErrorCode::PathCollision,
                    "error.attachmentPathOccupied",
                    &[],
                )
            } else {
                map_write(&error)
            }
        })
    })();
    let _ = fs::remove_file(&staging);
    match outcome {
        Ok(()) => Ok((canonical, AttachmentWrite::Stored)),
        Err(error) => {
            // A racing writer may have published the same bytes; that is success.
            if let Ok(Some(existing)) = read_regular(&target, limit) {
                if hex_sha256(&existing) == expected {
                    return Ok((canonical, AttachmentWrite::AlreadyPresent));
                }
            }
            Err(error)
        }
    }
}

/// Largest slice one read may return, so a big file crosses IPC in chunks.
pub const MAX_ATTACHMENT_CHUNK: u64 = 8 * 1024 * 1024;

/// Bytes kept from the start of a file so the caller can detect its type.
const ATTACHMENT_HEAD_BYTES: usize = 4096;

/// What a reader needs before asking for bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentProbe {
    pub file: AttachmentFile,
    /// The first bytes of the file, for type detection by content.
    pub head: Vec<u8>,
}

/// Opens a vault attachment for reading. The same path rules as writing apply:
/// no Markdown, nothing under `.opennote`, nothing behind a reparse point.
fn open_for_read(root: &Path, relative_path: &str) -> SeaRes<(String, fs::File, u64)> {
    let canonical = pathcheck::validate_attachment_path(relative_path)?;
    refuse_reparse_ancestors(root, &canonical)?;
    let target = root.join(&canonical);
    let meta = match fs::symlink_metadata(&target) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(err(
                ErrorCode::ObjectNotFound,
                "error.attachmentMissing",
                &[],
            ));
        }
        Err(_) => return Err(io_err("error.attachmentReadFailed")),
    };
    if scan::is_reparse(&target) || !meta.is_file() {
        return Err(io_err("error.attachmentPathReparse"));
    }
    let file = fs::File::open(&target).map_err(|_| io_err("error.attachmentReadFailed"))?;
    Ok((canonical, file, meta.len()))
}

/// Read-only access to the attachments of one vault.
#[derive(Debug, Clone)]
pub struct AttachmentReader {
    root: PathBuf,
}

impl AttachmentReader {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Size, hash and leading bytes of one attachment.
    pub fn probe(&self, relative_path: &str) -> SeaRes<AttachmentProbe> {
        probe(&self.root, relative_path)
    }

    /// One slice of an attachment, at most [`MAX_ATTACHMENT_CHUNK`] bytes.
    pub fn read_chunk(&self, relative_path: &str, offset: u64, length: u64) -> SeaRes<Vec<u8>> {
        read_chunk(&self.root, relative_path, offset, length)
    }
}

fn probe(root: &Path, relative_path: &str) -> SeaRes<AttachmentProbe> {
    let (canonical, mut file, _) = open_for_read(root, relative_path)?;
    let mut hasher = Sha256::new();
    let mut head = Vec::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| io_err("error.attachmentReadFailed"))?;
        if read == 0 {
            break;
        }
        if head.len() < ATTACHMENT_HEAD_BYTES {
            let room = ATTACHMENT_HEAD_BYTES - head.len();
            head.extend_from_slice(&buffer[..read.min(room)]);
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok(AttachmentProbe {
        file: AttachmentFile {
            relative_path: canonical,
            sha256: hex::encode(hasher.finalize()),
            size,
        },
        head,
    })
}

fn read_chunk(root: &Path, relative_path: &str, offset: u64, length: u64) -> SeaRes<Vec<u8>> {
    let (_, mut file, size) = open_for_read(root, relative_path)?;
    if offset >= size || length == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| io_err("error.attachmentReadFailed"))?;
    let mut bytes = Vec::new();
    file.take(length.min(MAX_ATTACHMENT_CHUNK))
        .read_to_end(&mut bytes)
        .map_err(|_| io_err("error.attachmentReadFailed"))?;
    Ok(bytes)
}

/// Lists regular, non-Markdown files directly inside `folder` with their hashes,
/// so the planner can reuse identical bytes and rename around different ones.
/// Files larger than `limit` are listed by name with an empty hash (they still
/// occupy the name but are not read).
pub(crate) fn list_folder(root: &Path, folder: &str, limit: u64) -> SeaRes<Vec<AttachmentFile>> {
    let trimmed = folder.trim().replace('\\', "/");
    let trimmed = trimmed.trim_matches('/');
    let directory = if trimmed.is_empty() {
        root.to_path_buf()
    } else {
        // Validate as if it were a file inside the folder.
        let probe = pathcheck::validate_attachment_path(&format!("{trimmed}/probe.bin"))?;
        refuse_reparse_ancestors(root, &probe)?;
        root.join(trimmed)
    };
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(io_err("error.attachmentWriteFailed")),
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if scan::is_reparse(&path) || !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".md") || lower.ends_with(".opennote-partial") || name.starts_with('.') {
            continue;
        }
        let relative = if trimmed.is_empty() {
            name
        } else {
            format!("{trimmed}/{name}")
        };
        let sha256 = if meta.len() <= limit {
            match read_regular(&path, limit) {
                Ok(Some(bytes)) => hex_sha256(&bytes),
                _ => String::new(),
            }
        } else {
            String::new()
        };
        files.push(AttachmentFile {
            relative_path: relative,
            sha256,
            size: meta.len(),
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

// ---------------------------------------------------------------------------
// Structured vault documents (`.canvas`, `.onview.json`).
//
// These are ordinary UTF-8 files owned by the vault, not notes: no YAML identity
// and no schema record, so the on-disk vault format does not change. A save is a
// compare-and-swap on the content hash the editor loaded:
//
// * the same bytes already on disk  -> success, nothing written (idempotent retry);
// * file still has the loaded hash  -> verified staging file atomically replaces it;
// * anything else (edited elsewhere, deleted, created meanwhile) -> VersionConflict,
//   nothing written, so a newer external edit is never overwritten.
// ---------------------------------------------------------------------------

/// Largest structured document read or written in one piece.
pub const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DOCUMENT_LISTING: usize = 10_000;

/// A structured document read from the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDocument {
    pub relative_path: String,
    pub source: String,
    pub sha256: String,
}

fn validate_document_path(raw: &str, suffix: &str) -> SeaRes<String> {
    let canonical = pathcheck::validate_attachment_path(raw)?;
    if !canonical.to_ascii_lowercase().ends_with(suffix) {
        return Err(err(ErrorCode::InvalidInput, "error.documentWrongType", &[]));
    }
    Ok(canonical)
}

fn conflict() -> onote_domain::ServiceError {
    err(ErrorCode::VersionConflict, "error.documentChanged", &[])
}

pub(crate) fn read_document(
    root: &Path,
    relative_path: &str,
    suffix: &str,
) -> SeaRes<VaultDocument> {
    let canonical = validate_document_path(relative_path, suffix)?;
    refuse_reparse_ancestors(root, &canonical)?;
    let bytes = read_regular(&root.join(&canonical), MAX_DOCUMENT_BYTES)?
        .ok_or_else(|| err(ErrorCode::ObjectNotFound, "error.documentMissing", &[]))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(err(ErrorCode::InvalidInput, "error.documentTooLarge", &[]));
    }
    let sha256 = hex_sha256(&bytes);
    let source = String::from_utf8(bytes)
        .map_err(|_| err(ErrorCode::InvalidInput, "error.documentEncoding", &[]))?;
    Ok(VaultDocument {
        relative_path: canonical,
        source,
        sha256,
    })
}

/// Compare-and-swap save; see the section docs. Returns the path and new hash.
pub(crate) fn save_document(
    root: &Path,
    relative_path: &str,
    suffix: &str,
    source: &str,
    expected_sha256: Option<&str>,
    operation_id: &str,
) -> SeaRes<(String, String, AttachmentWrite)> {
    let canonical = validate_document_path(relative_path, suffix)?;
    compare_and_swap(
        root,
        canonical,
        source.as_bytes(),
        expected_sha256,
        operation_id,
        MAX_DOCUMENT_BYTES,
    )
}

/// The compare-and-swap behind `save_document`, for an already validated
/// vault-relative path. `limit` bounds both the new bytes and the file read
/// back from disk; a larger file on disk counts as a conflict, never a read.
pub(crate) fn compare_and_swap(
    root: &Path,
    canonical: String,
    bytes: &[u8],
    expected_sha256: Option<&str>,
    operation_id: &str,
    limit: u64,
) -> SeaRes<(String, String, AttachmentWrite)> {
    if bytes.len() as u64 > limit {
        return Err(err(ErrorCode::InvalidInput, "error.documentTooLarge", &[]));
    }
    let new_sha = hex_sha256(bytes);
    refuse_reparse_ancestors(root, &canonical)?;
    let target = root.join(&canonical);
    let current = match read_regular(&target, limit) {
        Ok(value) => value,
        // Something that is not a readable regular file of bounded size.
        Err(_) => return Err(conflict()),
    };
    match (&current, expected_sha256) {
        (Some(existing), _) if hex_sha256(existing) == new_sha => {
            return Ok((canonical, new_sha, AttachmentWrite::AlreadyPresent));
        }
        (None, None) => {
            let (path, write) = store(root, &canonical, bytes, &new_sha, operation_id)?;
            return Ok((path, new_sha, write));
        }
        (Some(existing), Some(expected))
            if hex_sha256(existing) == expected.to_ascii_lowercase() => {}
        _ => return Err(conflict()),
    }

    let staging = staging_path(&target, operation_id);
    let _ = fs::remove_file(&staging);
    let outcome = (|| -> SeaRes<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| map_write(&error))?;
        file.write_all(bytes).map_err(|error| map_write(&error))?;
        file.sync_all().map_err(|error| map_write(&error))?;
        drop(file);
        let written = read_regular(&staging, bytes.len() as u64)?.unwrap_or_default();
        if hex_sha256(&written) != new_sha {
            return Err(io_err("error.attachmentVerifyFailed"));
        }
        // Last check before the atomic replace; the writer gate excludes other
        // app writers, so only an external editor could still race here.
        let still = read_regular(&target, limit).ok().flatten();
        let expected = expected_sha256.unwrap_or_default().to_ascii_lowercase();
        if still.map(|value| hex_sha256(&value)) != Some(expected) {
            return Err(conflict());
        }
        fs::rename(&staging, &target).map_err(|error| map_write(&error))
    })();
    let _ = fs::remove_file(&staging);
    outcome.map(|()| (canonical, new_sha, AttachmentWrite::Stored))
}

/// Every file with `suffix` in the vault (outside `.opennote`), sorted.
pub(crate) fn list_documents(root: &Path, suffix: &str) -> SeaRes<Vec<AttachmentFile>> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if scan::is_reparse(&path) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                // `.opennote`, staging files and other hidden entries.
                continue;
            }
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if meta.is_dir() {
                stack.push((path, relative));
            } else if meta.is_file() && name.to_ascii_lowercase().ends_with(suffix) {
                if found.len() == MAX_DOCUMENT_LISTING {
                    return Err(err(ErrorCode::InvalidInput, "error.documentListLimit", &[]));
                }
                found.push(AttachmentFile {
                    relative_path: relative,
                    sha256: String::new(),
                    size: meta.len(),
                });
            }
        }
    }
    found.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(found)
}
