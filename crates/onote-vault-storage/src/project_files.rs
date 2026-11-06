//! M13 LaTeX project files on disk (F45).
//!
//! A LaTeX project is an ordinary vault folder: `.tex`, `.bib`, images and the
//! rest stay plain files with no YAML identity, so other TeX tools can still
//! open the project. What this module adds on top of the shared path rules:
//!
//! * enumeration is bounded (file count, single file, total bytes) and refuses
//!   instead of truncating, so a build never silently misses a file;
//! * a reparse point anywhere inside the project is refused, not skipped,
//!   for the same reason;
//! * a source save is the same compare-and-swap as structured documents, limited
//!   to text source extensions and valid UTF-8, never rewritten lossily;
//! * the project record lives at `.opennote/latex/<projectId>/project.json` and
//!   is replaced atomically against the hash it was read with.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use onote_domain::ErrorCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::attachments::{self, hex_sha256, AttachmentWrite};
use crate::errors::{err, SeaRes};
use crate::{pathcheck, scan, state};

/// Most files one project may contain.
pub const MAX_PROJECT_FILES: usize = 4096;
/// Largest single project file.
pub const MAX_PROJECT_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Largest project in total.
pub const MAX_PROJECT_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// Largest project record.
const MAX_RECORD_BYTES: u64 = attachments::MAX_DOCUMENT_BYTES;

/// Extensions a source save may write. Everything else in a project (images,
/// fonts) is read-only input here.
pub const PROJECT_SOURCE_EXTENSIONS: &[&str] = &[".tex", ".bib", ".sty", ".cls", ".bst"];

/// One file inside a project folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileEntry {
    /// Vault-relative path with `/` separators.
    pub relative_path: String,
    pub size: u64,
    pub sha256: String,
}

/// The bytes of one project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFileBytes {
    pub relative_path: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// A stored project record and the hash it was read with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecordBytes {
    pub bytes: Vec<u8>,
    pub sha256: String,
}

fn denied(key: &str) -> onote_domain::ServiceError {
    err(ErrorCode::IoDenied, key, &[])
}

fn invalid(key: &str) -> onote_domain::ServiceError {
    err(ErrorCode::InvalidInput, key, &[])
}

/// The canonical project folder; empty means the vault root.
fn project_folder(raw: &str) -> SeaRes<String> {
    let trimmed = raw.trim().replace('\\', "/");
    let trimmed = trimmed.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    pathcheck::validate_vault_relative_path(trimmed)
}

fn hash_file(path: &Path, limit: u64) -> SeaRes<String> {
    let file = fs::File::open(path).map_err(|_| denied("error.projectReadFailed"))?;
    let mut reader = file.take(limit.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| denied("error.projectReadFailed"))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > limit {
            // Grew after it was listed; refuse rather than hash a prefix.
            return Err(invalid("error.projectFileTooLarge"));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Every file of the project folder, sorted, with sizes and hashes.
///
/// Hidden directories (`.git`, `.opennote`) and this app's staging files are
/// not project input and are left out; hidden files such as `.latexmkrc` are
/// listed so inspection can report them. A reparse point is an error.
pub(crate) fn list_project(root: &Path, folder: &str) -> SeaRes<Vec<ProjectFileEntry>> {
    let folder = project_folder(folder)?;
    if !folder.is_empty() {
        attachments::refuse_reparse_ancestors(root, &format!("{folder}/probe"))?;
    }
    let base = if folder.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&folder)
    };
    match fs::symlink_metadata(&base) {
        Ok(meta) if meta.is_dir() && !scan::is_reparse(&base) => {}
        Ok(_) => return Err(denied("error.projectReparse")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(err(ErrorCode::ObjectNotFound, "error.projectMissing", &[]))
        }
        Err(_) => return Err(denied("error.projectReadFailed")),
    }

    let mut found: Vec<(String, std::path::PathBuf, u64)> = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![(base, folder.clone())];
    while let Some((directory, prefix)) = stack.pop() {
        let entries = fs::read_dir(&directory).map_err(|_| denied("error.projectReadFailed"))?;
        for entry in entries {
            let entry = entry.map_err(|_| denied("error.projectReadFailed"))?;
            let path = entry.path();
            let meta =
                fs::symlink_metadata(&path).map_err(|_| denied("error.projectReadFailed"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if scan::is_reparse(&path) {
                return Err(denied("error.projectReparse"));
            }
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if meta.is_dir() {
                if !name.starts_with('.') {
                    stack.push((path, relative));
                }
                continue;
            }
            if !meta.is_file() || name.to_ascii_lowercase().ends_with(".opennote-partial") {
                continue;
            }
            if found.len() == MAX_PROJECT_FILES {
                return Err(invalid("error.projectTooManyFiles"));
            }
            if meta.len() > MAX_PROJECT_FILE_BYTES {
                return Err(invalid("error.projectFileTooLarge"));
            }
            total += meta.len();
            if total > MAX_PROJECT_TOTAL_BYTES {
                return Err(invalid("error.projectTooLarge"));
            }
            found.push((relative, path, meta.len()));
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
        .into_iter()
        .map(|(relative_path, path, size)| {
            Ok(ProjectFileEntry {
                relative_path,
                size,
                sha256: hash_file(&path, MAX_PROJECT_FILE_BYTES)?,
            })
        })
        .collect()
}

/// Reads one project file (any type) in full.
pub(crate) fn read_project_file(root: &Path, relative_path: &str) -> SeaRes<ProjectFileBytes> {
    let canonical = pathcheck::validate_vault_relative_path(relative_path)?;
    attachments::refuse_reparse_ancestors(root, &canonical)
        .map_err(|_| denied("error.projectReparse"))?;
    let target = root.join(&canonical);
    if scan::is_reparse(&target) {
        return Err(denied("error.projectReparse"));
    }
    let bytes = match attachments::read_regular(&target, MAX_PROJECT_FILE_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Err(err(
                ErrorCode::ObjectNotFound,
                "error.projectFileMissing",
                &[],
            ))
        }
        // Not a regular file, or larger than the limit.
        Err(_) => return Err(invalid("error.projectFileUnreadable")),
    };
    Ok(ProjectFileBytes {
        relative_path: canonical,
        sha256: hex_sha256(&bytes),
        bytes,
    })
}

/// Whether `path` names a source file a project save may write.
pub fn is_project_source(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    PROJECT_SOURCE_EXTENSIONS
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// Compare-and-swap save of one source file; see `attachments::compare_and_swap`.
pub(crate) fn save_project_source(
    root: &Path,
    relative_path: &str,
    bytes: &[u8],
    expected_sha256: Option<&str>,
    operation_id: &str,
) -> SeaRes<(String, String, AttachmentWrite)> {
    let canonical = pathcheck::validate_attachment_path(relative_path)?;
    if !is_project_source(&canonical) {
        return Err(invalid("error.projectNotSource"));
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(invalid("error.projectEncoding"));
    }
    attachments::compare_and_swap(
        root,
        canonical,
        bytes,
        expected_sha256,
        operation_id,
        MAX_PROJECT_FILE_BYTES,
    )
}

/// `.opennote/latex/<projectId>` for a project id that must be a UUID.
fn record_dir(root: &Path, project_id: &str) -> SeaRes<std::path::PathBuf> {
    let parsed =
        uuid::Uuid::parse_str(project_id).map_err(|_| invalid("error.projectIdInvalid"))?;
    // The hyphenated lowercase form only, so one project has one directory.
    if parsed.hyphenated().to_string() != project_id {
        return Err(invalid("error.projectIdInvalid"));
    }
    let mut current = root.to_path_buf();
    for part in [".opennote", "latex", project_id] {
        current.push(part);
        if scan::is_reparse(&current) {
            return Err(denied("error.opennoteReparse"));
        }
    }
    Ok(current)
}

/// The stored record, or `None` when the project has none yet.
pub(crate) fn read_record(root: &Path, project_id: &str) -> SeaRes<Option<ProjectRecordBytes>> {
    let path = record_dir(root, project_id)?.join("project.json");
    if scan::is_reparse(&path) {
        return Err(denied("error.opennoteReparse"));
    }
    match attachments::read_regular(&path, MAX_RECORD_BYTES) {
        Ok(Some(bytes)) => Ok(Some(ProjectRecordBytes {
            sha256: hex_sha256(&bytes),
            bytes,
        })),
        Ok(None) => Ok(None),
        Err(_) => Err(denied("error.projectRecordUnreadable")),
    }
}

/// Every stored project record, by project id, sorted. A directory that is not
/// a canonical project id, or holds no record, is not a project.
pub(crate) fn list_records(root: &Path) -> SeaRes<Vec<(String, ProjectRecordBytes)>> {
    let mut directory = root.join(".opennote");
    if scan::is_reparse(&directory) {
        return Err(denied("error.opennoteReparse"));
    }
    directory.push("latex");
    if scan::is_reparse(&directory) {
        return Err(denied("error.opennoteReparse"));
    }
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(denied("error.projectRecordUnreadable")),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| denied("error.projectRecordUnreadable"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if record_dir(root, &name).is_err() {
            continue;
        }
        if let Some(record) = read_record(root, &name)? {
            records.push((name, record));
        }
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records)
}

/// Removes a project record that still has `expected_sha256`. Only the record
/// under `.opennote` is removed; the project's files are never touched. An
/// already removed record is success, so a retry is safe.
pub(crate) fn remove_record(root: &Path, project_id: &str, expected_sha256: &str) -> SeaRes<()> {
    let directory = record_dir(root, project_id)?;
    let target = directory.join("project.json");
    match attachments::read_regular(&target, MAX_RECORD_BYTES) {
        Ok(None) => return Ok(()),
        Ok(Some(bytes)) if hex_sha256(&bytes) == expected_sha256.to_ascii_lowercase() => {}
        Ok(Some(_)) => {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.projectRecordChanged",
                &[],
            ))
        }
        Err(_) => return Err(denied("error.projectRecordUnreadable")),
    }
    fs::remove_file(&target).map_err(|_| denied("error.projectRecordWriteFailed"))?;
    // Leftover staging files only; anything else keeps the directory.
    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .ends_with(".opennote-partial")
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let _ = fs::remove_dir(&directory);
    Ok(())
}

/// Atomically replaces the record when it still has `expected_sha256` (None:
/// it must not exist yet). Returns the new hash. A record changed meanwhile is
/// a VersionConflict and is left untouched.
pub(crate) fn write_record(
    root: &Path,
    project_id: &str,
    bytes: &[u8],
    expected_sha256: Option<&str>,
    operation_id: &str,
) -> SeaRes<String> {
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid("error.projectRecordTooLarge"));
    }
    let directory = record_dir(root, project_id)?;
    state::ensure_writable_dir_parents(root, &directory)?;
    let target = directory.join("project.json");
    let new_sha = hex_sha256(bytes);
    let matches_expected = |current: Option<Vec<u8>>| match (current, expected_sha256) {
        (None, None) => true,
        (Some(existing), Some(expected)) => hex_sha256(&existing) == expected.to_ascii_lowercase(),
        _ => false,
    };
    let current = attachments::read_regular(&target, MAX_RECORD_BYTES)
        .map_err(|_| denied("error.projectRecordUnreadable"))?;
    if current.as_deref().map(hex_sha256).as_deref() == Some(new_sha.as_str()) {
        return Ok(new_sha);
    }
    if !matches_expected(current) {
        return Err(err(
            ErrorCode::VersionConflict,
            "error.projectRecordChanged",
            &[],
        ));
    }

    let staging = directory.join(format!(".project.json.{operation_id}.opennote-partial"));
    let _ = fs::remove_file(&staging);
    let map_write = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::StorageFull {
            err(ErrorCode::DiskFull, "error.diskFull", &[])
        } else {
            denied("error.projectRecordWriteFailed")
        }
    };
    let outcome = (|| -> SeaRes<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(map_write)?;
        file.write_all(bytes).map_err(map_write)?;
        file.sync_all().map_err(map_write)?;
        drop(file);
        let written = attachments::read_regular(&staging, MAX_RECORD_BYTES)?.unwrap_or_default();
        if hex_sha256(&written) != new_sha {
            return Err(denied("error.projectRecordWriteFailed"));
        }
        let still = attachments::read_regular(&target, MAX_RECORD_BYTES)
            .map_err(|_| denied("error.projectRecordUnreadable"))?;
        if !matches_expected(still) {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.projectRecordChanged",
                &[],
            ));
        }
        if expected_sha256.is_none() {
            // Fails if a record appeared meanwhile, so it is never replaced.
            fs::hard_link(&staging, &target).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    err(
                        ErrorCode::VersionConflict,
                        "error.projectRecordChanged",
                        &[],
                    )
                } else {
                    map_write(error)
                }
            })
        } else {
            fs::rename(&staging, &target).map_err(map_write)
        }
    })();
    let _ = fs::remove_file(&staging);
    outcome.map(|()| new_sha)
}
