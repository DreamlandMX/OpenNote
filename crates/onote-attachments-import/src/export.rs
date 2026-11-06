//! F25 export planning and a ZIP writer with no external dependency.
//!
//! The export flow is: plan (pure), write to a staging path, verify the staged
//! manifest, then publish. Everything here is deterministic and testable:
//!
//! * `plan_export` decides which vault-relative paths belong in the archive and
//!   which are excluded, and reports the references a lossless export cannot
//!   satisfy (F25-R02/R03, F25 §7 RESOURCE_MISSING);
//! * `ZipWriter` emits a valid ZIP with stored (uncompressed) entries. A stored
//!   archive is larger but the writer has no compression code to get wrong and
//!   needs no crate; the reader side is the OS/other tools, not this app;
//! * `manifest` lists every entry with its size and CRC-32, so a truncated or
//!   half-written archive can be detected instead of shown as complete
//!   (F25-A02).
//!
//! No function here touches the filesystem. The host reads notes through M02,
//! writes the bytes to a staging file and only then renames it into place; this
//! module cannot publish a partial archive on its own.

use serde::{Deserialize, Serialize};

/// F25-R02: these never leave the vault.
pub const EXCLUDED_PREFIXES: &[&str] = &[".opennote/"];
/// F25-R02: caches and search history are derived or device state.
pub const EXCLUDED_NAMES: &[&str] = &[
    "index.sqlite",
    "index.sqlite-journal",
    "index.sqlite-wal",
    "index.sqlite-shm",
    "search-history.json",
    "device-prefs.json",
    ".ds_store",
    "thumbs.db",
];
/// A single exported file above this is reported instead of silently included.
pub const MAX_EXPORT_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    /// A folder of Markdown files plus attachments, no container.
    Markdown,
    /// A ZIP with stored entries.
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportScope {
    /// One note and the attachments it references.
    Note,
    /// A folder and everything under it.
    Folder,
    /// Every note in the vault.
    Vault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub include_attachments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportIssue {
    /// The file is excluded by policy (F25-R02), listed so the user knows why.
    Excluded(String),
    /// A referenced resource is missing; a complete export needs a fix or an
    /// explicit lossy choice (F25 §7 RESOURCE_MISSING).
    ResourceMissing(String),
    /// Larger than this build exports in one entry.
    EntryTooLarge(String),
    /// An attachment was skipped because `includeAttachments` was false.
    AttachmentSkipped(String),
    /// A path that would not survive a round trip (absolute, `..`).
    UnsafePath(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEntry {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPlan {
    pub format: ExportFormat,
    pub entries: Vec<ExportEntry>,
    pub issues: Vec<ExportIssue>,
    pub losses: Vec<String>,
}

/// Wiki syntax that survives the export byte-for-byte but not in other tools
/// (F11-R04: keep the original syntax, list the compatibility loss).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompatibilitySyntax {
    Embed,
    WikiLink,
    BlockReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityLoss {
    pub path: String,
    pub syntax: CompatibilitySyntax,
    /// Text between the brackets, without an alias; user content, kept verbatim.
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportPlanError {
    /// Nothing matched the scope; an empty archive is not a success.
    NothingToExport,
}

fn is_excluded(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    if EXCLUDED_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(&prefix.to_ascii_lowercase()))
    {
        return true;
    }
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    EXCLUDED_NAMES.iter().any(|excluded| name == *excluded)
}

fn is_attachment(path: &str) -> bool {
    match path.rsplit_once('.') {
        Some((_, suffix)) => matches!(
            suffix.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "svg"
                | "bmp"
                | "avif"
                | "pdf"
                | "mp3"
                | "mp4"
                | "m4a"
                | "wav"
                | "ogg"
                | "webm"
                | "zip"
        ),
        None => false,
    }
}

fn is_safe_relative(path: &str) -> bool {
    let raw = path.replace('\\', "/");
    if raw.starts_with('/') || raw.contains('\0') {
        return false;
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    !raw.split('/').any(|part| part == "..")
}

/**
 * Plans an export from the vault's note list.
 *
 * `references` are vault-relative paths the selected notes point at (images,
 * attachments). They are checked against `existing` so a missing resource is a
 * reported loss rather than a silently broken export.
 */
pub fn plan_export(
    scope: ExportScope,
    target: &str,
    notes: &[String],
    attachments: &[String],
    existing: &[String],
    options: &ExportOptions,
) -> Result<ExportPlan, ExportPlanError> {
    let prefix = target.trim_matches('/');
    let in_scope = |path: &str| -> bool {
        let normalized = path.replace('\\', "/");
        match scope {
            ExportScope::Vault => true,
            ExportScope::Note => normalized == prefix,
            ExportScope::Folder => {
                prefix.is_empty()
                    || normalized == prefix
                    || normalized.starts_with(&format!("{prefix}/"))
            }
        }
    };

    let mut entries: Vec<ExportEntry> = Vec::new();
    let mut issues: Vec<ExportIssue> = Vec::new();
    let mut losses: Vec<String> = Vec::new();
    let existing_set: Vec<String> = existing
        .iter()
        .map(|path| path.replace('\\', "/").to_ascii_lowercase())
        .collect();

    for note in notes {
        if !in_scope(note) {
            continue;
        }
        if is_excluded(note) {
            issues.push(ExportIssue::Excluded(note.clone()));
            losses.push(note.clone());
            continue;
        }
        if !is_safe_relative(note) {
            issues.push(ExportIssue::UnsafePath(note.clone()));
            losses.push(note.clone());
            continue;
        }
        entries.push(ExportEntry {
            path: note.replace('\\', "/"),
            size: 0,
        });
    }

    // The caller supplies references from selected notes; shared assets may live outside their folder.
    if options.include_attachments {
        for attachment in attachments {
            if is_excluded(attachment) {
                issues.push(ExportIssue::Excluded(attachment.clone()));
                losses.push(attachment.clone());
                continue;
            }
            if !is_safe_relative(attachment) {
                issues.push(ExportIssue::UnsafePath(attachment.clone()));
                losses.push(attachment.clone());
                continue;
            }
            let normalized = attachment.replace('\\', "/").to_ascii_lowercase();
            if !existing_set.iter().any(|path| path == &normalized) {
                issues.push(ExportIssue::ResourceMissing(attachment.clone()));
                losses.push(attachment.clone());
                continue;
            }
            entries.push(ExportEntry {
                path: attachment.replace('\\', "/"),
                size: 0,
            });
        }
    } else {
        for attachment in attachments {
            if !is_note_path(attachment) {
                issues.push(ExportIssue::AttachmentSkipped(attachment.clone()));
                losses.push(attachment.clone());
            }
        }
    }

    if entries.is_empty() {
        return Err(ExportPlanError::NothingToExport);
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries.dedup_by(|left, right| left.path == right.path);
    Ok(ExportPlan {
        format: options.format,
        entries,
        issues,
        losses,
    })
}

/// True when the plan loses nothing, so the caller can call it complete.
pub fn is_lossless(plan: &ExportPlan) -> bool {
    plan.issues.is_empty() && plan.losses.is_empty()
}

// ---------------------------------------------------------------------------
// ZIP writer (stored entries, no compression)
// ---------------------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    // Standard CRC-32 (IEEE 802.3) with a lazily built table-free loop. Exports
    // are not a hot path, and a table would only be worth it at a much larger
    // scale.
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub crc32: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportManifest {
    pub format: ExportFormat,
    pub entries: Vec<ManifestEntry>,
    /// True only when the staging file passed the manifest check.
    pub verified: bool,
}

#[derive(Debug, Default)]
pub struct ZipWriter {
    data: Vec<u8>,
    entries: Vec<ManifestEntry>,
}

fn dos_time() -> (u16, u16) {
    // A fixed 1980-01-01 00:00 timestamp keeps exports byte-identical across
    // runs, which makes a manifest comparison meaningful. The file's real time
    // is not part of a note's content and would only add noise.
    (0, 0x0021)
}

impl ZipWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one stored entry. A duplicate path is rejected by the caller's plan,
    /// so here it would only be a programming error and is ignored.
    pub fn add(&mut self, path: &str, bytes: &[u8]) {
        if self.entries.iter().any(|entry| entry.path == path) {
            return;
        }
        let crc = crc32(bytes);
        let size = bytes.len() as u32;
        let name = path.as_bytes();
        let (time, date) = dos_time();
        let mut local = Vec::with_capacity(30 + name.len() + bytes.len());
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes()); // version needed
        local.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        local.extend_from_slice(&0u16.to_le_bytes()); // stored
        local.extend_from_slice(&time.to_le_bytes());
        local.extend_from_slice(&date.to_le_bytes());
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes()); // extra length
        local.extend_from_slice(name);
        local.extend_from_slice(bytes);
        self.data.extend_from_slice(&local);
        self.entries.push(ManifestEntry {
            path: path.to_owned(),
            size: u64::from(size),
            crc32: crc,
        });
    }

    /// Writes the central directory and returns the archive.
    pub fn finish(mut self) -> Vec<u8> {
        let mut central: Vec<u8> = Vec::new();
        let mut local_offset = 0usize;
        for entry in &self.entries {
            let name = entry.path.as_bytes();
            let (time, date) = dos_time();
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            central.extend_from_slice(&0u16.to_le_bytes()); // stored
            central.extend_from_slice(&time.to_le_bytes());
            central.extend_from_slice(&date.to_le_bytes());
            central.extend_from_slice(&entry.crc32.to_le_bytes());
            central.extend_from_slice(&(entry.size as u32).to_le_bytes());
            central.extend_from_slice(&(entry.size as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(local_offset as u32).to_le_bytes());
            central.extend_from_slice(name);
            local_offset += 30 + name.len() + entry.size as usize;
        }
        let central_offset = self.data.len() as u32;
        let central_size = central.len() as u32;
        self.data.extend_from_slice(&central);
        self.data.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data
            .extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        self.data
            .extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        self.data.extend_from_slice(&central_size.to_le_bytes());
        self.data.extend_from_slice(&central_offset.to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data
    }

    pub fn manifest(&self) -> ExportManifest {
        ExportManifest {
            format: ExportFormat::Zip,
            entries: self.entries.clone(),
            verified: false,
        }
    }
}

/**
 * Verifies a written archive against its manifest.
 *
 * F25-A02: a truncated or half-written ZIP must not be reported as complete. The
 * check validates both headers, directory bounds and the CRC-32 of actual data.
 */
pub fn verify_zip(bytes: &[u8], manifest: &ExportManifest) -> Result<(), String> {
    if bytes.len() < 22 {
        return Err("archive too short".to_owned());
    }
    // shortcut: checks our stored ZIPs only; use a ZIP reader for future import verification.
    let u16_at = |header: &[u8], at| u16::from_le_bytes([header[at], header[at + 1]]);
    let u32_at = |header: &[u8], at| {
        u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    let eocd = bytes.len() - 22;
    let end = &bytes[eocd..];
    if manifest.format != ExportFormat::Zip
        || u32_at(end, 0) != 0x0605_4b50
        || u16_at(end, 4) != 0
        || u16_at(end, 6) != 0
        || u16_at(end, 20) != 0
        || usize::from(u16_at(end, 8)) != manifest.entries.len()
        || usize::from(u16_at(end, 10)) != manifest.entries.len()
    {
        return Err("invalid end of central directory".to_owned());
    }
    let central_size = u32_at(end, 12) as usize;
    let central_offset = u32_at(end, 16) as usize;
    if central_offset.checked_add(central_size) != Some(eocd) {
        return Err("central directory bounds mismatch".to_owned());
    }
    let mut cursor = central_offset;
    let mut local_cursor = 0usize;
    for entry in &manifest.entries {
        let central = bytes
            .get(cursor..eocd)
            .and_then(|tail| tail.get(..46))
            .ok_or_else(|| "truncated central directory".to_owned())?;
        let local = bytes
            .get(local_cursor..central_offset)
            .and_then(|tail| tail.get(..30))
            .ok_or_else(|| "truncated local header".to_owned())?;
        if u32_at(central, 0) != 0x0201_4b50
            || u32_at(local, 0) != 0x0403_4b50
            || local[4..28] != central[6..30]
            || u16_at(local, 6) & !0x0800 != 0
            || u16_at(local, 8) != 0
            || u16_at(local, 28) != 0
            || u16_at(central, 30) != 0
            || u16_at(central, 32) != 0
            || u16_at(central, 34) != 0
            || u32_at(central, 42) as usize != local_cursor
        {
            return Err(format!("inconsistent ZIP headers for {}", entry.path));
        }
        let size = u32_at(central, 24);
        if u32_at(central, 16) != entry.crc32
            || u64::from(size) != entry.size
            || u32_at(central, 20) != size
        {
            return Err(format!("manifest mismatch for {}", entry.path));
        }
        let name_len = usize::from(u16_at(central, 28));
        let name_end = cursor
            .checked_add(46 + name_len)
            .filter(|end| *end <= eocd)
            .ok_or_else(|| "truncated central name".to_owned())?;
        let data_start = local_cursor
            .checked_add(30 + name_len)
            .ok_or_else(|| "local name overflow".to_owned())?;
        let data_end = data_start
            .checked_add(size as usize)
            .filter(|end| *end <= central_offset)
            .ok_or_else(|| "truncated file data".to_owned())?;
        if bytes.get(cursor + 46..name_end) != Some(entry.path.as_bytes())
            || bytes.get(local_cursor + 30..data_start) != Some(entry.path.as_bytes())
            || crc32(&bytes[data_start..data_end]) != entry.crc32
        {
            return Err(format!("name or content mismatch for {}", entry.path));
        }
        cursor = name_end;
        local_cursor = data_end;
    }
    if cursor != eocd || local_cursor != central_offset {
        return Err("unaccounted ZIP data".to_owned());
    }
    Ok(())
}

/// A staged archive that passed verification and may be published.
///
/// [`stage`] verifies bytes in memory; the host must write and read back the
/// staging file before publishing it (F25-A02, F25-R04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedArchive {
    pub bytes: Vec<u8>,
    pub manifest: ExportManifest,
}

/// Named staging suffix, so an interrupted export is recognisable on disk.
pub const STAGING_SUFFIX: &str = ".opennote-partial";

pub fn stage(writer: ZipWriter) -> Result<StagedArchive, String> {
    let mut manifest = writer.manifest();
    let bytes = writer.finish();
    verify_zip(&bytes, &manifest)?;
    manifest.verified = true;
    Ok(StagedArchive { bytes, manifest })
}

/// `report.pdf#page=3` is the file `report.pdf`, opened at a page (F20).
fn strip_pdf_page(target: &str) -> &str {
    match target.rsplit_once("#page=") {
        Some((file, page))
            if file.to_ascii_lowercase().ends_with(".pdf")
                && !page.is_empty()
                && page.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            file
        }
        _ => target,
    }
}

/// Resolves an attachment target written in a note to a vault-relative path.
///
/// `note_path` is the note's own vault-relative path; a relative target is
/// resolved against its directory. A target that escapes the vault keeps its
/// `..` segments so the planner reports `UnsafePath` instead of silently
/// dropping it, and a remote/data target returns `None`.
fn resolve_reference(note_path: &str, target: &str) -> Option<String> {
    let trimmed = target.trim().trim_matches('<').trim_matches('>').trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("mailto:")
        || trimmed.starts_with("//")
    {
        return None;
    }
    let normalized_target = strip_pdf_page(trimmed).replace('\\', "/");
    if normalized_target.starts_with('/')
        || normalized_target
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':')
    {
        // Absolute paths are not vault-relative attachments; the planner would
        // reject them, and there is nothing to copy from the vault.
        return Some(normalized_target);
    }
    // Absolute targets are returned unchanged so the planner can refuse them;
    // joining them onto the note directory would invent a different path.
    if normalized_target.starts_with('/') || normalized_target.contains(':') {
        return Some(normalized_target);
    }
    let note_dir = match note_path.rfind('/') {
        Some(index) => &note_path[..index],
        None => "",
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in note_dir.split('/').chain(normalized_target.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                // Pop a real directory; if there is none left, keep the `..` so
                // the planner can report the escape rather than hide it.
                if parts.is_empty() || parts.last() == Some(&"..") {
                    parts.push("..");
                } else {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn is_note_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/**
 * Attachment paths a note references.
 *
 * Recognizes `[file](path)`, `![alt](path)`, angle destinations, titles and
 * `![[file.png]]`. Code fences and inline code are skipped, so an example in a
 * fenced block is never treated as a real reference. A wiki embed of another
 * note is a note link, not an attachment, and is ignored here.
 */
pub fn referenced_assets(note_path: &str, source: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for visible in visible_lines(source) {
        if let Some(reference) = scan_line(&visible, note_path) {
            found.extend(reference);
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Lines with fenced blocks removed and inline-code spans stripped, so a
/// bracket inside backticks is never treated as a link or embed.
fn visible_lines(source: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut fence: Option<&str> = None;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }
        let mut visible = String::with_capacity(line.len());
        let mut in_code = false;
        for character in line.chars() {
            if character == '`' {
                in_code = !in_code;
                continue;
            }
            if !in_code {
                visible.push(character);
            }
        }
        lines.push(visible);
    }
    lines
}

// shortcut: inline destinations only; use a shared Markdown parser when reference-style exports are added.
fn scan_line(line: &str, note_path: &str) -> Option<Vec<String>> {
    let mut found = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        let image = bytes[index] == b'!' && bytes[index + 1] == b'[';
        if image {
            index += 1;
        }
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        if index + 1 < bytes.len() && bytes[index + 1] == b'[' {
            // `![[target]]`: an attachment only when it is not a note.
            let rest = &line[index + 2..];
            let Some(end) = rest.find("]]") else {
                break;
            };
            let inner = rest[..end].split('|').next().unwrap_or("").trim();
            let without_anchor = inner.split('#').next().unwrap_or("").trim();
            // `![[#heading]]` embeds a section of this note; it names no file.
            if image
                && !without_anchor.is_empty()
                && is_attachment(without_anchor)
                && !is_note_path(without_anchor)
            {
                if let Some(resolved) = resolve_reference(note_path, without_anchor) {
                    found.push(resolved);
                }
            }
            index += 2 + end + 2;
            continue;
        }
        {
            let rest = &line[index + 1..];
            let Some(label_end) = rest.find(']') else {
                break;
            };
            let after = &rest[label_end + 1..];
            if !after.starts_with('(') {
                index += 1 + label_end + 1;
                continue;
            }
            let Some(close) = after.find(')') else {
                break;
            };
            let destination = &after[1..close].trim();
            // `<...>` groups a destination that contains spaces; otherwise a
            // destination may be followed by a title: `![a](p.png "title")`.
            let target = if let Some(inner) = destination
                .strip_prefix('<')
                .and_then(|rest| rest.split_once('>'))
                .map(|(path, _)| path)
            {
                inner
            } else {
                destination
                    .split_once(char::is_whitespace)
                    .map(|(path, _)| path)
                    .unwrap_or(destination)
            };
            let target = target.split('#').next().unwrap_or("");
            if let Some(resolved) = resolve_reference(note_path, target) {
                if !is_note_path(&resolved) {
                    found.push(resolved);
                }
            }
            index += 1 + label_end + 1 + close + 1;
            continue;
        }
    }
    Some(found).filter(|items| !items.is_empty())
}

/// Lists wiki links and embeds that other tools will show literally.
///
/// `notes` are the exported (path, source) pairs. The same syntax and target in
/// one note is reported once, and the order is stable so the UI does not
/// reshuffle when the vault listing changes.
pub fn compatibility_losses(notes: &[(&str, &str)]) -> Vec<CompatibilityLoss> {
    let mut found = Vec::new();
    for (path, source) in notes {
        for line in visible_lines(source) {
            scan_compatibility(&line, path, &mut found);
        }
    }
    found.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.syntax.cmp(&right.syntax))
            .then(left.target.cmp(&right.target))
    });
    found.dedup_by(|left, right| {
        left.path == right.path && left.syntax == right.syntax && left.target == right.target
    });
    found
}

fn scan_compatibility(line: &str, path: &str, found: &mut Vec<CompatibilityLoss>) {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] == b'\\' {
            // An escaped bracket pair is literal text, not a link.
            index += 2;
            continue;
        }
        let embed = bytes[index] == b'!'
            && index + 2 < bytes.len()
            && bytes[index + 1] == b'['
            && bytes[index + 2] == b'[';
        let link = bytes[index] == b'[' && bytes[index + 1] == b'[';
        if !embed && !link {
            index += 1;
            continue;
        }
        let start = index + if embed { 3 } else { 2 };
        let rest = &line[start..];
        let Some(end) = rest.find("]]") else {
            index += 1;
            continue;
        };
        let inner = rest[..end].split('|').next().unwrap_or("").trim();
        if !inner.is_empty() {
            found.push(CompatibilityLoss {
                path: path.to_owned(),
                syntax: if embed {
                    CompatibilitySyntax::Embed
                } else if inner.contains("#^") {
                    CompatibilitySyntax::BlockReference
                } else {
                    CompatibilitySyntax::WikiLink
                },
                target: inner.to_owned(),
            });
        }
        index = start + end + 2;
    }
}

/// Report file written at the root of a Markdown folder export (F25 §4).
pub const LOSS_REPORT_NAME: &str = "export-losses.md";

/// Picks the report file name, avoiding a note that already uses it.
pub fn loss_report_name(entries: &[ExportEntry]) -> String {
    let taken: std::collections::BTreeSet<String> = entries
        .iter()
        .map(|entry| entry.path.replace('\\', "/").to_ascii_lowercase())
        .collect();
    if !taken.contains(LOSS_REPORT_NAME) {
        return LOSS_REPORT_NAME.to_owned();
    }
    for index in 2u32.. {
        let candidate = format!("export-losses-{index}.md");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("an unbounded suffix loop always finds a free name")
}

/// Builds the manifest of a Markdown folder export from the files about to be
/// written. Duplicate and unsafe paths are refused here as well as in the plan,
/// so a malformed plan can never direct the host outside the staging folder.
pub fn folder_manifest(files: &[(&str, &[u8])]) -> Result<ExportManifest, String> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut entries = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        let normalized = path.replace('\\', "/");
        if normalized.is_empty() || !is_safe_relative(&normalized) {
            return Err(format!("unsafe export path: {path}"));
        }
        if !seen.insert(normalized.to_ascii_lowercase()) {
            return Err(format!("duplicate export path: {path}"));
        }
        entries.push(ManifestEntry {
            path: normalized,
            size: bytes.len() as u64,
            crc32: crc32(bytes),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(ExportManifest {
        format: ExportFormat::Markdown,
        entries,
        verified: false,
    })
}

/// Verifies a written folder against its manifest.
///
/// Every manifest entry must exist with the recorded size and CRC-32, and no
/// extra file may be present. A missing, truncated or substituted file is a
/// verification failure, never a successful export (F25-A02).
pub fn verify_folder(manifest: &ExportManifest, files: &[(&str, &[u8])]) -> Result<(), String> {
    if files.len() != manifest.entries.len() {
        return Err(format!(
            "file count mismatch: folder {}, manifest {}",
            files.len(),
            manifest.entries.len()
        ));
    }
    let mut actual: std::collections::BTreeMap<String, &[u8]> = std::collections::BTreeMap::new();
    for (path, bytes) in files {
        let normalized = path.replace('\\', "/");
        if actual.insert(normalized.clone(), *bytes).is_some() {
            return Err(format!("duplicate file: {normalized}"));
        }
    }
    for entry in &manifest.entries {
        let Some(bytes) = actual.get(&entry.path) else {
            return Err(format!("missing file: {}", entry.path));
        };
        if bytes.len() as u64 != entry.size {
            return Err(format!("size mismatch for {}", entry.path));
        }
        if crc32(bytes) != entry.crc32 {
            return Err(format!("crc mismatch for {}", entry.path));
        }
    }
    Ok(())
}

/// The loss report written into a Markdown folder export.
///
/// Unlike the ZIP format, which reports losses only in the UI, a folder export
/// carries the report with the notes so the information survives after the app
/// is closed (F25 §4, F25 §7). Wiki syntax that other tools will show literally
/// is listed too, because a Markdown folder is more likely to be opened
/// elsewhere than an archive built by this app.
pub fn folder_loss_report(plan: &ExportPlan, compatibility: &[CompatibilityLoss]) -> String {
    let mut report = loss_report(plan);
    if !compatibility.is_empty() {
        report.push_str("\n## Wiki syntax kept literally\n\n");
        for loss in compatibility {
            let syntax = match loss.syntax {
                CompatibilitySyntax::Embed => "embed",
                CompatibilitySyntax::WikiLink => "link",
                CompatibilitySyntax::BlockReference => "block reference",
            };
            report.push_str(&format!(
                "- `{}`: {} `{}`\n",
                loss.path, syntax, loss.target
            ));
        }
    }
    report.push_str(
        "\n## Not included\n\nCanvas and database view definitions are not persisted by this build, so none were included.\n",
    );
    report
}

/// A Markdown loss report the user can keep next to the export (F25 §7).
pub fn loss_report(plan: &ExportPlan) -> String {
    let mut report = String::from("# Export losses\n\n");
    if plan.issues.is_empty() {
        report.push_str("No losses were detected.\n");
        return report;
    }
    for issue in &plan.issues {
        report.push_str("- ");
        match issue {
            ExportIssue::Excluded(path) => {
                report.push_str(&format!("Excluded by policy: `{path}`\n"));
            }
            ExportIssue::ResourceMissing(path) => {
                report.push_str(&format!("Referenced resource missing: `{path}`\n"));
            }
            ExportIssue::EntryTooLarge(path) => {
                report.push_str(&format!("Too large to export in one entry: `{path}`\n"));
            }
            ExportIssue::AttachmentSkipped(path) => {
                report.push_str(&format!("Attachment skipped on request: `{path}`\n"));
            }
            ExportIssue::UnsafePath(path) => {
                report.push_str(&format!("Unsafe path not exported: `{path}`\n"));
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(include_attachments: bool) -> ExportOptions {
        ExportOptions {
            format: ExportFormat::Zip,
            include_attachments,
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn the_vault_scope_exports_notes_and_skips_internal_state() {
        let plan = plan_export(
            ExportScope::Vault,
            "",
            &strings(&["a.md", "notes/b.md", ".opennote/index.sqlite"]),
            &[],
            &strings(&["a.md", "notes/b.md"]),
            &options(true),
        )
        .expect("plan");
        assert_eq!(
            plan.entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.md", "notes/b.md"]
        );
        assert_eq!(
            plan.issues,
            vec![ExportIssue::Excluded(".opennote/index.sqlite".to_owned())]
        );
        assert!(!is_lossless(&plan));
    }

    #[test]
    fn caches_and_credentials_are_never_included() {
        // F25-A03: the archive must not contain tokens or device state.
        let plan = plan_export(
            ExportScope::Vault,
            "",
            &strings(&["a.md"]),
            &strings(&[
                "notes/.DS_Store",
                "index.sqlite",
                "search-history.json",
                "device-prefs.json",
            ]),
            &strings(&["a.md"]),
            &options(true),
        )
        .expect("plan");
        let paths: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, vec!["a.md"]);
        assert!(plan
            .issues
            .iter()
            .all(|issue| matches!(issue, ExportIssue::Excluded(_))));
    }

    #[test]
    fn a_note_scope_only_takes_that_note() {
        let plan = plan_export(
            ExportScope::Note,
            "notes/b.md",
            &strings(&["a.md", "notes/b.md"]),
            &[],
            &strings(&["a.md", "notes/b.md"]),
            &options(true),
        )
        .expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].path, "notes/b.md");
    }

    #[test]
    fn a_folder_scope_takes_the_folder_and_nested_notes_only() {
        let plan = plan_export(
            ExportScope::Folder,
            "notes",
            &strings(&["notes/a.md", "notes/deep/b.md", "other/c.md"]),
            &[],
            &strings(&["notes/a.md", "notes/deep/b.md", "other/c.md"]),
            &options(true),
        )
        .expect("plan");
        assert_eq!(
            plan.entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["notes/a.md", "notes/deep/b.md"]
        );
    }

    #[test]
    fn a_missing_attachment_is_reported_not_silently_dropped() {
        let plan = plan_export(
            ExportScope::Note,
            "a.md",
            &strings(&["a.md"]),
            &strings(&["img/gone.png", "img/here.png"]),
            &strings(&["a.md", "img/here.png"]),
            &options(true),
        )
        .expect("plan");
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(
            plan.issues,
            vec![ExportIssue::ResourceMissing("img/gone.png".to_owned())]
        );
    }

    #[test]
    fn skipping_attachments_is_reported_as_a_loss() {
        let plan = plan_export(
            ExportScope::Note,
            "a.md",
            &strings(&["a.md"]),
            &strings(&["img/p.png"]),
            &strings(&["a.md", "img/p.png"]),
            &options(false),
        )
        .expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.issues,
            vec![ExportIssue::AttachmentSkipped("img/p.png".to_owned())]
        );
        assert!(loss_report(&plan).contains("Attachment skipped"));
    }

    #[test]
    fn an_unsafe_path_is_refused() {
        let plan = plan_export(
            ExportScope::Vault,
            "",
            &strings(&["../escape.md", "a.md"]),
            &[],
            &strings(&["a.md"]),
            &options(true),
        )
        .expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert!(matches!(plan.issues[0], ExportIssue::UnsafePath(_)));
    }

    #[test]
    fn an_empty_selection_is_an_error_not_an_empty_archive() {
        assert_eq!(
            plan_export(
                ExportScope::Note,
                "missing.md",
                &strings(&["a.md"]),
                &[],
                &[],
                &options(true)
            ),
            Err(ExportPlanError::NothingToExport)
        );
    }

    #[test]
    fn the_zip_writer_produces_a_verifiable_stored_archive() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"# Title\n");
        writer.add("img/p.png", &[1, 2, 3, 4]);
        let manifest = writer.manifest();
        let bytes = writer.finish();
        assert!(bytes.starts_with(&0x0403_4b50u32.to_le_bytes()));
        assert_eq!(
            &bytes[bytes.len() - 22..bytes.len() - 18],
            &0x0605_4b50u32.to_le_bytes()
        );
        verify_zip(&bytes, &manifest).expect("manifest verifies");
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.entries[0].size, 8);
        // Deterministic timestamps mean two runs produce identical bytes.
        let mut again = ZipWriter::new();
        again.add("a.md", b"# Title\n");
        again.add("img/p.png", &[1, 2, 3, 4]);
        assert_eq!(again.finish(), bytes);
    }

    #[test]
    fn a_truncated_archive_fails_verification() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"content");
        let manifest = writer.manifest();
        let bytes = writer.finish();
        let truncated = &bytes[..bytes.len() / 2];
        assert!(verify_zip(truncated, &manifest).is_err());

        // Flip a byte of the first entry's CRC inside the central directory.
        let mut tampered = bytes.clone();
        let eocd = tampered
            .windows(4)
            .rposition(|window| window == 0x0605_4b50u32.to_le_bytes())
            .expect("eocd");
        let central_offset = u32::from_le_bytes([
            tampered[eocd + 16],
            tampered[eocd + 17],
            tampered[eocd + 18],
            tampered[eocd + 19],
        ]) as usize;
        tampered[central_offset + 16] ^= 0xFF;
        assert!(verify_zip(&tampered, &manifest).is_err());
    }

    #[test]
    fn every_incomplete_zip_prefix_is_refused_without_panicking() {
        let mut writer = ZipWriter::new();
        writer.add("notes/京都.md", b"content PK\x05\x06");
        let manifest = writer.manifest();
        let bytes = writer.finish();
        for length in 0..bytes.len() {
            assert!(
                verify_zip(&bytes[..length], &manifest).is_err(),
                "prefix {length}"
            );
        }
        verify_zip(&bytes, &manifest).unwrap();
    }

    #[test]
    fn stored_payload_damage_is_refused() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"original body");
        let manifest = writer.manifest();
        let mut bytes = writer.finish();
        bytes[30 + "a.md".len()] ^= 1;
        assert!(verify_zip(&bytes, &manifest).is_err());
    }

    #[test]
    fn damaged_local_records_and_directory_offsets_are_refused() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"content");
        let manifest = writer.manifest();
        let bytes = writer.finish();
        for offset in [0, 6, 8, 14, 18, 22, 26, 28, 30] {
            let mut damaged = bytes.clone();
            damaged[offset] ^= 1;
            assert!(
                verify_zip(&damaged, &manifest).is_err(),
                "local field {offset}"
            );
        }
        let mut damaged = bytes.clone();
        let central = 30 + "a.md".len() + b"content".len();
        damaged[central + 42] ^= 1;
        assert!(verify_zip(&damaged, &manifest).is_err());
    }

    #[test]
    fn damaged_end_record_fields_are_refused() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"content");
        let manifest = writer.manifest();
        let bytes = writer.finish();
        for offset in [4, 6, 8, 12, 16, 20] {
            let mut damaged = bytes.clone();
            let end = damaged.len() - 22;
            damaged[end + offset] ^= 1;
            assert!(
                verify_zip(&damaged, &manifest).is_err(),
                "end field {offset}"
            );
        }
        let mut damaged = bytes.clone();
        damaged.extend_from_slice(b"PK\x05\x06");
        assert!(verify_zip(&damaged, &manifest).is_err());
    }

    #[test]
    fn unicode_zip_names_are_marked_utf8_in_both_headers() {
        let mut writer = ZipWriter::new();
        writer.add("京都/内容.md", b"body");
        let manifest = writer.manifest();
        let bytes = writer.finish();
        let central = 30 + "京都/内容.md".len() + b"body".len();
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), 0x0800);
        assert_eq!(
            u16::from_le_bytes([bytes[central + 8], bytes[central + 9]]),
            0x0800
        );
        verify_zip(&bytes, &manifest).unwrap();
    }

    #[test]
    fn prior_unflagged_stored_archives_still_verify() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"");
        let manifest = writer.manifest();
        let mut bytes = writer.finish();
        bytes[6..8].fill(0);
        let central = 30 + "a.md".len();
        bytes[central + 8..central + 10].fill(0);
        verify_zip(&bytes, &manifest).unwrap();
    }

    #[test]
    fn a_manifest_with_a_stale_crc_is_refused() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"content");
        let mut manifest = writer.manifest();
        let bytes = writer.finish();
        manifest.entries[0].crc32 ^= 1;
        assert!(verify_zip(&bytes, &manifest).is_err());
        manifest.entries[0].crc32 ^= 1;
        manifest.entries[0].size += 1;
        assert!(verify_zip(&bytes, &manifest).is_err());
    }

    #[test]
    fn a_reader_can_extract_every_entry_by_local_headers() {
        // F25-A01: the archive must be readable by another tool. This walks the
        // local headers the same way a simple extractor would and compares the
        // bytes, which also proves the central offsets are consistent.
        let mut writer = ZipWriter::new();
        writer.add("a.md", "# Title\n".as_bytes());
        writer.add("notes/京都.md", "本文".as_bytes());
        let bytes = writer.finish();

        let mut cursor = 0usize;
        let mut found: Vec<(String, Vec<u8>)> = Vec::new();
        while cursor + 4 <= bytes.len()
            && u32::from_le_bytes([
                bytes[cursor],
                bytes[cursor + 1],
                bytes[cursor + 2],
                bytes[cursor + 3],
            ]) == 0x0403_4b50
        {
            let crc = u32::from_le_bytes([
                bytes[cursor + 14],
                bytes[cursor + 15],
                bytes[cursor + 16],
                bytes[cursor + 17],
            ]);
            let size = u32::from_le_bytes([
                bytes[cursor + 18],
                bytes[cursor + 19],
                bytes[cursor + 20],
                bytes[cursor + 21],
            ]) as usize;
            let name_len = u16::from_le_bytes([bytes[cursor + 26], bytes[cursor + 27]]) as usize;
            let name_start = cursor + 30;
            let name =
                String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).to_string();
            let data_start = name_start + name_len;
            let data = bytes[data_start..data_start + size].to_vec();
            assert_eq!(crc32(&data), crc, "stored crc must match the data");
            found.push((name, data));
            cursor = data_start + size;
        }
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "a.md");
        assert_eq!(found[0].1, "# Title\n".as_bytes());
        assert_eq!(found[1].0, "notes/京都.md");
        assert_eq!(String::from_utf8(found[1].1.clone()).unwrap(), "本文");
    }

    #[test]
    fn a_duplicate_entry_is_ignored_rather_than_written_twice() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"one");
        writer.add("a.md", b"two");
        assert_eq!(writer.manifest().entries.len(), 1);
    }

    #[test]
    fn staging_marks_the_manifest_verified_only_after_a_real_check() {
        let mut writer = ZipWriter::new();
        writer.add("a.md", b"x");
        let staged = stage(writer).expect("staged");
        assert!(staged.manifest.verified);
        assert_eq!(staged.manifest.entries.len(), 1);
        assert_eq!(STAGING_SUFFIX, ".opennote-partial");
    }

    #[test]
    fn a_pdf_page_link_refers_to_the_file() {
        assert_eq!(
            referenced_assets("notes/a.md", "[r](../assets/r.pdf#page=3)"),
            vec!["assets/r.pdf".to_owned()]
        );
        assert_eq!(
            referenced_assets("a.md", "[r](<assets/my r.pdf#page=12>)"),
            vec!["assets/my r.pdf".to_owned()]
        );
    }

    #[test]
    fn references_are_resolved_against_the_note_directory() {
        let source = "![a](../img/p.png)\n![b](local.png)\n![c](<with space.png>)\n";
        assert_eq!(
            referenced_assets("notes/deep/a.md", source),
            vec![
                "notes/deep/local.png".to_owned(),
                "notes/deep/with space.png".to_owned(),
                // One `..` pops only `deep`; `notes` remains.
                "notes/img/p.png".to_owned(),
            ]
        );
    }

    #[test]
    fn code_fences_and_inline_code_are_not_references() {
        let source = "![real](real.png)\n```\n![fake](fake.png)\n```\n`![inline](inline.png)`\n~~~\n![also](no.png)\n~~~\n";
        assert_eq!(
            referenced_assets("a.md", source),
            vec!["real.png".to_owned()]
        );
    }

    #[test]
    fn wiki_embeds_of_attachments_count_but_note_embeds_do_not() {
        let source = "![[img/pic.png]] ![[note.md]] ![[other#x]] ![[pic.png|200]]";
        assert_eq!(
            referenced_assets("a.md", source),
            vec!["img/pic.png".to_owned(), "pic.png".to_owned()]
        );
    }

    #[test]
    fn remote_and_absolute_targets_are_not_attachments() {
        let source = "![r](https://x/y.png) ![d](data:image/png;base64,AAA) ![a](/etc/x.png)";
        // The absolute path is returned so the planner can report UnsafePath,
        // while remote and data URLs are not vault content at all.
        assert_eq!(
            referenced_assets("a.md", source),
            vec!["/etc/x.png".to_owned()]
        );
    }

    #[test]
    fn a_reference_escaping_the_vault_keeps_its_dotdot() {
        assert_eq!(
            referenced_assets("a.md", "![x](../../secret.png)"),
            vec!["../../secret.png".to_owned()]
        );
    }

    #[test]
    fn a_title_after_the_destination_is_ignored() {
        assert_eq!(
            referenced_assets("a.md", "![x](p.png \"a title\")"),
            vec!["p.png".to_owned()]
        );
    }

    #[test]
    fn ordinary_attachment_links_are_exported_without_treating_notes_as_assets() {
        let source = "[PDF](../files/report.pdf#page=3) [data](<../files/raw data.bin>) \
            [extensionless](../files/LICENSE) [note](other.md#section) [here](#heading) \
            [web](https://example.org/file.pdf) [mail](mailto:a@example.org)";
        assert_eq!(
            referenced_assets("notes/a.md", source),
            vec!["files/LICENSE", "files/raw data.bin", "files/report.pdf",]
        );
    }

    #[test]
    fn generated_non_image_attachment_references_survive_export_scanning() {
        let reference = crate::reference_for(
            "assets/data.bin",
            crate::AttachmentKind::Binary,
            crate::AttachmentPreview::Download,
        );
        assert_eq!(
            referenced_assets("a.md", &reference),
            vec!["assets/data.bin"]
        );
    }

    #[test]
    fn ordinary_links_in_examples_or_escaped_text_do_not_export_files() {
        let source = "`[code](fake.pdf)`\n```md\n[fenced](fake.bin)\n```\n\\[escaped](fake.pdf)\n[real](real.pdf)";
        assert_eq!(referenced_assets("a.md", source), vec!["real.pdf"]);
    }

    #[test]
    fn disabled_attachment_export_reports_unknown_types_and_extensionless_files() {
        let plan = plan_export(
            ExportScope::Note,
            "a.md",
            &strings(&["a.md"]),
            &strings(&["assets/data.bin", "assets/LICENSE"]),
            &strings(&["a.md", "assets/data.bin", "assets/LICENSE"]),
            &options(false),
        )
        .unwrap();
        assert_eq!(plan.losses, ["assets/data.bin", "assets/LICENSE"]);
        assert!(!is_lossless(&plan));
    }

    #[test]
    fn wiki_syntax_is_reported_as_a_compatibility_loss() {
        let source = "![[note]] [[other|alias]] [[a#^b]] [[a#heading]]";
        assert_eq!(
            compatibility_losses(&[("notes/a.md", source)]),
            vec![
                CompatibilityLoss {
                    path: "notes/a.md".to_owned(),
                    syntax: CompatibilitySyntax::Embed,
                    target: "note".to_owned(),
                },
                CompatibilityLoss {
                    path: "notes/a.md".to_owned(),
                    syntax: CompatibilitySyntax::WikiLink,
                    target: "a#heading".to_owned(),
                },
                CompatibilityLoss {
                    path: "notes/a.md".to_owned(),
                    syntax: CompatibilitySyntax::WikiLink,
                    target: "other".to_owned(),
                },
                CompatibilityLoss {
                    path: "notes/a.md".to_owned(),
                    syntax: CompatibilitySyntax::BlockReference,
                    target: "a#^b".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn compatibility_losses_ignore_code_escapes_and_plain_markdown_links() {
        let source = "`![[fake]]`\n```\n[[fenced]]\n```\n\\[[escaped]]\n![[real]] ![[real|201]]\n[plain](other.md)";
        assert_eq!(
            compatibility_losses(&[("a.md", source)]),
            vec![CompatibilityLoss {
                path: "a.md".to_owned(),
                syntax: CompatibilitySyntax::Embed,
                target: "real".to_owned(),
            }]
        );
    }

    #[test]
    fn the_crc_of_known_bytes_matches_the_standard() {
        // "123456789" has the well-known CRC-32 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_folder_manifest_verifies_exact_bytes_and_refuses_drift() {
        let files: &[(&str, &[u8])] = &[
            ("notes/b.md", b"# B\n"),
            ("notes/a.md", b"# A\n"),
            ("img/p.png", b"\x89PNG"),
        ];
        let manifest = folder_manifest(files).expect("manifest");
        assert_eq!(manifest.format, ExportFormat::Markdown);
        assert!(!manifest.verified);
        assert_eq!(
            manifest
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["img/p.png", "notes/a.md", "notes/b.md"]
        );
        verify_folder(&manifest, files).expect("exact folder verifies");

        // F25-A02: a missing, truncated or extra file cannot pass verification.
        let missing: &[(&str, &[u8])] = &[("notes/a.md", b"# A\n"), ("img/p.png", b"\x89PNG")];
        assert!(verify_folder(&manifest, missing).is_err());
        let truncated: &[(&str, &[u8])] = &[
            ("notes/a.md", b"# A\n"),
            ("notes/b.md", b"#"),
            ("img/p.png", b"\x89PNG"),
        ];
        assert!(verify_folder(&manifest, truncated).is_err());
        let extra: &[(&str, &[u8])] = &[
            ("notes/a.md", b"# A\n"),
            ("notes/b.md", b"# B\n"),
            ("img/p.png", b"\x89PNG"),
            ("stray.txt", b"x"),
        ];
        assert!(verify_folder(&manifest, extra).is_err());
        let substituted: &[(&str, &[u8])] = &[
            ("notes/a.md", b"# X\n"),
            ("notes/b.md", b"# B\n"),
            ("img/p.png", b"\x89PNG"),
        ];
        assert!(verify_folder(&manifest, substituted).is_err());
    }

    #[test]
    fn a_folder_manifest_rejects_unsafe_and_duplicate_paths() {
        assert!(folder_manifest(&[("../outside.md", b"x")]).is_err());
        assert!(folder_manifest(&[("C:/win.md", b"x")]).is_err());
        assert!(folder_manifest(&[("/abs.md", b"x")]).is_err());
        assert!(folder_manifest(&[("", b"x")]).is_err());
        // Windows would treat these as the same file, so the writer must too.
        assert!(folder_manifest(&[("A.md", b"x"), ("a.md", b"y")]).is_err());
    }

    #[test]
    fn the_report_name_never_overwrites_a_note() {
        let entries = |paths: &[&str]| -> Vec<ExportEntry> {
            paths
                .iter()
                .map(|path| ExportEntry {
                    path: (*path).to_owned(),
                    size: 0,
                })
                .collect()
        };
        assert_eq!(loss_report_name(&entries(&["a.md"])), "export-losses.md");
        assert_eq!(
            loss_report_name(&entries(&["a.md", "Export-Losses.md"])),
            "export-losses-2.md"
        );
        assert_eq!(
            loss_report_name(&entries(&["export-losses.md", "export-losses-2.md"])),
            "export-losses-3.md"
        );
    }

    #[test]
    fn the_folder_report_carries_issues_compatibility_and_view_losses() {
        let plan = plan_export(
            ExportScope::Vault,
            "",
            &strings(&["a.md"]),
            &strings(&["img/missing.png"]),
            &strings(&["a.md"]),
            &options(true),
        )
        .expect("plan");
        let compatibility = vec![CompatibilityLoss {
            path: "a.md".to_owned(),
            syntax: CompatibilitySyntax::Embed,
            target: "b".to_owned(),
        }];
        let report = folder_loss_report(&plan, &compatibility);
        assert!(report.contains("Referenced resource missing"));
        assert!(report.contains("## Wiki syntax kept literally"));
        assert!(report.contains("`a.md`: embed `b`"));
        assert!(report.contains("## Not included"));
        assert!(report.contains("view definitions"));
    }
}
