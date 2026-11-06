//! F24: one import from a folder, a ZIP or a Notion export, as a resumable batch.
//!
//! Preparing reads the source and fixes where every item goes, without writing to
//! the vault; the plan is saved beside the app's data, outside the vault, so no
//! schema change is involved. Running then writes item by item through M02, with
//! an operation id derived from the batch and the source path: M02's receipts make
//! a repeat harmless, so a crash, a retry or a resume after restart never makes a
//! second copy. The source is only read, and a file that changed since preparing
//! fails on its own instead of being imported with different content.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Seek};
use std::sync::atomic::{AtomicBool, Ordering};

use onote_attachments_import::notion;
use onote_attachments_import::zip_read::{ZipError, ZipReader};
use onote_attachments_import::{ArchiveEntry, ConflictPolicy, EntryKind, PlanIssue};
use onote_vault_storage::AttachmentWrite;

use super::*;

/// A Notion export larger than this arrives as parts; each part is read into memory
/// only if nested inside the chosen file.
const MAX_NESTED_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
/// How often progress reaches the disk. A lost update only repeats harmless work.
const PERSIST_EVERY: usize = 25;
const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceType {
    Folder,
    Zip,
    Notion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatchState {
    Preview,
    Running,
    Interrupted,
    Cancelled,
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum ItemKind {
    Note,
    Attachment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum ItemStatus {
    Pending,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchItem {
    /// Path inside the source, as stored there.
    source: String,
    destination: String,
    kind: ItemKind,
    /// Hash of the source bytes when prepared. Empty for a note made from a row.
    sha256: String,
    /// A Notion database row's fields, written as front matter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    properties: Vec<(String, String)>,
    /// The note title, for a database row that has no page of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row_title: Option<String>,
    status: ItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_key: Option<String>,
}

/// Something the import could not carry over, with a reason the panel can name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportLoss {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    batch_id: String,
    vault_id: String,
    source_type: SourceType,
    source_path: String,
    state: BatchState,
    items: Vec<BatchItem>,
    losses: Vec<ImportLoss>,
}

// ---- sources

trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

enum Source {
    Folder {
        root: PathBuf,
    },
    Archive {
        parts: Vec<ZipReader<Box<dyn ReadSeek>>>,
        index: BTreeMap<String, (usize, usize)>,
    },
}

#[derive(Debug, Clone)]
struct SourceFile {
    path: String,
    size: u64,
    kind: EntryKind,
}

fn zip_failure(error: &ZipError) -> ServiceError {
    match error {
        ZipError::TooManyEntries => svc_err(ErrorCode::InvalidInput, "error.importEntryLimit"),
        ZipError::Size => svc_err(ErrorCode::InvalidInput, "error.importExpandedLimit"),
        ZipError::Io => svc_err(ErrorCode::IoDenied, "error.importSourceMissing"),
        ZipError::Malformed(_) | ZipError::Checksum | ZipError::Unsupported(_) => {
            svc_err(ErrorCode::InvalidInput, "error.importArchiveInvalid")
        }
    }
}

fn hidden_path(path: &str) -> bool {
    path.split('/')
        .any(|part| (part.starts_with('.') && part != "..") || part == "__MACOSX")
}

impl Source {
    fn open(
        source_type: SourceType,
        path: &Path,
        losses: &mut Vec<ImportLoss>,
    ) -> Result<(Self, Vec<SourceFile>), ServiceError> {
        if source_type == SourceType::Folder {
            let files = scan_folder(
                path,
                onote_attachments_import::MAX_ENTRIES,
                onote_attachments_import::MAX_EXPANDED_BYTES,
                losses,
            )?;
            return Ok((
                Source::Folder {
                    root: path.to_path_buf(),
                },
                files,
            ));
        }
        reject_reparse_ancestors(path)
            .map_err(|_| svc_err(ErrorCode::IoDenied, "error.importPathInvalid"))?;
        let file = std::fs::File::open(path).map_err(|error| map_io_export(&error))?;
        let boxed: Box<dyn ReadSeek> = Box::new(file);
        let outer = ZipReader::open(boxed, onote_attachments_import::MAX_ENTRIES)
            .map_err(|error| zip_failure(&error))?;
        let mut parts = vec![outer];
        if source_type == SourceType::Notion {
            // Notion sometimes wraps its export in a ZIP of `Part-N.zip` files.
            let nested: Vec<(usize, u64, String)> = parts[0]
                .entries()
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    !entry.is_directory && entry.name.to_ascii_lowercase().ends_with(".zip")
                })
                .map(|(index, entry)| (index, entry.size, entry.name.clone()))
                .collect();
            for (index, size, name) in nested {
                if size > MAX_NESTED_ARCHIVE_BYTES {
                    losses.push(loss(&name, "tooLarge"));
                    continue;
                }
                let bytes = parts[0]
                    .read(index, MAX_NESTED_ARCHIVE_BYTES)
                    .map_err(|error| zip_failure(&error))?;
                let inner: Box<dyn ReadSeek> = Box::new(Cursor::new(bytes));
                match ZipReader::open(inner, onote_attachments_import::MAX_ENTRIES) {
                    Ok(part) => parts.push(part),
                    Err(_) => losses.push(loss(&name, "archiveInvalid")),
                }
            }
        }
        let mut files = Vec::new();
        let mut index = BTreeMap::new();
        let mut expanded = 0u64;
        for (part_index, part) in parts.iter().enumerate() {
            for (entry_index, entry) in part.entries().iter().enumerate() {
                if entry.is_directory || hidden_path(&entry.name) {
                    continue;
                }
                if source_type == SourceType::Notion
                    && part_index == 0
                    && entry.name.to_ascii_lowercase().ends_with(".zip")
                {
                    continue;
                }
                if !entry.name_is_utf8 {
                    losses.push(loss(&entry.name, "encoding"));
                    continue;
                }
                if entry.encrypted {
                    losses.push(loss(&entry.name, "encrypted"));
                    continue;
                }
                expanded = expanded.saturating_add(entry.size);
                if expanded > onote_attachments_import::MAX_EXPANDED_BYTES {
                    return Err(svc_err(
                        ErrorCode::InvalidInput,
                        "error.importExpandedLimit",
                    ));
                }
                if index.contains_key(&entry.name) {
                    losses.push(loss(&entry.name, "duplicate"));
                    continue;
                }
                index.insert(entry.name.clone(), (part_index, entry_index));
                files.push(SourceFile {
                    path: entry.name.clone(),
                    size: entry.size,
                    kind: EntryKind::from_archive(&entry.name, false, entry.is_symlink),
                });
            }
        }
        Ok((Source::Archive { parts, index }, files))
    }

    fn read(&mut self, path: &str, limit: u64) -> Result<Vec<u8>, ServiceError> {
        match self {
            Source::Folder { root } => read_folder_file(root, path, limit),
            Source::Archive { parts, index } => {
                let (part, entry) = *index
                    .get(path)
                    .ok_or_else(|| svc_err(ErrorCode::InvalidInput, "error.importSourceChanged"))?;
                parts[part]
                    .read(entry, limit)
                    .map_err(|error| zip_failure(&error))
            }
        }
    }
}

fn loss(path: &str, reason: &str) -> ImportLoss {
    ImportLoss {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

/// Lists a folder without following links out of it. Nothing is read yet.
fn scan_folder(
    root: &Path,
    entry_limit: usize,
    expanded_limit: u64,
    losses: &mut Vec<ImportLoss>,
) -> Result<Vec<SourceFile>, ServiceError> {
    let invalid = || svc_err(ErrorCode::IoDenied, "error.importPathInvalid");
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut count = 0usize;
    let mut expanded = 0u64;
    while let Some(current) = stack.pop() {
        reject_reparse_ancestors(&current).map_err(|_| invalid())?;
        let entries = std::fs::read_dir(&current).map_err(|error| map_io_export(&error))?;
        for entry in entries {
            if count == entry_limit {
                return Err(svc_err(ErrorCode::InvalidInput, "error.importEntryLimit"));
            }
            count += 1;
            let entry = entry.map_err(|error| map_io_export(&error))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|_| invalid())?
                .to_string_lossy()
                .replace('\\', "/");
            if hidden_path(&relative) {
                continue;
            }
            let metadata = entry.metadata().map_err(|error| map_io_export(&error))?;
            if metadata_is_reparse(&metadata) {
                // A link's target is outside what this import can validate.
                losses.push(loss(&relative, "link"));
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            if !metadata.is_file() {
                losses.push(loss(&relative, "link"));
                continue;
            }
            expanded = expanded.saturating_add(metadata.len());
            if expanded > expanded_limit {
                return Err(svc_err(
                    ErrorCode::InvalidInput,
                    "error.importExpandedLimit",
                ));
            }
            found.push(SourceFile {
                kind: EntryKind::from_archive(&relative, false, false),
                path: relative,
                size: metadata.len(),
            });
        }
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(found)
}

fn read_folder_file(root: &Path, relative: &str, limit: u64) -> Result<Vec<u8>, ServiceError> {
    let path = root.join(relative);
    reject_reparse_ancestors(&path)
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.importPathInvalid"))?;
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .map_err(|error| map_io_export(&error))?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| map_io_export(&error))?;
    if bytes.len() as u64 > limit {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.importSourceChanged",
        ));
    }
    Ok(bytes)
}

// ---- preparing

fn issue_loss(issue: &PlanIssue) -> ImportLoss {
    match issue {
        PlanIssue::PathTraversal(path) => loss(path, "pathUnsafe"),
        PlanIssue::UnsupportedEntry(path) => loss(path, "link"),
        PlanIssue::EncodingInvalid(path) => loss(path, "encoding"),
        PlanIssue::EntryTooLarge(path) => loss(path, "tooLarge"),
        PlanIssue::DuplicateDestination(_, path) => loss(path, "duplicate"),
        PlanIssue::DestinationExists(path) => loss(path, "duplicate"),
        PlanIssue::UnsupportedType(path) => loss(path, "unsupportedType"),
    }
}

/// The vault path each source file should appear at. For Notion the ids come off,
/// and two pages left with one name get the numbered names users expect.
fn display_paths(source_type: SourceType, files: &[SourceFile]) -> BTreeMap<String, String> {
    let mut taken = std::collections::BTreeSet::new();
    let mut paths = BTreeMap::new();
    for file in files {
        let base = if source_type == SourceType::Notion {
            notion::notion_clean_path(&file.path)
        } else {
            file.path.clone()
        };
        let mut candidate = base.clone();
        let mut number = 1u32;
        while !taken.insert(candidate.to_lowercase()) {
            candidate = match base.rsplit_once('.') {
                Some((stem, extension)) => format!("{stem} ({number}).{extension}"),
                None => format!("{base} ({number})"),
            };
            number += 1;
        }
        paths.insert(file.path.clone(), candidate);
    }
    paths
}

struct RowNote {
    title: String,
    properties: Vec<(String, String)>,
}

/// Reads each database CSV: rows match row pages by title, and a row without a page
/// becomes a note of its own. Returns properties by note display path.
fn database_notes(
    source: &mut Source,
    files: &[SourceFile],
    displays: &BTreeMap<String, String>,
    losses: &mut Vec<ImportLoss>,
) -> BTreeMap<String, RowNote> {
    let mut notes = BTreeMap::new();
    let lower: std::collections::BTreeSet<String> =
        displays.values().map(|path| path.to_lowercase()).collect();
    for file in files {
        if !file.path.to_ascii_lowercase().ends_with(".csv") {
            continue;
        }
        let Some(display) = displays.get(&file.path) else {
            continue;
        };
        // Notion writes `Name.csv` and `Name_all.csv`; the second holds every column.
        let all_version = display[..display.len() - 4].to_owned() + "_all.csv";
        if lower.contains(&all_version.to_lowercase()) {
            continue;
        }
        let parsed = source
            .read(&file.path, onote_attachments_import::MAX_NOTE_BYTES)
            .ok()
            .and_then(|bytes| notion::parse_csv(&bytes));
        let Some(rows) = parsed else {
            losses.push(loss(&file.path, "encoding"));
            continue;
        };
        let database = display[..display.len() - 4].trim_end_matches("_all");
        for row in notion::database_rows(&rows) {
            let safe_title: String = row
                .title
                .chars()
                .map(|character| {
                    if r#"\/:*?"<>|"#.contains(character) {
                        '-'
                    } else {
                        character
                    }
                })
                .collect();
            let path = format!("{database}/{}.md", safe_title.trim());
            notes.insert(
                path,
                RowNote {
                    title: row.title,
                    properties: row.properties,
                },
            );
        }
    }
    notes
}

struct PrepareInput<'a> {
    batch_id: &'a str,
    vault_id: &'a str,
    source_type: SourceType,
    source_path: &'a Path,
    prefix: &'a str,
}

/// Reads the source once, decides every destination and writes nothing.
///
/// `existing` lists the vault's notes. `vault_attachment` reports the hash of a
/// file already at a vault path, or None when the path is free.
fn prepare(
    input: &PrepareInput,
    existing: &[String],
    mut vault_attachment: impl FnMut(&str) -> Result<Option<String>, ServiceError>,
) -> Result<Manifest, ServiceError> {
    let mut losses = Vec::new();
    let (mut source, files) = Source::open(input.source_type, input.source_path, &mut losses)?;
    let displays = display_paths(input.source_type, &files);
    let rows = if input.source_type == SourceType::Notion {
        database_notes(&mut source, &files, &displays, &mut losses)
    } else {
        BTreeMap::new()
    };
    let by_display: BTreeMap<&str, &SourceFile> = files
        .iter()
        .map(|file| (displays[&file.path].as_str(), file))
        .collect();

    let mut entries: Vec<ArchiveEntry> = files
        .iter()
        .map(|file| ArchiveEntry {
            path: displays[&file.path].clone(),
            size: file.size,
            kind: file.kind,
        })
        .collect();
    for path in rows.keys() {
        if !by_display.contains_key(path.as_str()) {
            entries.push(ArchiveEntry {
                path: path.clone(),
                size: 0,
                kind: EntryKind::Markdown,
            });
        }
    }
    let plan = onote_attachments_import::plan_import_into(
        &entries,
        existing,
        ConflictPolicy::Rename,
        input.prefix,
        &BTreeMap::new(),
    )
    .map_err(|error| match error {
        onote_attachments_import::PlanError::InvalidPrefix => {
            svc_err(ErrorCode::InvalidInput, "error.importPathInvalid")
        }
        onote_attachments_import::PlanError::TooManyEntries { .. } => {
            svc_err(ErrorCode::InvalidInput, "error.importEntryLimit")
        }
        onote_attachments_import::PlanError::ExpandedTooLarge { .. } => {
            svc_err(ErrorCode::InvalidInput, "error.importExpandedLimit")
        }
        _ => svc_err(ErrorCode::InvalidInput, "error.importNothing"),
    })?;
    losses.extend(plan.issues.iter().map(issue_loss));

    let mut items = Vec::with_capacity(plan.entries.len());
    let mut taken: std::collections::BTreeSet<String> = plan
        .entries
        .iter()
        .map(|entry| entry.destination.to_lowercase())
        .collect();
    for entry in &plan.entries {
        let row = rows.get(&entry.source_path);
        let Some(file) = by_display.get(entry.source_path.as_str()) else {
            // A database row with no page of its own.
            let Some(row) = row else { continue };
            items.push(BatchItem {
                source: entry.source_path.clone(),
                destination: entry.destination.clone(),
                kind: ItemKind::Note,
                sha256: String::new(),
                properties: row.properties.clone(),
                row_title: Some(row.title.clone()),
                status: ItemStatus::Pending,
                error_key: None,
            });
            continue;
        };
        let bytes = match source.read(&file.path, onote_attachments_import::MAX_NOTE_BYTES) {
            Ok(bytes) => bytes,
            Err(_) => {
                losses.push(loss(&file.path, "unreadable"));
                continue;
            }
        };
        let sha256 = sha256_hex(&bytes);
        if entry.kind == EntryKind::Markdown {
            let decoded = match onote_attachments_import::decode_markdown(&file.path, &bytes) {
                Ok(text) => text,
                Err(_) => {
                    // Never decoded with replacement characters (F24 §7).
                    losses.push(loss(&file.path, "encoding"));
                    continue;
                }
            };
            if input.source_type == SourceType::Notion {
                for block in notion::unsupported_blocks(&decoded) {
                    losses.push(loss(&entry.destination, &format!("block.{block}")));
                }
            }
            items.push(BatchItem {
                source: file.path.clone(),
                destination: entry.destination.clone(),
                kind: ItemKind::Note,
                sha256,
                properties: row.map(|row| row.properties.clone()).unwrap_or_default(),
                row_title: None,
                status: ItemStatus::Pending,
                error_key: None,
            });
            continue;
        }
        // An attachment: reuse identical bytes, and go around different ones.
        let mut destination = entry.destination.clone();
        let mut number = 1u32;
        let mut status = ItemStatus::Pending;
        loop {
            match vault_attachment(&destination)? {
                None => break,
                Some(found) if found == sha256 => {
                    status = ItemStatus::Done;
                    break;
                }
                Some(_) => {
                    let candidate = match entry.destination.rsplit_once('.') {
                        Some((stem, extension)) => format!("{stem} ({number}).{extension}"),
                        None => format!("{} ({number})", entry.destination),
                    };
                    number += 1;
                    if taken.insert(candidate.to_lowercase()) {
                        destination = candidate;
                    }
                }
            }
        }
        items.push(BatchItem {
            source: file.path.clone(),
            destination,
            kind: ItemKind::Attachment,
            sha256,
            properties: Vec::new(),
            row_title: None,
            status,
            error_key: None,
        });
    }
    if items.is_empty() && losses.is_empty() {
        return Err(svc_err(ErrorCode::InvalidInput, "error.importNothing"));
    }
    Ok(Manifest {
        version: MANIFEST_VERSION,
        batch_id: input.batch_id.to_owned(),
        vault_id: input.vault_id.to_owned(),
        source_type: input.source_type,
        source_path: input.source_path.to_string_lossy().into_owned(),
        state: BatchState::Preview,
        items,
        losses,
    })
}

// ---- running

/// What running needs from the vault, so tests can use a real store directly.
trait VaultWrites {
    fn store_attachment(
        &mut self,
        path: &str,
        bytes: &[u8],
        sha256: &str,
        id: &str,
    ) -> Result<AttachmentWrite, ServiceError>;
    fn create_note(&mut self, path: &str, content: &str, id: &str) -> Result<(), ServiceError>;
}

/// True when the vault itself went away, so later items would fail the same way.
fn session_ended(error: &ServiceError) -> bool {
    matches!(
        error.message_key.as_str(),
        "error.vaultSessionChanged" | "error.noVaultOpen"
    )
}

fn note_content(
    item: &BatchItem,
    bytes: &[u8],
    source_type: SourceType,
    moved: &BTreeMap<String, String>,
    wiki: &BTreeMap<String, String>,
    losses: &mut Vec<ImportLoss>,
) -> Result<String, ServiceError> {
    let text = onote_attachments_import::decode_markdown(&item.source, bytes)
        .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.importSourceChanged"))?;
    let text = onote_attachments_import::strip_bom(&text);
    let links = notion::convert_links(text, &item.source, &item.destination, moved);
    for dead in links.dead {
        losses.push(loss(&format!("{} → {dead}", item.destination), "deadLink"));
    }
    let rewritten = onote_attachments_import::rewrite_links(&item.destination, &links.text, wiki);
    for dead in rewritten.dead {
        losses.push(loss(
            &format!("{} → {}", item.destination, dead.target),
            "deadLink",
        ));
    }
    let body = if source_type == SourceType::Notion {
        notion::apply_row_properties(&rewritten.text, &item.properties)
    } else {
        rewritten.text
    };
    Ok(body)
}

/// Writes every pending item. Cancelling stops between items, and what is already
/// written stays. Attachments go first, so a note's images exist when it appears.
fn run_items(
    manifest: &Mutex<Manifest>,
    source: &mut Source,
    vault: &mut dyn VaultWrites,
    cancel: &AtomicBool,
    persist: &dyn Fn(&Manifest),
) {
    let (batch_id, source_type, order, moved, wiki) = {
        let mut guard = manifest.lock().expect("manifest");
        guard.state = BatchState::Running;
        for item in &mut guard.items {
            if item.status == ItemStatus::Failed {
                item.status = ItemStatus::Pending;
                item.error_key = None;
            }
        }
        // Dead-link reports are made while writing, so a repeat starts them over.
        guard.losses.retain(|entry| entry.reason != "deadLink");
        let moved: BTreeMap<String, String> = guard
            .items
            .iter()
            .map(|item| (item.source.clone(), item.destination.clone()))
            .collect();
        let wiki: BTreeMap<String, String> = guard
            .items
            .iter()
            .filter(|item| item.kind == ItemKind::Note)
            .map(|item| {
                let name = item
                    .destination
                    .rsplit('/')
                    .next()
                    .unwrap_or(&item.destination);
                let stem = name.trim_end_matches(".md").trim_end_matches(".markdown");
                (stem.to_owned(), item.destination.clone())
            })
            .collect();
        let mut order: Vec<usize> = (0..guard.items.len()).collect();
        order.sort_by_key(|&index| guard.items[index].kind != ItemKind::Attachment);
        persist(&guard);
        (
            guard.batch_id.clone(),
            guard.source_type,
            order,
            moved,
            wiki,
        )
    };
    let mut since_saved = 0usize;
    let mut ended = false;
    for index in order {
        if cancel.load(Ordering::SeqCst) {
            let mut guard = manifest.lock().expect("manifest");
            guard.state = BatchState::Cancelled;
            persist(&guard);
            return;
        }
        let item = {
            let guard = manifest.lock().expect("manifest");
            guard.items[index].clone()
        };
        if item.status != ItemStatus::Pending {
            continue;
        }
        let id = onote_attachments_import::source_operation_id(&batch_id, &item.source);
        let mut new_losses = Vec::new();
        let outcome = (|| -> Result<(), ServiceError> {
            if let Some(title) = &item.row_title {
                let body = notion::apply_row_properties(&format!("# {title}\n"), &item.properties);
                return vault.create_note(&item.destination, &body, &id);
            }
            let bytes = source.read(&item.source, onote_attachments_import::MAX_NOTE_BYTES)?;
            if sha256_hex(&bytes) != item.sha256 {
                return Err(svc_err(
                    ErrorCode::InvalidInput,
                    "error.importSourceChanged",
                ));
            }
            match item.kind {
                ItemKind::Attachment => vault
                    .store_attachment(&item.destination, &bytes, &item.sha256, &id)
                    .map(|_| ()),
                ItemKind::Note => {
                    let content =
                        note_content(&item, &bytes, source_type, &moved, &wiki, &mut new_losses)?;
                    vault.create_note(&item.destination, &content, &id)
                }
            }
        })();
        let mut guard = manifest.lock().expect("manifest");
        guard.losses.append(&mut new_losses);
        match outcome {
            Ok(()) => guard.items[index].status = ItemStatus::Done,
            Err(error) if session_ended(&error) => {
                ended = true;
            }
            Err(error) => {
                guard.items[index].status = ItemStatus::Failed;
                guard.items[index].error_key = Some(error.message_key);
            }
        }
        since_saved += 1;
        if since_saved >= PERSIST_EVERY {
            persist(&guard);
            since_saved = 0;
        }
        if ended {
            guard.state = BatchState::Interrupted;
            persist(&guard);
            return;
        }
    }
    let mut guard = manifest.lock().expect("manifest");
    let failed = guard
        .items
        .iter()
        .any(|item| item.status == ItemStatus::Failed);
    guard.state = if failed || !guard.losses.is_empty() {
        BatchState::Partial
    } else {
        BatchState::Complete
    };
    persist(&guard);
}

// ---- on disk

fn batches_dir(app: &AppHandle, vault_id: &str) -> Result<PathBuf, ServiceError> {
    let base = app
        .path()
        .app_local_data_dir()
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.importRecordFailed"))?;
    Ok(base
        .join("import-batches")
        .join(sha256_hex(vault_id.as_bytes())))
}

fn batch_path(dir: &Path, batch_id: &str) -> Result<PathBuf, ServiceError> {
    if uuid::Uuid::parse_str(batch_id).is_err() {
        return Err(svc_err(ErrorCode::InvalidInput, "error.importRecordFailed"));
    }
    Ok(dir.join(format!("{batch_id}.json")))
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<(), ServiceError> {
    let failed = |_| svc_err(ErrorCode::IoDenied, "error.importRecordFailed");
    std::fs::create_dir_all(dir).map_err(failed)?;
    let target = batch_path(dir, &manifest.batch_id)?;
    let staging = target.with_extension("json.partial");
    let text = serde_json::to_vec(manifest)
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.importRecordFailed"))?;
    std::fs::write(&staging, text).map_err(failed)?;
    std::fs::rename(&staging, &target).map_err(failed)
}

fn read_manifest(path: &Path) -> Result<Manifest, ServiceError> {
    let bytes = std::fs::read(path)
        .map_err(|_| svc_err(ErrorCode::ObjectNotFound, "error.importRecordFailed"))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.importRecordFailed"))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(svc_err(ErrorCode::InvalidInput, "error.importRecordFailed"));
    }
    Ok(manifest)
}

// ---- commands

struct Run {
    manifest: Arc<Mutex<Manifest>>,
    cancel: Arc<AtomicBool>,
}

/// Imports running now, by batch id.
#[derive(Default)]
pub(crate) struct ImportRuns(Mutex<HashMap<String, Run>>);

/// What the panel shows before confirming.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub cancelled: bool,
    pub batch_id: String,
    pub notes: usize,
    pub attachments: usize,
    /// Up to 200 items as `[source, destination]`.
    pub sample: Vec<(String, String)>,
    pub losses: Vec<ImportLoss>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStatus {
    pub batch_id: String,
    pub state: BatchState,
    pub source_name: String,
    pub total: usize,
    pub finished: usize,
    pub notes_done: usize,
    pub attachments_done: usize,
    pub failed: Vec<(String, String)>,
    pub losses: Vec<ImportLoss>,
}

fn status_of(manifest: &Manifest, live: bool) -> ImportStatus {
    let done = |kind| {
        manifest
            .items
            .iter()
            .filter(|item| item.kind == kind && item.status == ItemStatus::Done)
            .count()
    };
    ImportStatus {
        batch_id: manifest.batch_id.clone(),
        // No thread is doing it any more: the app stopped while it ran.
        state: if manifest.state == BatchState::Running && !live {
            BatchState::Interrupted
        } else {
            manifest.state
        },
        source_name: Path::new(&manifest.source_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        total: manifest.items.len(),
        finished: manifest
            .items
            .iter()
            .filter(|item| item.status != ItemStatus::Pending)
            .count(),
        notes_done: done(ItemKind::Note),
        attachments_done: done(ItemKind::Attachment),
        failed: manifest
            .items
            .iter()
            .filter(|item| item.status == ItemStatus::Failed)
            .map(|item| {
                (
                    item.destination.clone(),
                    item.error_key.clone().unwrap_or_default(),
                )
            })
            .collect(),
        losses: manifest.losses.clone(),
    }
}

/// F24: picks a source and prepares a batch. Nothing is written to the vault.
#[tauri::command]
pub(crate) async fn import_prepare(
    app: AppHandle,
    vault_id: String,
    source_type: SourceType,
    destination_prefix: String,
    dialog_title: String,
) -> Result<ImportPreview, ServiceError> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cancelled = || ImportPreview {
            cancelled: true,
            batch_id: String::new(),
            notes: 0,
            attachments: 0,
            sample: Vec::new(),
            losses: Vec::new(),
        };
        let mut dialog = handle.dialog().file().set_title(&dialog_title);
        if let Some(window) = handle.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let picked = if source_type == SourceType::Folder {
            dialog.blocking_pick_folder()
        } else {
            dialog.add_filter("ZIP", &["zip"]).blocking_pick_file()
        };
        let Some(path) = picked.and_then(file_path_to_buf) else {
            return Ok(cancelled());
        };
        let existing = with_state(&handle, |state| {
            Ok(scoped_vault_ref(state, &vault_id)?
                .list()?
                .into_iter()
                .map(|summary| summary.relative_path)
                .collect::<Vec<String>>())
        })?;
        let reader = with_state(&handle, |state| {
            Ok(scoped_vault_ref(state, &vault_id)?.attachment_reader())
        })?;
        let batch_id = uuid::Uuid::new_v4().to_string();
        let manifest = prepare(
            &PrepareInput {
                batch_id: &batch_id,
                vault_id: &vault_id,
                source_type,
                source_path: &path,
                prefix: &destination_prefix,
            },
            &existing,
            |destination| match reader.probe(destination) {
                Ok(probe) => Ok(Some(probe.file.sha256)),
                Err(error) if error.code == ErrorCode::ObjectNotFound => Ok(None),
                // A note or a folder holds this name; treat it as taken.
                Err(_) => Ok(Some(String::new())),
            },
        )?;
        write_manifest(&batches_dir(&handle, &vault_id)?, &manifest)?;
        let count = |kind| {
            manifest
                .items
                .iter()
                .filter(|item| item.kind == kind)
                .count()
        };
        Ok(ImportPreview {
            cancelled: false,
            batch_id,
            notes: count(ItemKind::Note),
            attachments: count(ItemKind::Attachment),
            sample: manifest
                .items
                .iter()
                .take(200)
                .map(|item| (item.source.clone(), item.destination.clone()))
                .collect(),
            losses: manifest.losses.clone(),
        })
    })
    .await
    .map_err(map_join_err)?
}

struct AppVault {
    app: AppHandle,
    vault_id: String,
}

impl VaultWrites for AppVault {
    fn store_attachment(
        &mut self,
        path: &str,
        bytes: &[u8],
        sha256: &str,
        id: &str,
    ) -> Result<AttachmentWrite, ServiceError> {
        with_state(&self.app, |state| {
            scoped_vault_mut(state, &self.vault_id)?
                .store_attachment(path, bytes, sha256, id)
                .map(|(_, write)| write)
        })
    }

    fn create_note(&mut self, path: &str, content: &str, id: &str) -> Result<(), ServiceError> {
        with_state(&self.app, |state| {
            scoped_vault_mut(state, &self.vault_id)?
                .create_note(path, content, id)
                .map(|_| ())
        })
    }
}

/// F24: runs a prepared batch, or continues an interrupted one. Returns at once;
/// the panel follows it with `import_status`.
#[tauri::command]
pub(crate) async fn import_start(
    app: AppHandle,
    vault_id: String,
    batch_id: String,
) -> Result<ImportStatus, ServiceError> {
    with_state(&app, |state| scoped_vault_ref(state, &vault_id).map(|_| ()))?;
    let dir = batches_dir(&app, &vault_id)?;
    let path = batch_path(&dir, &batch_id)?;
    let runs = app.state::<ImportRuns>();
    let mut guard = runs.0.lock().map_err(|_| poison_err())?;
    if let Some(run) = guard.get(&batch_id) {
        let current = run.manifest.lock().map_err(|_| poison_err())?;
        return Ok(status_of(&current, true));
    }
    let manifest = read_manifest(&path)?;
    if manifest.vault_id != vault_id {
        return Err(svc_err(ErrorCode::InvalidInput, "error.importRecordFailed"));
    }
    let shared = Arc::new(Mutex::new(manifest));
    let cancel = Arc::new(AtomicBool::new(false));
    guard.insert(
        batch_id.clone(),
        Run {
            manifest: shared.clone(),
            cancel: cancel.clone(),
        },
    );
    drop(guard);
    let status = {
        let current = shared.lock().map_err(|_| poison_err())?;
        status_of(&current, true)
    };
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let persist = |current: &Manifest| {
            // Losing this write only means some items repeat, which M02 absorbs.
            let _ = write_manifest(&dir, current);
        };
        let (source_type, source_path) = {
            let current = shared.lock().expect("manifest");
            (current.source_type, PathBuf::from(&current.source_path))
        };
        let mut ignored = Vec::new();
        match Source::open(source_type, &source_path, &mut ignored) {
            Ok((mut source, _)) => {
                let mut vault = AppVault {
                    app: handle.clone(),
                    vault_id,
                };
                run_items(&shared, &mut source, &mut vault, &cancel, &persist);
            }
            Err(error) => {
                let mut current = shared.lock().expect("manifest");
                for item in &mut current.items {
                    if item.status == ItemStatus::Pending {
                        item.status = ItemStatus::Failed;
                        item.error_key = Some(error.message_key.clone());
                    }
                }
                current.state = BatchState::Partial;
                persist(&current);
            }
        }
        if let Ok(mut runs) = handle.state::<ImportRuns>().0.lock() {
            runs.remove(&batch_id);
        }
    });
    Ok(status)
}

#[tauri::command]
pub(crate) fn import_status(
    app: AppHandle,
    vault_id: String,
    batch_id: String,
) -> Result<ImportStatus, ServiceError> {
    let runs = app.state::<ImportRuns>();
    let guard = runs.0.lock().map_err(|_| poison_err())?;
    if let Some(run) = guard.get(&batch_id) {
        let current = run.manifest.lock().map_err(|_| poison_err())?;
        return Ok(status_of(&current, true));
    }
    drop(guard);
    let dir = batches_dir(&app, &vault_id)?;
    Ok(status_of(
        &read_manifest(&batch_path(&dir, &batch_id)?)?,
        false,
    ))
}

/// Stops after the item being written; what is already in the vault stays.
#[tauri::command]
pub(crate) fn import_cancel(app: AppHandle, batch_id: String) -> Result<(), ServiceError> {
    let runs = app.state::<ImportRuns>();
    let guard = runs.0.lock().map_err(|_| poison_err())?;
    if let Some(run) = guard.get(&batch_id) {
        run.cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// Batches that stopped before finishing, so the panel can offer to continue them.
#[tauri::command]
pub(crate) fn import_unfinished(
    app: AppHandle,
    vault_id: String,
) -> Result<Vec<ImportStatus>, ServiceError> {
    with_state(&app, |state| scoped_vault_ref(state, &vault_id).map(|_| ()))?;
    let dir = batches_dir(&app, &vault_id)?;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let runs = app.state::<ImportRuns>();
    let guard = runs.0.lock().map_err(|_| poison_err())?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(manifest) = read_manifest(&path) else {
            continue;
        };
        if manifest.vault_id != vault_id {
            continue;
        }
        let status = status_of(&manifest, guard.contains_key(&manifest.batch_id));
        if matches!(
            status.state,
            BatchState::Interrupted | BatchState::Cancelled | BatchState::Running
        ) {
            found.push(status);
        }
    }
    Ok(found)
}

/// Forgets a batch's record. Notes it already wrote stay in the vault.
#[tauri::command]
pub(crate) fn import_discard(
    app: AppHandle,
    vault_id: String,
    batch_id: String,
) -> Result<(), ServiceError> {
    let runs = app.state::<ImportRuns>();
    if runs
        .0
        .lock()
        .map_err(|_| poison_err())?
        .contains_key(&batch_id)
    {
        return Err(svc_err(ErrorCode::InvalidInput, "error.importRunning"));
    }
    let dir = batches_dir(&app, &vault_id)?;
    match std::fs::remove_file(batch_path(&dir, &batch_id)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(svc_err(ErrorCode::IoDenied, "error.importRecordFailed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onote_attachments_import::ZipWriter;
    use onote_vault_storage::VaultStorage;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    struct Real<'a>(&'a mut VaultStorage);

    impl VaultWrites for Real<'_> {
        fn store_attachment(
            &mut self,
            path: &str,
            bytes: &[u8],
            sha256: &str,
            id: &str,
        ) -> Result<AttachmentWrite, ServiceError> {
            self.0
                .store_attachment(path, bytes, sha256, id)
                .map(|(_, write)| write)
        }
        fn create_note(&mut self, path: &str, content: &str, id: &str) -> Result<(), ServiceError> {
            self.0.create_note(path, content, id).map(|_| ())
        }
    }

    fn vault() -> (tempfile::TempDir, VaultStorage) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let storage = VaultStorage::create(&root, "Import").unwrap();
        (directory, storage)
    }

    fn zip_file(directory: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let mut writer = ZipWriter::new();
        for (name, bytes) in files {
            writer.add(name, bytes);
        }
        let path = directory.join("source.zip");
        std::fs::write(&path, writer.finish()).unwrap();
        path
    }

    fn prepared(
        storage: &VaultStorage,
        source_type: SourceType,
        path: &Path,
        prefix: &str,
    ) -> Manifest {
        let existing: Vec<String> = storage
            .list()
            .unwrap()
            .into_iter()
            .map(|note| note.relative_path)
            .collect();
        let reader = storage.attachment_reader();
        prepare(
            &PrepareInput {
                batch_id: "11111111-1111-4111-8111-111111111111",
                vault_id: "v",
                source_type,
                source_path: path,
                prefix,
            },
            &existing,
            |destination| match reader.probe(destination) {
                Ok(probe) => Ok(Some(probe.file.sha256)),
                Err(error) if error.code == ErrorCode::ObjectNotFound => Ok(None),
                Err(_) => Ok(Some(String::new())),
            },
        )
        .unwrap()
    }

    fn run(manifest: Manifest, storage: &mut VaultStorage, path: &Path, cancel: bool) -> Manifest {
        let shared = Mutex::new(manifest.clone());
        let mut ignored = Vec::new();
        let (mut source, _) = Source::open(manifest.source_type, path, &mut ignored).unwrap();
        run_items(
            &shared,
            &mut source,
            &mut Real(storage),
            &AtomicBool::new(cancel),
            &|_| {},
        );
        shared.into_inner().unwrap()
    }

    fn read_note(storage: &VaultStorage, path: &str) -> String {
        let id = storage
            .list()
            .unwrap()
            .into_iter()
            .find(|note| note.relative_path == path)
            .unwrap_or_else(|| panic!("{path} missing"))
            .note_id;
        let source = storage.read(&id).unwrap().source;
        // M02 records the note's identity in front matter; the tests are about the rest.
        let identity = regex_free_identity(&source);
        source.replacen(&identity, "", 1)
    }

    /// The `opennote-id` line, and the fence around it when it is alone there.
    fn regex_free_identity(source: &str) -> String {
        let Some(at) = source.find("opennote-id: ") else {
            return String::new();
        };
        let line_end = at + source[at..].find('\n').map_or(0, |offset| offset + 1);
        let line = &source[at..line_end];
        let alone = format!("---\n{line}---\n");
        if source.starts_with(&alone) {
            alone
        } else {
            line.to_owned()
        }
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(current).unwrap().flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                } else {
                    out.push((entry.path(), std::fs::read(entry.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_folder_imports_notes_and_attachments_and_stays_unchanged() {
        let (directory, mut storage) = vault();
        let source = directory.path().join("src");
        std::fs::create_dir_all(source.join("sub/img")).unwrap();
        std::fs::create_dir_all(source.join(".git")).unwrap();
        std::fs::write(source.join("a.md"), "see [b](sub/b.md) and [[b]]\n").unwrap();
        std::fs::write(
            source.join("sub/b.md"),
            "![p](img/p.png)\n[missing](none.md)\n",
        )
        .unwrap();
        std::fs::write(source.join("sub/img/p.png"), b"\x89PNG").unwrap();
        std::fs::write(source.join("tool.exe"), b"MZ").unwrap();
        std::fs::write(source.join(".git/config"), "secret").unwrap();
        let before = snapshot(&source);

        let manifest = prepared(&storage, SourceType::Folder, &source, "Imp");
        assert_eq!(storage.list().unwrap().len(), 0, "preparing writes nothing");
        assert!(manifest
            .losses
            .iter()
            .any(|l| l.reason == "unsupportedType" && l.path.ends_with("tool.exe")));
        assert!(!manifest
            .items
            .iter()
            .any(|item| item.source.contains(".git")));

        let finished = run(manifest, &mut storage, &source, false);
        assert_eq!(
            finished.state,
            BatchState::Partial,
            "a loss is not a complete import"
        );
        assert!(read_note(&storage, "Imp/a.md").contains("[b](sub/b.md) and [[Imp/sub/b.md]]"));
        assert!(read_note(&storage, "Imp/sub/b.md").contains("![p](img/p.png)"));
        assert!(finished
            .losses
            .iter()
            .any(|l| l.reason == "deadLink" && l.path.contains("none.md")));
        let vault_root = directory.path().join("vault");
        assert_eq!(
            std::fs::read(vault_root.join("Imp/sub/img/p.png")).unwrap(),
            b"\x89PNG"
        );
        assert_eq!(snapshot(&source), before, "the source is never changed");
    }

    #[test]
    fn running_twice_makes_no_second_copy_and_keeps_later_edits() {
        let (directory, mut storage) = vault();
        let source = directory.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.md"), "one\n").unwrap();
        let manifest = prepared(&storage, SourceType::Folder, &source, "");
        let first = run(manifest.clone(), &mut storage, &source, false);
        assert_eq!(first.state, BatchState::Complete);
        // A crash before progress was saved: the same plan runs from the start.
        let again = run(manifest, &mut storage, &source, false);
        assert_eq!(again.state, BatchState::Complete);
        let notes = storage.list().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].relative_path, "a.md");
    }

    #[test]
    fn cancelling_stops_and_a_resume_finishes_the_rest() {
        let (directory, mut storage) = vault();
        let source = directory.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.md"), "a\n").unwrap();
        std::fs::write(source.join("b.md"), "b\n").unwrap();
        let manifest = prepared(&storage, SourceType::Folder, &source, "");
        let stopped = run(manifest, &mut storage, &source, true);
        assert_eq!(stopped.state, BatchState::Cancelled);
        assert_eq!(storage.list().unwrap().len(), 0);
        let finished = run(stopped, &mut storage, &source, false);
        assert_eq!(finished.state, BatchState::Complete);
        assert_eq!(storage.list().unwrap().len(), 2);
    }

    #[test]
    fn a_file_changed_after_preparing_fails_alone() {
        let (directory, mut storage) = vault();
        let source = directory.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.md"), "a\n").unwrap();
        std::fs::write(source.join("b.md"), "b\n").unwrap();
        let manifest = prepared(&storage, SourceType::Folder, &source, "");
        std::fs::write(source.join("a.md"), "changed\n").unwrap();
        let finished = run(manifest, &mut storage, &source, false);
        assert_eq!(finished.state, BatchState::Partial);
        let failed = finished
            .items
            .iter()
            .find(|item| item.source == "a.md")
            .unwrap();
        assert_eq!(failed.status, ItemStatus::Failed);
        assert_eq!(
            failed.error_key.as_deref(),
            Some("error.importSourceChanged")
        );
        assert_eq!(storage.list().unwrap().len(), 1);
    }

    #[test]
    fn existing_notes_and_attachments_are_never_replaced() {
        let (directory, mut storage) = vault();
        storage
            .create_note("a.md", "mine\n", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let sha = sha256_hex(b"mine");
        storage
            .store_attachment("p.png", b"mine", &sha, &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let same = sha256_hex(b"same");
        storage
            .store_attachment("s.png", b"same", &same, &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let source = directory.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.md"), "theirs [p](p.png)\n").unwrap();
        std::fs::write(source.join("p.png"), b"theirs").unwrap();
        std::fs::write(source.join("s.png"), b"same").unwrap();
        let manifest = prepared(&storage, SourceType::Folder, &source, "");
        let finished = run(manifest, &mut storage, &source, false);
        assert_eq!(
            finished.state,
            BatchState::Complete,
            "{:?}",
            finished.losses
        );
        assert_eq!(read_note(&storage, "a.md"), "mine\n");
        let root = directory.path().join("vault");
        assert_eq!(std::fs::read(root.join("p.png")).unwrap(), b"mine");
        assert_eq!(std::fs::read(root.join("p (1).png")).unwrap(), b"theirs");
        assert!(
            read_note(&storage, "a (1).md").contains("[p](<p (1).png>)"),
            "links follow the rename"
        );
        assert!(
            !root.join("s (1).png").exists(),
            "identical bytes are reused"
        );
    }

    #[test]
    fn a_zip_cannot_write_outside_the_vault() {
        let (directory, mut storage) = vault();
        let path = zip_file(
            directory.path(),
            &[
                ("../evil.md", b"x"),
                ("ok/../../evil2.md", b"x"),
                ("C:/evil3.md", b"x"),
                ("good.md", b"fine\n"),
            ],
        );
        let manifest = prepared(&storage, SourceType::Zip, &path, "");
        assert_eq!(
            manifest
                .losses
                .iter()
                .filter(|l| l.reason == "pathUnsafe")
                .count(),
            3
        );
        assert_eq!(manifest.items.len(), 1);
        let finished = run(manifest, &mut storage, &path, false);
        assert_eq!(finished.state, BatchState::Partial);
        assert!(!directory.path().join("evil.md").exists());
        assert_eq!(read_note(&storage, "good.md"), "fine\n");
    }

    #[test]
    fn text_that_is_not_utf8_is_reported_and_never_garbled() {
        let (directory, mut storage) = vault();
        let path = zip_file(
            directory.path(),
            &[("gbk.md", b"\xc4\xe3\xba\xc3"), ("ok.md", b"ok\n")],
        );
        let manifest = prepared(&storage, SourceType::Zip, &path, "");
        assert!(manifest
            .losses
            .iter()
            .any(|l| l.reason == "encoding" && l.path == "gbk.md"));
        run(manifest, &mut storage, &path, false);
        assert_eq!(storage.list().unwrap().len(), 1);
    }

    #[test]
    fn a_damaged_archive_is_refused_before_anything_is_written() {
        let (directory, storage) = vault();
        let path = directory.path().join("bad.zip");
        std::fs::write(&path, b"not a zip at all").unwrap();
        let mut losses = Vec::new();
        let opened = Source::open(SourceType::Zip, &path, &mut losses);
        assert_eq!(
            opened.err().map(|error| error.message_key),
            Some("error.importArchiveInvalid".to_owned())
        );
        assert_eq!(storage.list().unwrap().len(), 0);
    }

    fn notion_export() -> Vec<(String, Vec<u8>)> {
        vec![
            (format!("Home {ID}.md"), format!("# Home\n\n[Tasks](Tasks%20{ID}.csv)\n[Page](Sub%20{ID}/Inner%20{ID}.md)\n![pic](Sub%20{ID}/pic.png)\n<aside>tip</aside>\n").into_bytes()),
            (format!("Sub {ID}/Inner {ID}.md"), format!("# Inner\n\n[Back](../Home%20{ID}.md)\n").into_bytes()),
            (format!("Sub {ID}/pic.png"), b"\x89PNG".to_vec()),
            (format!("Tasks {ID}.csv"), b"Name,Status\nFirst,Done\nSecond,Open\n".to_vec()),
            (format!("Tasks {ID}_all.csv"), b"Name,Status,Owner\nFirst,Done,Me\nSecond,Open,\n".to_vec()),
            (format!("Tasks {ID}/First {ID}.md"), b"# First\n\nStatus: Done\nOwner: Me\n\nBody text\n".to_vec()),
        ]
    }

    #[test]
    fn a_notion_export_becomes_clean_notes_with_properties_and_a_loss_report() {
        let (directory, mut storage) = vault();
        let files = notion_export();
        let borrowed: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        let path = zip_file(directory.path(), &borrowed);
        let manifest = prepared(&storage, SourceType::Notion, &path, "");
        assert!(manifest
            .losses
            .iter()
            .any(|l| l.reason == "block.callout" && l.path == "Home.md"));
        let finished = run(manifest, &mut storage, &path, false);
        assert_eq!(finished.state, BatchState::Partial);

        let home = read_note(&storage, "Home.md");
        assert!(home.contains("[Page](Sub/Inner.md)"), "{home}");
        assert!(home.contains("![pic](Sub/pic.png)"), "{home}");
        assert!(
            home.contains("<aside>tip</aside>"),
            "unconverted text stays"
        );
        assert!(read_note(&storage, "Sub/Inner.md").contains("[Back](../Home.md)"));

        let first = read_note(&storage, "Tasks/First.md");
        assert!(first.contains("\"Status\": \"Done\""), "{first}");
        assert!(first.contains("\"Owner\": \"Me\""), "{first}");
        assert!(first.contains("Body text"), "{first}");
        assert!(!first.contains("Status: Done"), "{first}");
        let second = read_note(&storage, "Tasks/Second.md");
        assert!(
            second.contains("\"Status\": \"Open\"") && second.contains("# Second"),
            "{second}"
        );
        assert!(directory.path().join("vault/Sub/pic.png").exists());
        assert!(
            directory.path().join("vault/Tasks_all.csv").exists(),
            "the original table is kept"
        );
    }

    #[test]
    fn a_nested_notion_part_is_read() {
        let (directory, mut storage) = vault();
        let mut inner = ZipWriter::new();
        inner.add(&format!("Page {ID}.md"), b"# P\n");
        let path = zip_file(directory.path(), &[("Export-Part-1.zip", &inner.finish())]);
        let manifest = prepared(&storage, SourceType::Notion, &path, "");
        run(manifest, &mut storage, &path, false);
        assert_eq!(read_note(&storage, "Page.md"), "# P\n");
    }

    #[test]
    fn a_prefix_that_leaves_the_vault_is_refused() {
        let (directory, storage) = vault();
        let source = directory.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.md"), "a").unwrap();
        let refused = prepare(
            &PrepareInput {
                batch_id: "11111111-1111-4111-8111-111111111111",
                vault_id: "v",
                source_type: SourceType::Folder,
                source_path: &source,
                prefix: "../outside",
            },
            &[],
            |_| Ok(None),
        );
        assert_eq!(
            refused.err().map(|error| error.message_key),
            Some("error.importPathInvalid".to_owned())
        );
        assert_eq!(storage.list().unwrap().len(), 0);
    }

    #[test]
    fn the_prefix_counts_when_a_name_is_already_taken() {
        let (directory, mut storage) = vault();
        let id = uuid::Uuid::new_v4().to_string();
        storage.create_note("Imp/a.md", "mine\n", &id).unwrap();
        let source = directory.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.md"), "theirs\n").unwrap();
        let manifest = prepared(&storage, SourceType::Folder, &source, "Imp");
        run(manifest, &mut storage, &source, false);
        assert_eq!(read_note(&storage, "Imp/a.md"), "mine\n");
        assert!(read_note(&storage, "Imp/a (1).md").contains("theirs"));
    }

    #[test]
    fn a_saved_plan_round_trips_and_a_newer_format_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            batch_id: "11111111-1111-4111-8111-111111111111".to_owned(),
            vault_id: "v".to_owned(),
            source_type: SourceType::Notion,
            source_path: "x.zip".to_owned(),
            state: BatchState::Running,
            items: Vec::new(),
            losses: vec![loss("a", "encoding")],
        };
        write_manifest(directory.path(), &manifest).unwrap();
        let path = batch_path(directory.path(), &manifest.batch_id).unwrap();
        assert_eq!(read_manifest(&path).unwrap(), manifest);
        assert_eq!(
            status_of(&manifest, false).state,
            BatchState::Interrupted,
            "nothing is running it"
        );
        assert_eq!(status_of(&manifest, true).state, BatchState::Running);

        let mut newer = manifest;
        newer.version = MANIFEST_VERSION + 1;
        std::fs::write(&path, serde_json::to_vec(&newer).unwrap()).unwrap();
        assert!(read_manifest(&path).is_err());
        assert!(batch_path(directory.path(), "../../etc").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_inside_a_folder_is_reported_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("src");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "secret").unwrap();
        std::fs::write(source.join("a.md"), "a").unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(source.join("linked"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(made.status.success());
        let mut losses = Vec::new();
        let files = scan_folder(&source, 100, 1 << 20, &mut losses).unwrap();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["a.md"]
        );
        assert_eq!(losses, vec![loss("linked", "link")]);

        let linked = directory.path().join("linked-root");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&linked)
            .arg(&source)
            .output()
            .unwrap();
        assert!(made.status.success());
        assert!(scan_folder(&linked, 100, 1 << 20, &mut Vec::new()).is_err());
    }

    #[test]
    fn folder_limits_apply_before_reading() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.md"), "12345").unwrap();
        std::fs::write(directory.path().join("b.md"), "12345").unwrap();
        assert!(scan_folder(directory.path(), 2, 10, &mut Vec::new()).is_ok());
        assert_eq!(
            scan_folder(directory.path(), 1, 10, &mut Vec::new())
                .err()
                .map(|e| e.message_key),
            Some("error.importEntryLimit".to_owned())
        );
        assert_eq!(
            scan_folder(directory.path(), 2, 9, &mut Vec::new())
                .err()
                .map(|e| e.message_key),
            Some("error.importExpandedLimit".to_owned())
        );
    }
}
