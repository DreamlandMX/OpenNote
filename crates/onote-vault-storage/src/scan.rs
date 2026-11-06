use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use onote_domain::VaultFinding;

use crate::errors::SeaRes;
use crate::identity::{parse_frontmatter, permits_identity_insertion, BlockId, Frontmatter};

/// A note discovered by scanning. Existing bytes are never written back.
#[derive(Debug, Clone)]
pub(crate) struct ScannedNote {
    pub relative_path: String,
    pub content_hash: String,
    pub read_only: bool,
    pub embedded_id: Option<String>,
}

pub(crate) struct ScanResult {
    pub notes: Vec<ScannedNote>,
    pub findings: Vec<VaultFinding>,
}

/// True if the path itself is a symlink/junction/reparse point.
/// Uses the raw FILE_ATTRIBUTE_REPARSE_POINT bit on Windows so junctions and
/// any other reparse tags are caught without relying on is_symlink semantics.
#[cfg(windows)]
pub(crate) fn is_reparse(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    fs::symlink_metadata(path).map_or(false, |m| {
        m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    })
}

#[cfg(not(windows))]
pub(crate) fn is_reparse(path: &Path) -> bool {
    fs::symlink_metadata(path).map_or(false, |m| m.file_type().is_symlink())
}

pub(crate) fn scan_vault(root: &Path) -> SeaRes<ScanResult> {
    let mut notes: Vec<ScannedNote> = Vec::new();
    let mut findings: Vec<VaultFinding> = Vec::new();
    let mut seen_ids: HashMap<String, String> = HashMap::new();
    walk(root, String::new(), 0, &mut notes, &mut findings)?;

    for note in &notes {
        if let Some(id) = &note.embedded_id {
            let first = seen_ids
                .entry(id.clone())
                .or_insert_with(|| note.relative_path.clone());
            if *first != note.relative_path {
                findings.push(VaultFinding {
                    code: "DuplicateNoteId".to_owned(),
                    target: Some(note.relative_path.clone()),
                    message_key: "finding.duplicateNoteId".to_owned(),
                });
            }
        }
    }

    Ok(ScanResult { notes, findings })
}

fn walk(
    dir: &Path,
    prefix: String,
    depth: usize,
    notes: &mut Vec<ScannedNote>,
    findings: &mut Vec<VaultFinding>,
) -> SeaRes<()> {
    if depth > 64 {
        findings.push(VaultFinding {
            code: "TreeTooDeep".to_owned(),
            target: Some(prefix.clone()),
            message_key: "finding.treeTooDeep".to_owned(),
        });
        return Ok(());
    }
    let mut entries: Vec<std::fs::DirEntry> = fs::read_dir(dir)
        .map_err(|e| crate::errors::io(&e, "error.scanDenied"))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    for entry in entries {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        if name_str.eq_ignore_ascii_case(".opennote") {
            continue;
        }
        if is_reparse(&entry.path()) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    continue;
                }
                return Err(crate::errors::io(&e, "error.scanDenied"));
            }
        };
        let relative = if prefix.is_empty() {
            name_str
        } else {
            format!("{}/{}", prefix, name_str)
        };
        if meta.is_dir() {
            walk(&entry.path(), relative, depth + 1, notes, findings)?;
        } else if meta.is_file() && is_markdown(&entry.path()) {
            scan_file(&entry.path(), &relative, notes)?;
        }
    }
    Ok(())
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn scan_file(path: &Path, relative_path: &str, notes: &mut Vec<ScannedNote>) -> SeaRes<()> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(crate::errors::io(&e, "error.scanReadDenied")),
    };
    let hash = hex_sha256(&bytes);
    // Malformed YAML (duplicates, unloaded content, non-mapping top level)
    // and an invalid reserved `opennote-id` make a note read-only; disk bytes
    // are never modified by scanning in any case.
    let (read_only, embedded_id) = match std::str::from_utf8(&bytes) {
        Ok(text) => match parse_frontmatter(text) {
            Frontmatter::NoBlock => (false, None),
            Frontmatter::WellFormed {
                id: BlockId::Valid(id),
                ..
            } => (false, Some(id)),
            // parseable but identity-less: writable only when an identity
            // could be inserted without ever producing invalid frontmatter
            Frontmatter::WellFormed {
                id: BlockId::None, ..
            } => (!permits_identity_insertion(text), None),
            Frontmatter::WellFormed {
                id: BlockId::Invalid,
                ..
            } => (true, None),
            Frontmatter::Malformed => (true, None),
        },
        Err(_) => (true, None),
    };
    notes.push(ScannedNote {
        relative_path: relative_path.to_owned(),
        content_hash: hash,
        read_only,
        embedded_id,
    });
    Ok(())
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
