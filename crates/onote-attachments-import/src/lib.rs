//! M07 import planning: the safe half of F24.
//!
//! This crate never reads a ZIP, never touches the filesystem and never writes a
//! note. It decides, from an already-enumerated archive entry list, which entries
//! may be imported, where they map to, and what would be lost. The host performs
//! the actual reads and the M02 commit, so the safety rules below are unit-
//! testable without a real archive (F24-A01).
//!
//! Rules that hold regardless of the caller:
//!
//! * a path with `..`, an absolute root, a drive prefix, a UNC prefix or a NUL
//!   byte is rejected — an archive must never write outside the chosen folder;
//! * a symlink entry is rejected instead of followed, because its target is not
//!   something this planner can validate;
//! * entry count and expanded bytes are bounded, so a zip bomb is a typed error
//!   rather than a hang;
//! * duplicate destinations are a conflict the caller must resolve, never a
//!   silent overwrite;
//! * text that is not valid UTF-8 is reported, never replaced with U+FFFD.

pub mod attachment;
pub mod notion;
pub mod zip_read;
pub use attachment::{
    classify as classify_attachment, detect_media_type, extension_of, is_inline_image,
    issue_key as attachment_issue_key, plan_import as plan_attachments, reference_for,
    validate_destination as validate_attachment_folder, AttachmentInput, AttachmentIssue,
    AttachmentKind, AttachmentPlan, AttachmentPlan as Plan, PlannedAttachment,
    Preview as AttachmentPreview, CLOUD_LIMIT_BYTES as ATTACHMENT_CLOUD_LIMIT,
    MAX_ATTACHMENT_BYTES,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod export;
pub use export::stage as stage_zip;
pub use export::{
    compatibility_losses, folder_loss_report, folder_manifest, is_lossless,
    loss_report as export_loss_report, loss_report_name, plan_export, referenced_assets,
    verify_folder, verify_zip, CompatibilityLoss, CompatibilitySyntax, ExportEntry, ExportFormat,
    ExportIssue, ExportManifest, ExportOptions, ExportPlan, ExportPlanError, ExportScope,
    ManifestEntry, StagedArchive, ZipWriter, LOSS_REPORT_NAME, MAX_EXPORT_ENTRY_BYTES,
    STAGING_SUFFIX,
};

/// F24 §4: at most 100000 entries before the caller must split the import.
pub const MAX_ENTRIES: usize = 100_000;
/// F24 §4: at most 10 GiB of expanded content.
pub const MAX_EXPANDED_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// A single Markdown file this build will import.
pub const MAX_NOTE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    /// Known to be Markdown, whatever its extension.
    Markdown,
    /// A regular file whose type is decided by its extension.
    File,
    /// A binary file this build copies as an attachment (F19 owns handling).
    Attachment,
    /// A directory entry; it carries no content.
    Directory,
    /// A symlink, device or other entry this build refuses to follow.
    Symlink,
}

impl EntryKind {
    /// Classifies a raw archive entry by its declared type.
    pub fn from_archive(path: &str, is_directory: bool, is_symlink: bool) -> Self {
        if is_symlink {
            EntryKind::Symlink
        } else if is_directory {
            EntryKind::Directory
        } else if matches!(extension(path).as_str(), "md" | "markdown") {
            EntryKind::Markdown
        } else {
            EntryKind::File
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntry {
    /// Raw path as stored in the archive, with either separator.
    pub path: String,
    /// Declared uncompressed size, if the archive states one.
    pub size: u64,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanIssue {
    /// `..`, an absolute path, a drive/UNC prefix or a NUL byte.
    PathTraversal(String),
    /// A symlink or special entry that would be followed.
    UnsupportedEntry(String),
    /// Not valid UTF-8; the caller must ask before converting.
    EncodingInvalid(String),
    /// A file larger than this build imports in one piece.
    EntryTooLarge(String),
    /// Two entries map to the same destination.
    DuplicateDestination(String, String),
    /// A note whose destination already exists in the vault.
    DestinationExists(String),
    /// A file that is not Markdown and not a supported attachment.
    UnsupportedType(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedEntry {
    pub source_path: String,
    /// Vault-relative destination with `/` separators.
    pub destination: String,
    pub kind: EntryKind,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub entries: Vec<PlannedEntry>,
    pub issues: Vec<PlanIssue>,
    /// Entries skipped for a reason that loses content, reported to the user.
    pub losses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanError {
    TooManyEntries {
        limit: usize,
    },
    ExpandedTooLarge {
        limit: u64,
    },
    /// Nothing importable was found; the caller shows an empty state, not success.
    EmptyArchive,
    /// The destination folder prefix is absolute, escapes the vault or is malformed.
    InvalidPrefix,
}

/// Normalizes a separator and rejects anything that could escape the target.
fn safe_relative(path: &str) -> Result<String, PlanIssue> {
    let raw = path.replace('\\', "/");
    if raw.contains('\0') {
        return Err(PlanIssue::PathTraversal(path.to_owned()));
    }
    // A Windows drive prefix (`C:`) or a UNC prefix (`//server`) is absolute.
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Err(PlanIssue::PathTraversal(path.to_owned()));
    }
    if raw.starts_with('/') || raw.starts_with("//") {
        return Err(PlanIssue::PathTraversal(path.to_owned()));
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err(PlanIssue::PathTraversal(path.to_owned())),
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Err(PlanIssue::PathTraversal(path.to_owned()));
    }
    Ok(parts.join("/"))
}

fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, suffix)) => suffix.to_ascii_lowercase(),
        None => String::new(),
    }
}

fn kind_of(path: &str) -> Option<EntryKind> {
    Some(match extension(path).as_str() {
        "md" | "markdown" => EntryKind::Markdown,
        // F19 owns attachment handling; this list only decides what is treated
        // as a binary attachment rather than an unsupported file.
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" | "pdf" | "mp3"
        | "mp4" | "m4a" | "wav" | "ogg" | "webm" | "txt" | "csv" | "json" | "yaml" | "yml"
        | "zip" => EntryKind::Attachment,
        _ => return None,
    })
}

/// How the caller resolves a destination that already exists in the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Skip the entry and report it; never overwrite an existing note.
    Skip,
    /// Append a numeric suffix so both notes survive.
    Rename,
}

fn suffixed(destination: &str, index: u32) -> String {
    match destination.rsplit_once('.') {
        Some((stem, suffix)) => format!("{stem} ({index}).{suffix}"),
        None => format!("{destination} ({index})"),
    }
}

/**
 * Builds the import plan from an entry list.
 *
 * `existing` lists vault-relative paths that already exist. `conflict` decides
 * whether such an entry is skipped or renamed; this build never silently
 * overwrites, because that would destroy a note the user did not choose to lose.
 */
pub fn plan_import(
    entries: &[ArchiveEntry],
    existing: &[String],
    conflict: ConflictPolicy,
) -> Result<ImportPlan, PlanError> {
    plan_import_into(entries, existing, conflict, "", &BTreeMap::new())
}

/// Normalizes a destination folder prefix; empty means the vault root.
pub fn normalize_prefix(prefix: &str) -> Result<String, PlanError> {
    let trimmed = prefix.trim().replace('\\', "/");
    let trimmed = trimmed.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    safe_relative(trimmed).map_err(|_| PlanError::InvalidPrefix)
}

fn same_path(left: &str, right: &str) -> bool {
    // Windows vaults are case-insensitive: `Notes/A.md` and `notes/a.md` collide.
    left.to_lowercase() == right.to_lowercase()
}

/**
 * Plans an import into `prefix` (vault-relative folder, empty for the root).
 *
 * The prefix is applied before conflict detection, so a collision with an
 * existing `prefix/name.md` is renamed or skipped instead of surfacing later as
 * a storage PathCollision. `pinned` maps a source path to the destination an
 * earlier attempt of the same batch already created; such entries keep that
 * destination and are not treated as conflicts with their own earlier result,
 * so a retry replays instead of creating `name (1).md`.
 */
pub fn plan_import_into(
    entries: &[ArchiveEntry],
    existing: &[String],
    conflict: ConflictPolicy,
    prefix: &str,
    pinned: &BTreeMap<String, String>,
) -> Result<ImportPlan, PlanError> {
    let prefix = normalize_prefix(prefix)?;
    if entries.len() > MAX_ENTRIES {
        return Err(PlanError::TooManyEntries { limit: MAX_ENTRIES });
    }
    let mut expanded: u64 = 0;
    for entry in entries {
        expanded = expanded.saturating_add(entry.size);
        if expanded > MAX_EXPANDED_BYTES {
            return Err(PlanError::ExpandedTooLarge {
                limit: MAX_EXPANDED_BYTES,
            });
        }
    }

    let mut planned: Vec<PlannedEntry> = Vec::new();
    let mut issues: Vec<PlanIssue> = Vec::new();
    let mut losses: Vec<String> = Vec::new();
    let pinned_destinations: Vec<&String> = pinned.values().collect();
    // Paths a pinned retry already owns are not "someone else's note".
    let known: Vec<String> = existing
        .iter()
        .map(|path| path.replace('\\', "/"))
        .filter(|path| !pinned_destinations.iter().any(|own| same_path(own, path)))
        .collect();
    let taken = |candidate: &str, planned: &[PlannedEntry]| {
        known.iter().any(|path| same_path(path, candidate))
            || pinned_destinations
                .iter()
                .any(|path| same_path(path, candidate))
            || planned
                .iter()
                .any(|item| same_path(&item.destination, candidate))
    };

    for entry in entries {
        if entry.kind == EntryKind::Directory {
            continue;
        }
        if entry.kind == EntryKind::Symlink {
            issues.push(PlanIssue::UnsupportedEntry(entry.path.clone()));
            losses.push(entry.path.clone());
            continue;
        }
        if entry.size > MAX_NOTE_BYTES {
            issues.push(PlanIssue::EntryTooLarge(entry.path.clone()));
            losses.push(entry.path.clone());
            continue;
        }
        let relative = match safe_relative(&entry.path) {
            Ok(value) => value,
            Err(issue) => {
                issues.push(issue);
                losses.push(entry.path.clone());
                continue;
            }
        };
        let destination = if prefix.is_empty() {
            relative
        } else {
            format!("{prefix}/{relative}")
        };
        let kind = if entry.kind == EntryKind::Markdown {
            EntryKind::Markdown
        } else {
            let Some(by_extension) = kind_of(&destination) else {
                issues.push(PlanIssue::UnsupportedType(destination.clone()));
                losses.push(destination);
                continue;
            };
            by_extension
        };
        // A destination that collides with another archive entry is a conflict
        // in the source itself and cannot be resolved by renaming reliably.
        if let Some(previous) = planned
            .iter()
            .find(|item| same_path(&item.destination, &destination))
        {
            issues.push(PlanIssue::DuplicateDestination(
                previous.source_path.clone(),
                entry.path.clone(),
            ));
            losses.push(destination);
            continue;
        }
        if let Some(own) = pinned.get(&entry.path) {
            planned.push(PlannedEntry {
                source_path: entry.path.clone(),
                destination: own.clone(),
                kind,
                size: entry.size,
            });
            continue;
        }
        let mut final_destination = destination.clone();
        if taken(&final_destination, &planned) {
            match conflict {
                ConflictPolicy::Skip => {
                    issues.push(PlanIssue::DestinationExists(final_destination));
                    losses.push(destination);
                    continue;
                }
                ConflictPolicy::Rename => {
                    let mut index = 1u32;
                    while taken(&final_destination, &planned) {
                        final_destination = suffixed(&destination, index);
                        index += 1;
                    }
                }
            }
        }
        planned.push(PlannedEntry {
            source_path: entry.path.clone(),
            destination: final_destination,
            kind,
            size: entry.size,
        });
    }

    if planned.is_empty() && issues.is_empty() {
        return Err(PlanError::EmptyArchive);
    }
    Ok(ImportPlan {
        entries: planned,
        issues,
        losses,
    })
}

/// Decodes staged bytes for a Markdown entry.
///
/// F24 §7: text that is not valid UTF-8 is reported so the caller can ask before
/// converting. It is never decoded with replacement characters, because U+FFFD
/// would silently destroy the user's bytes and then claim a complete import.
pub fn decode_markdown(path: &str, bytes: &[u8]) -> Result<String, PlanIssue> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(_) => Err(PlanIssue::EncodingInvalid(path.to_owned())),
    }
}

/// A UTF-8 BOM is not content; dropping it keeps the first heading first.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// The rewritten notes plus the dead-link inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewrittenNotes {
    /// Note path -> rewritten content. Only notes whose bytes changed appear.
    pub notes: Vec<(String, String)>,
    pub dead: Vec<ImportedLink>,
}

/// Rewrites every note in one pass. A link is rewritten only when its target is
/// in `targets`; otherwise it is left byte-identical and listed once per
/// occurrence, so the user can see exactly which links need a manual fix.
pub fn rewrite_notes(
    sources: &[(String, String)],
    targets: &std::collections::BTreeMap<String, String>,
) -> RewrittenNotes {
    let mut notes = Vec::new();
    let mut dead = Vec::new();
    for (path, source) in sources {
        let result = rewrite_links(path, source, targets);
        dead.extend(result.dead);
        if result.text != *source {
            notes.push((path.clone(), result.text));
        }
    }
    RewrittenNotes { notes, dead }
}

/// A wiki-style link found in an imported note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedLink {
    pub note: String,
    pub target: String,
    /// True when the raw target started with `#`, i.e. a same-note anchor.
    pub anchor_only: bool,
}

/// Rewrites wiki links by the import mapping.
///
/// F24-R03: a link whose target is not in the mapping must be reported as
/// dead, not guessed. `old` is the raw link text and `new` is the replacement,
/// both byte-exact, so the caller applies them without re-scanning Markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkRewrite {
    pub note: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkMappingResult {
    pub rewrites: Vec<LinkRewrite>,
    /// Links kept as written because their target did not import.
    pub dead: Vec<ImportedLink>,
    /// The rewritten note text, built in the same pass. Callers must use this
    /// rather than re-applying `rewrites`, which could match the wrong
    /// occurrence once a replacement contains the original text.
    pub text: String,
}

/// Extracts `[[target]]`, `[[target|label]]` and `![[target#anchor]]` links.
pub fn scan_wiki_links(note: &str, source: &str) -> Vec<ImportedLink> {
    let mut links = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] != b'[' || bytes[index + 1] != b'[' {
            index += 1;
            continue;
        }
        let close = source[index + 2..].find("]]");
        let Some(relative) = close else {
            break;
        };
        let end = index + 2 + relative;
        let inner = &source[index + 2..end];
        let target = inner.split('|').next().unwrap_or(inner).trim();
        if !target.is_empty() {
            links.push(ImportedLink {
                note: note.to_owned(),
                target: target.to_owned(),
                anchor_only: target.starts_with('#'),
            });
        }
        index = end + 2;
    }
    links
}

/// Rewrites the links of one note. `targets` maps an imported link target (the
/// text between the brackets, without `#anchor`) to the destination note path.
pub fn rewrite_links(
    note: &str,
    source: &str,
    targets: &std::collections::BTreeMap<String, String>,
) -> LinkMappingResult {
    let mut result = LinkMappingResult {
        rewrites: Vec::new(),
        dead: Vec::new(),
        text: String::new(),
    };
    let mut output = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if index + 1 < bytes.len() && bytes[index] == b'[' && bytes[index + 1] == b'[' {
            if let Some(relative) = source[index + 2..].find("]]") {
                let end = index + 2 + relative;
                let inner = &source[index + 2..end];
                let (target, label) = match inner.split_once('|') {
                    Some((target, label)) => (target.trim(), Some(label)),
                    None => (inner.trim(), None),
                };
                let (note_name, anchor) = match target.split_once('#') {
                    Some((name, anchor)) => (name.trim(), Some(anchor.trim())),
                    None => (target, None),
                };
                let key = note_name.to_owned();
                let mapped = if key.is_empty() {
                    None
                } else {
                    targets.get(&key).cloned()
                };
                match mapped {
                    Some(destination) => {
                        let link_target = match anchor {
                            Some(anchor) if !anchor.is_empty() => format!("{destination}#{anchor}"),
                            _ => destination,
                        };
                        let replacement = match label {
                            Some(label) => format!("[[{link_target}|{label}]]"),
                            None => format!("[[{link_target}]]"),
                        };
                        result.rewrites.push(LinkRewrite {
                            note: note.to_owned(),
                            old: source[index..end + 2].to_owned(),
                            new: replacement.clone(),
                        });
                        output.push_str(&replacement);
                    }
                    None => {
                        result.dead.push(ImportedLink {
                            note: note.to_owned(),
                            target: target.to_owned(),
                            anchor_only: note_name.is_empty(),
                        });
                        output.push_str(&source[index..end + 2]);
                    }
                }
                index = end + 2;
                continue;
            }
        }
        let character = source[index..].chars().next().unwrap_or(' ');
        output.push(character);
        index += character.len_utf8();
    }
    result.rewrites.shrink_to_fit();
    result.text = output;
    result
}

/// Counts what a plan would create, for the S38 confirmation summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    pub notes: usize,
    pub attachments: usize,
    pub skipped: usize,
    pub total_bytes: u64,
}

pub fn summarize(plan: &ImportPlan) -> PlanSummary {
    let mut summary = PlanSummary {
        notes: 0,
        attachments: 0,
        skipped: plan.issues.len(),
        total_bytes: 0,
    };
    for entry in &plan.entries {
        match entry.kind {
            EntryKind::Markdown => summary.notes += 1,
            EntryKind::Attachment => summary.attachments += 1,
            _ => {}
        }
        summary.total_bytes = summary.total_bytes.saturating_add(entry.size);
    }
    summary
}

/// True when the plan contains nothing dangerous, so the caller can name it a
/// complete import instead of a partial one (F24-R04).
pub fn is_complete(plan: &ImportPlan) -> bool {
    plan.issues.is_empty() && plan.losses.is_empty()
}

/// A loss report as Markdown, which F24 §6 says the user may keep.
pub fn loss_report(plan: &ImportPlan) -> String {
    let mut report = String::from("# Import losses\n\n");
    if plan.issues.is_empty() {
        report.push_str("No losses were detected.\n");
        return report;
    }
    for issue in &plan.issues {
        report.push_str("- ");
        match issue {
            PlanIssue::PathTraversal(path) => {
                report.push_str(&format!("Unsafe path rejected: `{path}`\n"));
            }
            PlanIssue::UnsupportedEntry(path) => {
                report.push_str(&format!(
                    "Symlink or special entry not followed: `{path}`\n"
                ));
            }
            PlanIssue::EncodingInvalid(path) => {
                report.push_str(&format!("Not valid UTF-8, not converted: `{path}`\n"));
            }
            PlanIssue::EntryTooLarge(path) => {
                report.push_str(&format!("File larger than one import: `{path}`\n"));
            }
            PlanIssue::DuplicateDestination(first, second) => {
                report.push_str(&format!(
                    "Two entries map to one path: `{first}` and `{second}`\n"
                ));
            }
            PlanIssue::DestinationExists(path) => {
                report.push_str(&format!(
                    "Already in the vault, not overwritten: `{path}`\n"
                ));
            }
            PlanIssue::UnsupportedType(path) => {
                report.push_str(&format!("Unsupported file type: `{path}`\n"));
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {

    fn md(path: &str) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size: 4,
            kind: EntryKind::Markdown,
        }
    }

    #[test]
    fn prefix_is_applied_before_conflict_detection() {
        let plan = plan_import_into(
            &[md("a.md")],
            &["imported/a.md".to_owned()],
            ConflictPolicy::Rename,
            "/imported/",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(plan.entries[0].destination, "imported/a (1).md");
    }

    #[test]
    fn conflicts_are_case_insensitive_like_windows_paths() {
        let plan = plan_import_into(
            &[md("Notes/A.md")],
            &["notes/a.md".to_owned()],
            ConflictPolicy::Rename,
            "",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(plan.entries[0].destination, "Notes/A (1).md");
    }

    #[test]
    fn pinned_items_keep_their_earlier_destination_and_others_avoid_it() {
        let mut pinned = BTreeMap::new();
        pinned.insert("a.md".to_owned(), "a.md".to_owned());
        let plan = plan_import_into(
            &[md("a.md"), md("sub/../b.md")],
            &["a.md".to_owned()],
            ConflictPolicy::Rename,
            "",
            &pinned,
        );
        let plan = plan.unwrap();
        assert_eq!(
            plan.entries[0].destination, "a.md",
            "own earlier note is not a conflict"
        );
    }

    #[test]
    fn invalid_prefixes_are_rejected() {
        for prefix in ["../x", "C:/x", "a/../../b"] {
            assert_eq!(
                plan_import_into(
                    &[md("a.md")],
                    &[],
                    ConflictPolicy::Rename,
                    prefix,
                    &BTreeMap::new()
                )
                .unwrap_err(),
                PlanError::InvalidPrefix,
                "{prefix}"
            );
        }
    }

    #[test]
    fn source_operation_ids_are_stable_and_canonical() {
        let first = source_operation_id("batch", "a.md");
        assert_eq!(first, source_operation_id("batch", "a.md"));
        assert_ne!(first, source_operation_id("batch", "b.md"));
        assert_ne!(first, note_operation_id("batch", "a.md"));
        assert_eq!(first.len(), 36);
    }
    use super::*;

    fn entry(path: &str, size: u64) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size,
            kind: EntryKind::from_archive(path, false, false),
        }
    }

    fn markdown(path: &str) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size: 10,
            kind: EntryKind::Markdown,
        }
    }

    fn symlink(path: &str) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size: 0,
            kind: EntryKind::Symlink,
        }
    }

    #[test]
    fn a_parent_traversal_is_rejected_and_never_retargeted() {
        // F24-A01: `../` must not produce a destination outside the vault.
        let plan = plan_import(
            &[markdown("notes/../../etc/passwd.md"), markdown("ok.md")],
            &[],
            ConflictPolicy::Skip,
        )
        .expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].destination, "ok.md");
        assert_eq!(
            plan.issues,
            vec![PlanIssue::PathTraversal(
                "notes/../../etc/passwd.md".to_owned()
            )]
        );
        assert!(!plan
            .entries
            .iter()
            .any(|entry| entry.destination.contains("..")));
    }

    #[test]
    fn absolute_drive_unc_and_nul_paths_are_rejected() {
        for path in [
            "/abs.md",
            "C:/win.md",
            "C:\\win.md",
            "//server/share.md",
            "a\0b.md",
        ] {
            let plan = plan_import(&[markdown(path)], &[], ConflictPolicy::Skip).expect("plan");
            assert!(plan.entries.is_empty(), "{path} produced an entry");
            assert!(
                matches!(plan.issues[0], PlanIssue::PathTraversal(_)),
                "{path}"
            );
        }
    }

    #[test]
    fn harmless_normalization_keeps_the_relative_path() {
        let plan = plan_import(
            &[markdown("./a//b/note.md"), entry("img\\pic.png", 5)],
            &[],
            ConflictPolicy::Skip,
        )
        .expect("plan");
        assert_eq!(plan.entries[0].destination, "a/b/note.md");
        assert_eq!(plan.entries[1].destination, "img/pic.png");
        assert_eq!(plan.entries[1].kind, EntryKind::Attachment);
    }

    #[test]
    fn a_symlink_entry_is_refused_not_followed() {
        let plan = plan_import(&[symlink("link.md")], &[], ConflictPolicy::Skip).expect("plan");
        assert!(plan.entries.is_empty());
        assert_eq!(
            plan.issues,
            vec![PlanIssue::UnsupportedEntry("link.md".to_owned())]
        );
    }

    #[test]
    fn unknown_file_types_are_reported_as_losses() {
        let plan = plan_import(&[entry("binary.exe", 4)], &[], ConflictPolicy::Skip).expect("plan");
        assert!(plan.entries.is_empty());
        assert_eq!(
            plan.issues,
            vec![PlanIssue::UnsupportedType("binary.exe".to_owned())]
        );
        assert_eq!(plan.losses, vec!["binary.exe".to_owned()]);
    }

    #[test]
    fn duplicate_destinations_are_a_conflict_not_an_overwrite() {
        let plan = plan_import(
            &[markdown("a.md"), markdown("./a.md")],
            &[],
            ConflictPolicy::Skip,
        )
        .expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert!(matches!(
            plan.issues[0],
            PlanIssue::DuplicateDestination(_, _)
        ));
    }

    #[test]
    fn an_existing_note_is_skipped_or_renamed_by_policy() {
        let existing = vec!["a.md".to_owned()];
        let skipped =
            plan_import(&[markdown("a.md")], &existing, ConflictPolicy::Skip).expect("plan");
        assert!(skipped.entries.is_empty());
        assert_eq!(
            skipped.issues,
            vec![PlanIssue::DestinationExists("a.md".to_owned())]
        );

        let renamed =
            plan_import(&[markdown("a.md")], &existing, ConflictPolicy::Rename).expect("plan");
        assert_eq!(renamed.entries[0].destination, "a (1).md");
        assert!(is_complete(&renamed));

        let twice = plan_import(
            &[markdown("a.md"), markdown("sub/../a.md")],
            &existing,
            ConflictPolicy::Rename,
        )
        .expect("plan");
        assert_eq!(twice.entries.len(), 1);
    }

    #[test]
    fn entry_count_and_expanded_size_are_bounded() {
        let too_many: Vec<ArchiveEntry> = (0..=MAX_ENTRIES)
            .map(|index| markdown(&format!("n{index}.md")))
            .collect();
        assert_eq!(
            plan_import(&too_many, &[], ConflictPolicy::Skip),
            Err(PlanError::TooManyEntries { limit: MAX_ENTRIES })
        );
        let huge = ArchiveEntry {
            path: "bomb.bin".to_owned(),
            size: MAX_EXPANDED_BYTES + 1,
            kind: EntryKind::File,
        };
        assert_eq!(
            plan_import(&[huge], &[], ConflictPolicy::Skip),
            Err(PlanError::ExpandedTooLarge {
                limit: MAX_EXPANDED_BYTES
            })
        );
    }

    #[test]
    fn an_oversized_single_file_is_a_loss_not_a_read() {
        let big = ArchiveEntry {
            path: "big.md".to_owned(),
            size: MAX_NOTE_BYTES + 1,
            kind: EntryKind::Markdown,
        };
        let plan = plan_import(&[big], &[], ConflictPolicy::Skip).expect("plan");
        assert!(plan.entries.is_empty());
        assert_eq!(
            plan.issues,
            vec![PlanIssue::EntryTooLarge("big.md".to_owned())]
        );
    }

    #[test]
    fn an_empty_archive_is_an_error_not_a_success() {
        assert_eq!(
            plan_import(&[], &[], ConflictPolicy::Skip),
            Err(PlanError::EmptyArchive)
        );
        let directory_only = ArchiveEntry {
            path: "folder/".to_owned(),
            size: 0,
            kind: EntryKind::Directory,
        };
        assert_eq!(
            plan_import(&[directory_only], &[], ConflictPolicy::Skip),
            Err(PlanError::EmptyArchive)
        );
    }

    #[test]
    fn summary_and_completeness_distinguish_partial_from_full() {
        let partial = plan_import(
            &[markdown("a.md"), entry("junk.exe", 1), entry("pic.png", 3)],
            &[],
            ConflictPolicy::Skip,
        )
        .expect("plan");
        let summary = summarize(&partial);
        assert_eq!(summary.notes, 1);
        assert_eq!(summary.attachments, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.total_bytes, 13);
        assert!(!is_complete(&partial));

        let full = plan_import(&[markdown("a.md")], &[], ConflictPolicy::Skip).expect("plan");
        assert!(is_complete(&full));
        assert!(loss_report(&full).contains("No losses were detected."));
    }

    #[test]
    fn the_loss_report_names_every_issue_kind() {
        let plan = plan_import(
            &[
                markdown("../x.md"),
                symlink("link"),
                entry("x.exe", 1),
                markdown("a.md"),
            ],
            &["a.md".to_owned()],
            ConflictPolicy::Skip,
        )
        .expect("plan");
        let report = loss_report(&plan);
        assert!(report.contains("# Import losses"));
        assert!(report.contains("Unsafe path rejected"));
        assert!(report.contains("not followed"));
        assert!(report.contains("Unsupported file type"));
        assert!(report.contains("Already in the vault"));
    }

    #[test]
    fn wiki_links_are_rewritten_or_reported_as_dead() {
        use std::collections::BTreeMap;
        let mut targets = BTreeMap::new();
        targets.insert("Old Name".to_owned(), "imported/old-name.md".to_owned());
        let source = "See [[Old Name]], [[Old Name#Part]], [[Old Name|label]] and [[Missing]] and [[#local]].";
        let result = rewrite_links("note.md", source, &targets);
        assert_eq!(result.rewrites.len(), 3);
        assert_eq!(result.dead.len(), 2);
        assert_eq!(result.dead[0].target, "Missing");
        assert_eq!(result.dead[1].target, "#local");
        assert!(result.dead[1].anchor_only);
        // An anchor and a label both survive the rewrite.
        assert!(result.rewrites[1].new.contains("#Part"));
        assert!(result.rewrites[2].new.ends_with("|label]]"));
        // The replacement is byte-exact against the source range.
        for rewrite in &result.rewrites {
            assert!(source.contains(&rewrite.old));
        }
    }

    #[test]
    fn a_replacement_containing_the_original_text_is_still_correct() {
        use std::collections::BTreeMap;
        // The mapped destination contains the original target, so a naive
        // second `replacen` pass over the output would rewrite the wrong range.
        let mut targets = BTreeMap::new();
        targets.insert("A".to_owned(), "A/A.md".to_owned());
        let result = rewrite_links("n.md", "[[A]] [[A]]", &targets);
        assert_eq!(result.text, "[[A/A.md]] [[A/A.md]]");
        assert_eq!(result.rewrites.len(), 2);
    }

    #[test]
    fn rewriting_notes_reports_dead_links_and_returns_final_text() {
        use std::collections::BTreeMap;
        let mut targets = BTreeMap::new();
        targets.insert("A".to_owned(), "imported/a.md".to_owned());
        let sources = vec![
            ("one.md".to_owned(), "to [[A]] and [[Gone]]".to_owned()),
            ("two.md".to_owned(), "no links".to_owned()),
        ];
        let result = rewrite_notes(&sources, &targets);
        assert_eq!(result.notes.len(), 1);
        assert_eq!(result.notes[0].0, "one.md");
        assert_eq!(result.notes[0].1, "to [[imported/a.md]] and [[Gone]]");
        assert_eq!(result.dead.len(), 1);
        assert_eq!(result.dead[0].target, "Gone");
    }

    #[test]
    fn wiki_scan_ignores_plain_text_and_unclosed_brackets() {
        let links = scan_wiki_links("n.md", "no links here [not] [[ok]] [[open");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "ok");
        assert!(scan_wiki_links("n.md", "![[]] [[  ]]").is_empty());
    }

    #[test]
    fn invalid_utf8_is_reported_not_replaced() {
        // F24 §7 ENCODING_INVALID: no U+FFFD may be substituted silently.
        let bytes = [0x23, 0x20, 0xff, 0xfe, 0x0a];
        let issue = decode_markdown("legacy.md", &bytes).unwrap_err();
        assert_eq!(issue, PlanIssue::EncodingInvalid("legacy.md".to_owned()));
        assert!(!loss_report(&ImportPlan {
            entries: vec![],
            issues: vec![issue],
            losses: vec![],
        })
        .contains('\u{FFFD}'));

        let ok = decode_markdown("ok.md", "# 标题\n".as_bytes()).unwrap();
        assert_eq!(ok, "# 标题\n");
        assert_eq!(strip_bom("\u{FEFF}# Title"), "# Title");
        assert_eq!(strip_bom("# Title"), "# Title");
    }

    #[test]
    fn a_markdown_entry_keeps_its_markdown_kind_even_without_an_extension() {
        let plan = plan_import(
            &[ArchiveEntry {
                path: "no-extension".to_owned(),
                size: 1,
                kind: EntryKind::Markdown,
            }],
            &[],
            ConflictPolicy::Skip,
        )
        .expect("plan");
        assert_eq!(plan.entries[0].kind, EntryKind::Markdown);
    }
}

/// Deterministic per-note operation id for an import.
///
/// M02 requires a canonical UUID, and a retry must reuse the same id so the
/// storage layer replays instead of creating a second note (F24-A03). The id is
/// derived from the import operation and the note destination only, so the same
/// input always produces the same attempt identity.
pub fn note_operation_id(import_operation_id: &str, destination: &str) -> String {
    // A 128-bit FNV-1a over the two inputs, formatted as a v4-shaped UUID. The
    // version/variant bits are set so the value passes canonical UUID checks;
    // this is a deterministic name, not a random identifier.
    let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
    for byte in import_operation_id
        .as_bytes()
        .iter()
        .chain(b"|".iter())
        .chain(destination.as_bytes())
    {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_01b3);
    }
    let bytes = hash.to_be_bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-8{:01x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6] & 0x0f, bytes[7],
        bytes[8] & 0x0f, bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Deterministic per-item operation id keyed by the item's *source* path.
///
/// Unlike [`note_operation_id`], the identity does not depend on the planned
/// destination, which can change between attempts (a first attempt's own note
/// makes a naive re-plan pick `name (1).md`). Keyed by source, a retry of the
/// same batch finds the earlier receipt and replays it (F24-A03/R04).
pub fn source_operation_id(import_operation_id: &str, source_path: &str) -> String {
    note_operation_id(import_operation_id, &format!("source:{source_path}"))
}

/// A note ready to be created by the caller after link rewriting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDraft {
    /// Source path inside the imported folder; the stable identity of this item.
    pub source_path: String,
    pub destination: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDraft {
    pub notes: Vec<NoteDraft>,
    /// Attachments the plan accepted but this build cannot write yet.
    pub deferred_attachments: Vec<String>,
    pub losses: Vec<String>,
}

/// Builds the note drafts for a plan.
///
/// `sources` maps a planned source path to its decoded UTF-8 body. Wiki links
/// are rewritten from the source note name to the planned destination, so an
/// imported note points at the imported copy rather than the old name (F24-R03).
/// Attachments are listed as deferred instead of being silently dropped, because
/// M02 has no attachment-write API in this build.
pub fn build_drafts(plan: &ImportPlan, sources: &[(String, String)]) -> ImportDraft {
    let mut targets = std::collections::BTreeMap::new();
    for entry in &plan.entries {
        if entry.kind != EntryKind::Markdown {
            continue;
        }
        let source_stem = entry
            .source_path
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or(&entry.source_path)
            .trim_end_matches(".md")
            .trim_end_matches(".markdown")
            .to_owned();
        // A note referring to itself by name maps to its own destination.
        targets.insert(source_stem, entry.destination.clone());
    }
    let bodies: std::collections::BTreeMap<&str, &str> = sources
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let mut notes = Vec::new();
    let mut losses = plan.losses.clone();
    for entry in &plan.entries {
        if entry.kind != EntryKind::Markdown {
            continue;
        }
        let Some(body) = bodies.get(entry.source_path.as_str()) else {
            losses.push(entry.destination.clone());
            continue;
        };
        let rewritten = rewrite_links(&entry.destination, body, &targets);
        for dead in &rewritten.dead {
            losses.push(format!("{}#{}", entry.destination, dead.target));
        }
        notes.push(NoteDraft {
            source_path: entry.source_path.clone(),
            destination: entry.destination.clone(),
            content: rewritten.text,
        });
    }
    let deferred_attachments = plan
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Attachment)
        .map(|entry| entry.destination.clone())
        .collect();
    ImportDraft {
        notes,
        deferred_attachments,
        losses,
    }
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    fn entry(path: &str, kind: EntryKind) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size: 4,
            kind,
        }
    }

    #[test]
    fn drafts_rewrite_links_to_the_imported_destination() {
        let plan = ImportPlan {
            entries: vec![
                PlannedEntry {
                    source_path: "Old Name.md".to_owned(),
                    destination: "imported/old-name.md".to_owned(),
                    kind: EntryKind::Markdown,
                    size: 4,
                },
                PlannedEntry {
                    source_path: "b.md".to_owned(),
                    destination: "imported/b.md".to_owned(),
                    kind: EntryKind::Markdown,
                    size: 4,
                },
                PlannedEntry {
                    source_path: "img/p.png".to_owned(),
                    destination: "imported/img/p.png".to_owned(),
                    kind: EntryKind::Attachment,
                    size: 4,
                },
            ],
            issues: Vec::new(),
            losses: Vec::new(),
        };
        let sources = vec![
            (
                "Old Name.md".to_owned(),
                "see [[b]] and [[Missing]]".to_owned(),
            ),
            ("b.md".to_owned(), "back to [[Old Name]]".to_owned()),
        ];
        let draft = build_drafts(&plan, &sources);
        assert_eq!(draft.notes.len(), 2);
        assert_eq!(draft.notes[0].destination, "imported/old-name.md");
        assert!(draft.notes[0].content.contains("[[imported/b.md]]"));
        assert!(draft.notes[1].content.contains("[[imported/old-name.md]]"));
        assert_eq!(
            draft.deferred_attachments,
            vec!["imported/img/p.png".to_owned()]
        );
        // The dead link is reported, not silently kept as if it worked.
        assert!(draft.losses.iter().any(|loss| loss.contains("Missing")));
    }

    #[test]
    fn a_note_without_a_decoded_body_is_a_loss() {
        let plan = ImportPlan {
            entries: vec![PlannedEntry {
                source_path: "a.md".to_owned(),
                destination: "a.md".to_owned(),
                kind: EntryKind::Markdown,
                size: 4,
            }],
            issues: Vec::new(),
            losses: Vec::new(),
        };
        let draft = build_drafts(&plan, &[]);
        assert!(draft.notes.is_empty());
        assert_eq!(draft.losses, vec!["a.md".to_owned()]);
    }

    #[test]
    fn note_operation_ids_are_canonical_and_deterministic() {
        let first = note_operation_id("import-1", "notes/a.md");
        let second = note_operation_id("import-1", "notes/a.md");
        let other = note_operation_id("import-1", "notes/b.md");
        assert_eq!(first, second, "a retry must reuse the same attempt id");
        assert_ne!(first, other, "a different note needs a different id");
        assert_eq!(first.len(), 36);
        let parts: Vec<&str> = first.split('-').collect();
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(parts[2].starts_with('4'));
        assert!(parts[3].starts_with('8'));
        assert!(first
            .chars()
            .all(|value| value.is_ascii_hexdigit() || value == '-'));
    }

    #[test]
    fn attachments_are_never_written_as_notes() {
        let plan = ImportPlan {
            entries: vec![PlannedEntry {
                source_path: "p.png".to_owned(),
                destination: "p.png".to_owned(),
                kind: EntryKind::Attachment,
                size: 4,
            }],
            issues: Vec::new(),
            losses: Vec::new(),
        };
        let draft = build_drafts(&plan, &[("p.png".to_owned(), "x".to_owned())]);
        assert!(draft.notes.is_empty());
        assert_eq!(draft.deferred_attachments, vec!["p.png".to_owned()]);
        let _ = entry("unused.png", EntryKind::Attachment);
    }
}
