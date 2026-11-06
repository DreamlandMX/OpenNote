use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use onote_domain::{
    CheckpointRequest, CheckpointResult, DraftPage, ErrorCode, NoteDocument, NoteDraft,
    NoteSummary, SaveResult, ServiceError, Vault,
};

use crate::errors::{err, SeaRes};
use crate::state::NoteRecord;

mod attachments;
mod catalog;
pub use catalog::CatalogScan;
mod commit;
mod drafts;
mod errors;
mod folders;
pub use folders::FolderContents;
mod gate;
mod identity;
pub mod naming;
pub use attachments::{
    AttachmentFile, AttachmentProbe, AttachmentReader, AttachmentWrite, VaultDocument,
    MAX_ATTACHMENT_CHUNK, MAX_DOCUMENT_BYTES,
};
mod ops;
mod pathcheck;
mod project_files;
pub use project_files::{
    is_project_source, ProjectFileBytes, ProjectFileEntry, ProjectRecordBytes, MAX_PROJECT_FILES,
    MAX_PROJECT_FILE_BYTES, MAX_PROJECT_TOTAL_BYTES, PROJECT_SOURCE_EXTENSIONS,
};
mod rename;
mod scan;
mod search_cache;
pub use search_cache::SearchCacheSource;
mod snapshot;
mod state;
mod tag_batches;
mod trash;
mod upgrade;
pub use pathcheck::{compare_windows_paths, validate_new_note_path};
pub use trash::validate_trash_record;
pub use upgrade::SchemaUpgradeResult;

/// Injected by the platform composition layer; storage never imports the history implementation.
pub trait RevisionArchive: Send + Sync {
    fn preserve(&self, revision: &NoteDocument) -> Result<(), ServiceError>;
    fn verify(&self, note_id: &str, revision: u64, content_hash: &str) -> Result<(), ServiceError>;
    /// Must bypass edit coalescing before destructive or explicit restoration operations.
    fn force_preserve(&self, revision: &NoteDocument) -> Result<(), ServiceError>;
}

pub trait TrashArchive: Send + Sync {
    fn capture_deleted(&self, record: &onote_domain::TrashRecord) -> SeaRes<()>;
    fn read_deleted(&self, object_id: &str) -> SeaRes<onote_domain::TrashRecord>;
    fn purge_deleted(&self, entry: &onote_domain::TrashEntry) -> SeaRes<()>;
}

/// Corrupted authoritative mapping identity (e.g. non-canonical noteId in
/// the note table): integrity refusal for checkpoint writes.
fn draft_corrupt_refusal() -> ServiceError {
    errors::err(ErrorCode::IoDenied, "error.draftCorrupted", &[])
}

/// Crash-injection points for the save protocol (per-instance, deterministic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Failpoint {
    /// After the journal entry is written with status `prepared`.
    AfterPrepared,
    /// After staging, before the target revalidation and replacement:
    /// tests use it to mutate the target concurrently and deterministically.
    BeforeReplace,
    /// After the staged file was atomically replaced over the target.
    AfterReplace,
    /// After replace, before the SQLite commit transaction completes.
    BeforeDbCommit,
    /// Simulated underlying write failure; old file must stay intact.
    WriteFailure,
    TagBatchItemCommitted,
    RenameSourceRemoved,
    RenamePublished,
    RenameItemCommitted,
    UpgradePrepared,
    UpgradeDatabase,
    UpgradeMetadata,
}

/// Explicit recovery report: `applied` = replaced slots finalized by the
/// journal; `pending` = unreplaced/ambiguous slots kept as original + draft.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverOutcome {
    pub applied: Vec<String>,
    pub pending: Vec<String>,
}

pub struct VaultStorage {
    root: PathBuf,
    conn: Option<rusqlite::Connection>,
    vault_info: Vault,
    notes: RefCell<Vec<NoteRecord>>,
    fatal_findings: usize,
    failpoints: RefCell<Vec<Failpoint>>,
    revision_archive: Option<Arc<dyn RevisionArchive>>,
    trash_archive: Option<Arc<dyn TrashArchive>>,
}

impl std::fmt::Debug for VaultStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultStorage")
            .field("root", &self.root)
            .finish()
    }
}

impl VaultStorage {
    /// Create a brand-new vault at `root`.
    pub fn create(root: impl Into<PathBuf>, display_name: &str) -> SeaRes<VaultStorage> {
        let opened = state::initialize(root.into(), Some(display_name))?;
        Ok(VaultStorage {
            root: opened.root,
            conn: opened.conn,
            vault_info: opened.vault_info,
            notes: RefCell::new(opened.notes),
            fatal_findings: opened.fatal_findings,
            failpoints: opened.failpoints,
            revision_archive: None,
            trash_archive: None,
        })
    }

    /// Open an existing vault directory (original Markdown files are authoritative).
    pub fn open(root: impl Into<PathBuf>) -> SeaRes<VaultStorage> {
        let opened = state::initialize(root.into(), None)?;
        Ok(VaultStorage {
            root: opened.root,
            conn: opened.conn,
            vault_info: opened.vault_info,
            notes: RefCell::new(opened.notes),
            fatal_findings: opened.fatal_findings,
            failpoints: opened.failpoints,
            revision_archive: None,
            trash_archive: None,
        })
    }

    /// Enable durable revision protection for all subsequent commits and recovery.
    pub fn set_revision_archive(&mut self, archive: Arc<dyn RevisionArchive>) {
        self.revision_archive = Some(archive);
    }
    pub fn set_trash_archive(&mut self, archive: Arc<dyn TrashArchive>) {
        self.trash_archive = Some(archive);
    }

    /// Queue a failpoint for the next operations (deterministic test injection).
    pub fn push_failpoint(&self, failpoint: Failpoint) {
        self.failpoints.borrow_mut().push(failpoint);
    }

    /// Remove and return the next queued failpoint, if any.
    pub fn pop_failpoint(&self) -> Option<Failpoint> {
        self.failpoints.borrow_mut().pop()
    }

    fn take_failpoint(&self, want: Failpoint) -> bool {
        let mut queue = self.failpoints.borrow_mut();
        match queue.iter().position(|f| *f == want) {
            Some(idx) => {
                queue.remove(idx);
                true
            }
            None => false,
        }
    }

    fn require_writable(&self) -> SeaRes<&rusqlite::Connection> {
        if self.fatal_findings > 0 {
            return Err(err(
                ErrorCode::RescanRequired,
                "finding.duplicateNoteId",
                &[],
            ));
        }
        self.conn
            .as_ref()
            .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))
    }

    /// operationId is a caller-provided UUID in the exact canonical
    /// hyphenated-lowercase form this crate writes; anything else — any
    /// parseable-but-noncanonical spelling (`urn:uuid:…`, braced, simple,
    /// uppercase) included — is InvalidInput before any idempotency logic
    /// or filesystem write, because its colons/raw bytes must never be
    /// interpolated into aux filenames (Windows ADS paths).
    fn require_operation_id(operation_id: &str) -> SeaRes<()> {
        if !commit::is_canonical_uuid(operation_id) {
            return Err(err(
                ErrorCode::InvalidInput,
                "error.invalidOperationId",
                &[],
            ));
        }
        Ok(())
    }

    pub fn vault(&self) -> Vault {
        self.vault_info.clone()
    }

    /// Infrastructure adapters use this path; it is never part of the public vault DTO.
    pub fn root_path(&self) -> &std::path::Path {
        &self.root
    }

    /// Reads one note source for a derived, read-only snapshot.
    ///
    /// Search captures note summaries under the state lock, releases it, and
    /// then reads the sources on the calling thread. Applying the same relative
    /// path validation and reparse checks here keeps that snapshot inside the
    /// vault boundary. It deliberately does not take the writer gate: indexing
    /// must never make a save wait (F12-R03). A concurrent atomic save yields
    /// either the old or the new complete file; a delete yields an error that
    /// the index reports as an unreadable note instead of a false hit.
    pub fn read_source_snapshot(root: &Path, relative_path: &str) -> Result<String, ServiceError> {
        let bytes = state::guarded_read(root, relative_path)?;
        String::from_utf8(bytes).map_err(|_| {
            err(
                ErrorCode::IoDenied,
                "error.invalidEncoding",
                &[("relativePath", relative_path.to_owned())],
            )
        })
    }

    /// Read a captured note identity/hash without reconciling revisions or taking the writer gate.
    pub fn read_note_source_snapshot(
        root: &Path,
        note: &NoteSummary,
    ) -> Result<String, ServiceError> {
        if !commit::is_canonical_uuid(&note.note_id) || !commit::is_hash(&note.content_hash) {
            return Err(err(ErrorCode::InvalidInput, "error.invalidNoteId", &[]));
        }
        let source = Self::read_source_snapshot(root, &note.relative_path)?;
        if matches!(identity::parse_frontmatter(&source), identity::Frontmatter::WellFormed { id: identity::BlockId::Valid(ref id), .. } if id != &note.note_id)
        {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.noteIdentityChanged",
                &[],
            ));
        }
        if scan::hex_sha256(source.as_bytes()) != note.content_hash {
            return Err(err(ErrorCode::VersionConflict, "error.catalogChanged", &[]));
        }
        Ok(source)
    }

    /// M08 maintenance shares the note writer gate; callbacks must not acquire it again.
    pub fn with_history_write<T>(&self, write: impl FnOnce() -> SeaRes<T>) -> SeaRes<T> {
        let check = || {
            self.require_writable()?;
            let bytes = state::guarded_read(&self.root, ".opennote/vault.json")?;
            let metadata: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| err(ErrorCode::InvalidInput, "error.vaultMetaInvalid", &[]))?;
            if !self.vault_info.writable
                || self.vault_info.schema_version != state::SCHEMA_VERSION
                || metadata["schemaVersion"].as_u64() != Some(state::SCHEMA_VERSION as u64)
                || metadata["writable"].as_bool() != Some(true)
            {
                return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
            }
            if metadata["vaultId"].as_str() != Some(self.vault_info.vault_id.as_str()) {
                return Err(err(
                    ErrorCode::VersionConflict,
                    "error.vaultSessionChanged",
                    &[],
                ));
            }
            Ok(())
        };
        check()?;
        let gate = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate)?;
        check()?;
        rename::guard_pending(self.require_writable()?)?;
        write()
    }

    /// Notes whose M02 or trash journal is unfinished. History cleanup must
    /// preserve these notes because their restore source may still be needed.
    pub fn protected_note_ids(&self) -> Result<Vec<String>, ServiceError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))?;
        let mut ids: Vec<String> = ops::list_prepared(conn)?
            .into_iter()
            .map(|operation| operation.note_id)
            .collect();
        ids.extend(trash::protected(conn)?.1);
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// Creates an empty folder at a vault-relative path; returns its canonical form.
    pub fn create_folder(&self, relative_path: &str) -> Result<String, ServiceError> {
        self.folder_write(|root| folders::create(root, relative_path))
    }

    /// Every visible folder in the vault, including those without notes.
    pub fn list_folders(&self) -> Result<Vec<String>, ServiceError> {
        folders::list(&self.root)
    }

    /// What a folder holds besides notes.
    pub fn inspect_folder(&self, relative_path: &str) -> Result<FolderContents, ServiceError> {
        folders::inspect(&self.root, relative_path)
    }

    /// Removes a folder tree that holds no files.
    pub fn remove_empty_folder(&self, relative_path: &str) -> Result<(), ServiceError> {
        self.folder_write(|root| folders::remove_empty(root, relative_path))
    }

    /// Sends a folder that holds no notes away whole; `send` is the system
    /// recycle bin in the app. Notes inside must be trashed first.
    pub fn recycle_folder(
        &self,
        relative_path: &str,
        send: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<(), ServiceError> {
        self.folder_write(|root| folders::recycle(root, relative_path, send))
    }

    /// Renames or moves a folder tree that holds no files.
    pub fn rename_empty_folder(&self, from: &str, to: &str) -> Result<String, ServiceError> {
        self.folder_write(|root| folders::rename_empty(root, from, to))
    }

    fn folder_write<T>(&self, write: impl FnOnce(&Path) -> SeaRes<T>) -> Result<T, ServiceError> {
        self.require_writable()?;
        if !self.vault_info.writable {
            return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
        }
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        write(&self.root)
    }

    /// Scan the vault and list note summaries without modifying any file bytes.
    pub fn list(&self) -> Result<Vec<NoteSummary>, ServiceError> {
        if self.fatal_findings > 0 {
            return Err(err(
                ErrorCode::RescanRequired,
                "finding.duplicateNoteId",
                &[],
            ));
        }
        Ok(self
            .notes
            .borrow()
            .iter()
            .map(|n| state::summary(n, &self.vault_info.vault_id))
            .collect())
    }

    /// Read a note's full text. Missing files are not auto-created.
    /// Consistent read: the returned document always satisfies
    /// SHA256(source bytes) == contentHash. Supported-schema reads run under
    /// the writer gate; an external hash change refreshes the authoritative
    /// DB mapping exactly once (revision +1). While a journal operation is
    /// pending for the path (crash before explicit recover), the read stays
    /// READONLY on the authoritative baseline revision with the actual disk
    /// hash and no provisional DB bump; recovery owns the transition.
    /// Read-only records, non-writable vaults and newer-schema vaults never
    /// write to the DB and surface read-only documents.
    pub fn read(&self, note_id: &str) -> Result<NoteDocument, ServiceError> {
        if self.fatal_findings > 0 {
            return Err(err(
                ErrorCode::RescanRequired,
                "finding.duplicateNoteId",
                &[],
            ));
        }
        let cached = self
            .notes
            .borrow()
            .iter()
            .find(|n| n.note_id == note_id)
            .cloned()
            .ok_or_else(|| err(ErrorCode::ObjectNotFound, "error.noteNotFound", &[]))?;

        // Newer schema (or otherwise no DB): raw bytes only, zero DB writes,
        // the document is always read-only.
        let conn = match self.conn.as_ref() {
            Some(conn) if self.vault_info.writable => conn,
            _ => {
                let source = self.load_source(&cached.relative_path)?;
                if matches!(identity::parse_frontmatter(&source), identity::Frontmatter::WellFormed { id: identity::BlockId::Valid(ref id), .. } if id != note_id)
                {
                    return Err(err(
                        ErrorCode::VersionConflict,
                        "error.noteIdentityChanged",
                        &[],
                    ));
                }
                let fresh_hash = scan::hex_sha256(source.as_bytes());
                return Ok(NoteDocument {
                    note_id: cached.note_id.clone(),
                    vault_id: self.vault_info.vault_id.clone(),
                    relative_path: cached.relative_path.clone(),
                    source,
                    local_revision: cached.local_revision,
                    content_hash: fresh_hash,
                    read_only: true,
                });
            }
        };

        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;

        // Authoritative note record from SQLite under the gate, mirroring
        // save(): instance-local caches may be arbitrarily stale.
        let mut record = state::load_notes(conn)?
            .into_iter()
            .find(|n| n.note_id == note_id)
            .unwrap_or(cached);

        let source = self.load_source(&record.relative_path)?;
        let fresh_hash = scan::hex_sha256(source.as_bytes());
        if matches!(identity::parse_frontmatter(&source), identity::Frontmatter::WellFormed { id: identity::BlockId::Valid(ref id), .. } if id != note_id)
        {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.noteIdentityChanged",
                &[],
            ));
        }

        // While a journal operation is still pending for this path/note, the
        // disk may hold unrecovered replacement bytes: the authoritative DB
        // baseline (old revision/hash) must stay untouched until explicit
        // recover(), which owns the revision transition. Reads surface the
        // actual bytes with a matching contentHash but READONLY, and never
        // write a provisional revision bump that a later recovery could
        // contradict or regress.
        let pending_recovery = ops::list_prepared(conn)?
            .iter()
            .any(|p| p.note_relative == record.relative_path || p.note_id == record.note_id);
        let (trash_paths, trash_ids) = trash::protected(conn)?;
        let pending_rename = !rename::pending(conn)?.is_empty();
        if pending_recovery
            || pending_rename
            || trash_paths.contains(&record.relative_path)
            || trash_ids.contains(note_id)
        {
            return Ok(NoteDocument {
                note_id: record.note_id.clone(),
                vault_id: self.vault_info.vault_id.clone(),
                relative_path: record.relative_path.clone(),
                source,
                local_revision: record.local_revision,
                content_hash: fresh_hash,
                read_only: true,
            });
        }

        if record.content_hash != fresh_hash {
            // External hash change: refresh the authoritative mapping exactly
            // once per change, only where a mapping may be written at all.
            if !record.read_only && self.vault_info.writable {
                record.content_hash = fresh_hash.clone();
                record.local_revision += 1;
                state::persist_notes(conn, std::slice::from_ref(&record))?;
                self.upsert_note(record.clone());
            } else {
                record.content_hash = fresh_hash;
                record.read_only = true;
            }
        }

        Ok(NoteDocument {
            note_id: record.note_id.clone(),
            vault_id: self.vault_info.vault_id.clone(),
            relative_path: record.relative_path.clone(),
            source,
            local_revision: record.local_revision,
            content_hash: record.content_hash,
            read_only: record.read_only || !self.vault_info.writable,
        })
    }

    /// Read the raw note bytes under `root` as UTF-8 text.
    fn load_source(&self, relative_path: &str) -> Result<String, ServiceError> {
        Self::read_source_snapshot(&self.root, relative_path)
    }

    /// Create a new note at `relative_path` with full Markdown `source`.
    /// Allocates a stable UUID and embeds the `opennote-id` frontmatter.
    /// `operation_id` is the idempotency identity: the original-caller request
    /// (kind + path + full raw source) is digested before any disk/DB state
    /// checks, so identical retries replay the original result including the
    /// original note UUID — a new UUID is allocated only on the first request.
    /// F19: writes attachment bytes at a vault-relative path.
    ///
    /// Takes the same writer gate as note commits and refuses read-only vaults.
    /// The bytes are verified against `sha256` after staging and published
    /// without replacing any existing file; identical bytes already at the path
    /// are reported as `AlreadyPresent`, so a retry never writes a copy.
    pub fn store_attachment(
        &mut self,
        relative_path: &str,
        bytes: &[u8],
        sha256: &str,
        operation_id: &str,
    ) -> Result<(String, AttachmentWrite), ServiceError> {
        self.require_writable()?;
        if !self.vault_info.writable {
            return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
        }
        Self::require_operation_id(operation_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        attachments::store(&self.root, relative_path, bytes, sha256, operation_id)
    }

    /// F19: attachment files directly inside a vault folder, with hashes of
    /// files up to `limit` bytes. Read-only.
    pub fn list_attachments(
        &self,
        folder: &str,
        limit: u64,
    ) -> Result<Vec<AttachmentFile>, ServiceError> {
        attachments::list_folder(&self.root, folder, limit)
    }

    /// F19: reads attachments without holding the storage, so hashing a large
    /// file never makes the rest of the app wait for it.
    pub fn attachment_reader(&self) -> AttachmentReader {
        AttachmentReader::new(self.root.clone())
    }

    /// Structured vault files with `suffix` (e.g. `.canvas`). Read-only.
    pub fn list_documents(&self, suffix: &str) -> Result<Vec<AttachmentFile>, ServiceError> {
        attachments::list_documents(&self.root, suffix)
    }

    /// Reads one structured vault file; it must end with `suffix` and be UTF-8.
    pub fn read_document(
        &self,
        relative_path: &str,
        suffix: &str,
    ) -> Result<VaultDocument, ServiceError> {
        attachments::read_document(&self.root, relative_path, suffix)
    }

    /// Compare-and-swap save of a structured vault file. `expected_sha256` is
    /// the hash the editor loaded (None for a new file). A file changed or
    /// created elsewhere is a VersionConflict and is never overwritten; saving
    /// bytes already on disk succeeds without writing.
    pub fn save_document(
        &mut self,
        relative_path: &str,
        suffix: &str,
        source: &str,
        expected_sha256: Option<&str>,
        operation_id: &str,
    ) -> Result<(String, String, AttachmentWrite), ServiceError> {
        self.require_writable()?;
        if !self.vault_info.writable {
            return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
        }
        Self::require_operation_id(operation_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        attachments::save_document(
            &self.root,
            relative_path,
            suffix,
            source,
            expected_sha256,
            operation_id,
        )
    }

    /// F45: every file of a LaTeX project folder with size and hash. Bounded;
    /// a limit or a reparse point is an error, never a shortened listing.
    pub fn list_project_files(&self, folder: &str) -> Result<Vec<ProjectFileEntry>, ServiceError> {
        project_files::list_project(&self.root, folder)
    }

    /// F45: the bytes of one project file. Read-only.
    pub fn read_project_file(&self, relative_path: &str) -> Result<ProjectFileBytes, ServiceError> {
        project_files::read_project_file(&self.root, relative_path)
    }

    /// F45: compare-and-swap save of one `.tex/.bib/.sty/.cls/.bst` source.
    /// Same contract as `save_document`; the bytes must be valid UTF-8.
    pub fn save_project_source(
        &mut self,
        relative_path: &str,
        bytes: &[u8],
        expected_sha256: Option<&str>,
        operation_id: &str,
    ) -> Result<(String, String, AttachmentWrite), ServiceError> {
        self.require_writable()?;
        if !self.vault_info.writable {
            return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
        }
        Self::require_operation_id(operation_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        project_files::save_project_source(
            &self.root,
            relative_path,
            bytes,
            expected_sha256,
            operation_id,
        )
    }

    /// F45: the stored record of a LaTeX project, if it has one.
    pub fn read_project_record(
        &self,
        project_id: &str,
    ) -> Result<Option<ProjectRecordBytes>, ServiceError> {
        project_files::read_record(&self.root, project_id)
    }

    /// F45: every stored LaTeX project record, by project id. Read-only.
    pub fn list_project_records(&self) -> Result<Vec<(String, ProjectRecordBytes)>, ServiceError> {
        project_files::list_records(&self.root)
    }

    /// F45: removes a project registration (the record only, never the
    /// project's files) when it still has `expected_sha256`.
    pub fn remove_project_record(
        &mut self,
        project_id: &str,
        expected_sha256: &str,
        operation_id: &str,
    ) -> Result<(), ServiceError> {
        self.require_writable()?;
        if !self.vault_info.writable {
            return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
        }
        Self::require_operation_id(operation_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        project_files::remove_record(&self.root, project_id, expected_sha256)
    }

    /// F45: atomically replaces a project record that still has
    /// `expected_sha256` (None: none may exist). Returns the new hash.
    pub fn write_project_record(
        &mut self,
        project_id: &str,
        bytes: &[u8],
        expected_sha256: Option<&str>,
        operation_id: &str,
    ) -> Result<String, ServiceError> {
        self.require_writable()?;
        if !self.vault_info.writable {
            return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
        }
        Self::require_operation_id(operation_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        project_files::write_record(&self.root, project_id, bytes, expected_sha256, operation_id)
    }

    /// Destination recorded by an earlier `create_note` request with this
    /// operation id, if one exists.
    ///
    /// Read-only. A batch caller (folder import) uses it to pin a retried item to
    /// the path its first attempt chose, so a retry replays the original request
    /// instead of planning a fresh `name (1).md` around the note it already made.
    /// Non-create operations under the same id are reported as `None`; the later
    /// create call still rejects the id through the normal idempotency check.
    pub fn recorded_create_destination(
        &self,
        operation_id: &str,
    ) -> Result<Option<String>, ServiceError> {
        Self::require_operation_id(operation_id)?;
        let Some(conn) = self.conn.as_ref() else {
            return Ok(None);
        };
        Ok(ops::find_by_operation_id(conn, operation_id)?
            .filter(|record| record.kind == "create")
            .map(|record| record.note_relative))
    }

    pub fn create_note(
        &mut self,
        relative_path: &str,
        source: &str,
        operation_id: &str,
    ) -> Result<NoteDocument, ServiceError> {
        self.create_note_request(relative_path, source, operation_id, None)
    }

    pub fn create_copy(
        &mut self,
        source_note_id: &str,
        relative_path: &str,
        source: &str,
        operation_id: &str,
    ) -> Result<NoteDocument, ServiceError> {
        if !commit::is_canonical_uuid(source_note_id) {
            return Err(err(ErrorCode::InvalidInput, "error.invalidNoteId", &[]));
        }
        self.create_note_request(relative_path, source, operation_id, Some(source_note_id))
    }

    fn create_note_request(
        &mut self,
        relative_path: &str,
        source: &str,
        operation_id: &str,
        copy_from: Option<&str>,
    ) -> Result<NoteDocument, ServiceError> {
        let conn = self.require_writable()?;
        Self::require_operation_id(operation_id)?;
        let canonical = pathcheck::validate_new_note_path(relative_path)?;
        let context = copy_from
            .map(|id| format!("copy:{id}"))
            .unwrap_or_else(|| "create".into());
        let request_digest = ops::digest(&context, &canonical, source, None, None);
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;

        // idempotent replay / resume of the same operation request comes
        // before stale/existence checks
        if let Some(existing) = ops::find_by_operation_id(conn, operation_id)? {
            if existing.request_digest != request_digest {
                return Err(err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[("operationId", operation_id.to_owned())],
                ));
            }
            if existing.status != "committed" && !self.vault_info.writable {
                return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
            }
            let doc = self.replay_or_resume(conn, &existing, source, copy_from)?;
            return Ok(NoteDocument {
                note_id: existing.note_id.clone(),
                vault_id: self.vault_info.vault_id.clone(),
                relative_path: existing.note_relative.clone(),
                source: doc.0,
                local_revision: doc.1,
                content_hash: existing.new_hash.clone(),
                read_only: false,
            });
        }

        trash::refuse_operation_reuse(conn, operation_id)?;
        trash::guard_pending(conn, None, &canonical)?;
        if !self.vault_info.writable {
            return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
        }

        // new request: the target must not exist on disk (dangling links count)
        let target = self.root.join(&canonical);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(err(
                    ErrorCode::PathCollision,
                    "error.noteAlreadyExists",
                    &[("relativePath", canonical.clone())],
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(errors::io(&e, "error.createTargetDenied")),
        }
        // no case collision against already-known notes
        {
            let notes = self.notes.borrow();
            for other in notes.iter() {
                if pathcheck::same_windows_path(&other.relative_path, &canonical)
                    && other.relative_path != canonical
                {
                    return Err(err(
                        ErrorCode::PathCollision,
                        "error.caseCollision",
                        &[("relativePath", canonical.clone())],
                    ));
                }
            }
        }

        // first request only: allocate the note UUID now
        let note_id = uuid::Uuid::new_v4().to_string();
        let composed = match copy_from {
            Some(id) => identity::compose_for_copy(source, id, &note_id)?,
            None => identity::compose_for_create(source, &note_id)?,
        };
        let new_hash = scan::hex_sha256(composed.as_bytes());

        let plan = commit::CommitPlan {
            kind: "create".to_owned(),
            note_id: note_id.clone(),
            relative: canonical.clone(),
            composed: composed.clone(),
            new_hash: new_hash.clone(),
            old_hash: None,
            new_revision: 0,
            request_digest,
            resume_prepared: false,
            resume_stage: None,
            resume_old: None,
        };
        self.run_commit(conn, &plan, operation_id)?;
        Ok(NoteDocument {
            note_id,
            vault_id: self.vault_info.vault_id.clone(),
            relative_path: canonical,
            source: composed,
            local_revision: plan.new_revision,
            content_hash: new_hash,
            read_only: false,
        })
    }

    /// Atomic save with fresh-disk-hash + revision check and an idempotent
    /// `operation_id`, honoring any queued failpoint injection. The idempotent
    /// lookup compares the digest of the original caller request (kind, note
    /// path, full raw source, expectedRevision, noteId) before any stale or
    /// fresh-disk check, so identical retries replay the original result.
    /// The authoritative baseline (revision, hash, read-only) is the SQLite
    /// note row loaded AFTER the writer gate is acquired: instance-local
    /// caches may be arbitrarily stale, so a stale instance can never
    /// overwrite a newer revision committed meanwhile (even when disk bytes
    /// cycle back to bytes it still knows). No read-only record or read-only
    /// vault is ever saved.
    pub fn save(
        &mut self,
        note_id: &str,
        source: &str,
        expected_revision: u64,
        operation_id: &str,
    ) -> Result<SaveResult, ServiceError> {
        self.save_with_context(
            note_id,
            source,
            expected_revision,
            operation_id,
            "save",
            None,
        )
    }

    pub fn restore_revision(
        &mut self,
        note_id: &str,
        source: &str,
        expected_revision: u64,
        revision_id: &str,
        operation_id: &str,
    ) -> Result<SaveResult, ServiceError> {
        if revision_id.is_empty() || revision_id.len() > 256 {
            return Err(err(
                ErrorCode::InvalidInput,
                "error.invalidHistoryRevision",
                &[],
            ));
        }
        let context = format!(
            "restore-history:{}",
            scan::hex_sha256(revision_id.as_bytes())
        );
        self.save_with_context(
            note_id,
            source,
            expected_revision,
            operation_id,
            &context,
            None,
        )
    }

    fn save_with_context(
        &mut self,
        note_id: &str,
        source: &str,
        expected_revision: u64,
        operation_id: &str,
        request_context: &str,
        expected_baseline: Option<(&str, &str)>,
    ) -> Result<SaveResult, ServiceError> {
        let conn = self.require_writable()?;
        Self::require_operation_id(operation_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        trash::refuse_operation_reuse(conn, operation_id)?;
        if let Some(existing) = ops::find_by_operation_id(conn, operation_id)? {
            if existing.status == "committed" {
                if existing.request_digest
                    != ops::digest(
                        request_context,
                        &existing.note_relative,
                        source,
                        Some(expected_revision),
                        Some(note_id),
                    )
                {
                    return Err(err(
                        ErrorCode::IdempotencyMismatch,
                        "error.idempotencyMismatch",
                        &[],
                    ));
                }
                return Ok(SaveResult {
                    note_id: existing.note_id,
                    local_revision: existing.new_revision.unwrap_or(expected_revision),
                    content_hash: existing.new_hash,
                });
            }
        }

        if expected_baseline.is_some() {
            let metadata: serde_json::Value =
                serde_json::from_slice(&state::guarded_read(&self.root, ".opennote/vault.json")?)
                    .map_err(|_| err(ErrorCode::InvalidInput, "error.vaultMetaInvalid", &[]))?;
            if metadata["vaultId"].as_str() != Some(self.vault_info.vault_id.as_str())
                || metadata["schemaVersion"].as_u64() != Some(self.vault_info.schema_version as u64)
                || metadata["writable"].as_bool() != Some(true)
            {
                return Err(err(
                    ErrorCode::VersionConflict,
                    "error.vaultSessionChanged",
                    &[],
                ));
            }
        }

        // Authoritative note record from SQLite under the gate. Instance-local
        // caches may be arbitrarily stale vs another instance that committed
        // (or externally cycled) meanwhile.
        let record = state::load_notes(conn)?
            .into_iter()
            .find(|n| n.note_id == note_id)
            .ok_or_else(|| err(ErrorCode::ObjectNotFound, "error.noteNotFound", &[]))?;

        if expected_baseline
            .is_some_and(|(path, hash)| record.relative_path != path || record.content_hash != hash)
        {
            return Err(err(ErrorCode::VersionConflict, "error.externalChange", &[]));
        }

        // The ORIGINAL request digest: kind, path, full raw source,
        // expectedRevision and noteId — compared against the stored row
        // BEFORE any fresh disk read so a committed replay returns the
        // original result even when the note file has since been removed.
        let request_digest = ops::digest(
            request_context,
            &record.relative_path,
            source,
            Some(expected_revision),
            Some(note_id),
        );

        // idempotent replay / resume of the same operation request comes
        // before any stale or fresh-disk check
        if let Some(existing) = ops::find_by_operation_id(conn, operation_id)? {
            if existing.request_digest != request_digest {
                return Err(err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[("operationId", operation_id.to_owned())],
                ));
            }
            if existing.status == "committed" {
                // committed replay: pure original result, no disk reads
                return Ok(SaveResult {
                    note_id: existing.note_id.clone(),
                    local_revision: existing.new_revision.unwrap_or(expected_revision),
                    content_hash: existing.new_hash.clone(),
                });
            }
            // prepared retry resumes under its original request; the fresh
            // target state is classified inside the commit protocol (reuse
            // of validated stage/backup, guarded finalize or refusal that
            // preserves everything)
            if !self.vault_info.writable {
                return Err(err(ErrorCode::InvalidInput, "error.vaultReadOnly", &[]));
            }
            let plan = self.plan_from_record(&existing, source, None)?;
            let new_revision = plan.new_revision;
            let final_hash = plan.new_hash.clone();
            self.run_commit(conn, &plan, operation_id)?;
            return Ok(SaveResult {
                note_id: note_id.to_owned(),
                local_revision: new_revision,
                content_hash: final_hash,
            });
        }

        trash::guard_pending(conn, Some(note_id), &record.relative_path)?;
        // fresh disk hash: any external change (even with unchanged mtime) conflicts
        let fresh = state::guarded_read(&self.root, &record.relative_path)?;
        let fresh_hash = scan::hex_sha256(&fresh);

        // A read-only record (or a non-writable vault) is never saved, even by
        // supplying a fresh valid source.
        if record.read_only || !self.vault_info.writable {
            return Err(err(
                ErrorCode::InvalidInput,
                "error.noteReadOnly",
                &[("relativePath", record.relative_path.clone())],
            ));
        }

        // still-authoritative revision check comes after idempotency lookup,
        // plus a fresh-disk check so unchanged mtime cannot hide external bytes
        if fresh_hash != record.content_hash {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.externalChange",
                &[("relativePath", record.relative_path.clone())],
            ));
        }
        if expected_revision != record.local_revision {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.expectedRevisionMismatch",
                &[("relativePath", record.relative_path.clone())],
            ));
        }

        // The on-disk bytes are authoritative for the composed artifact's
        // BOM/EOL format (the raw caller digest above is untouched).
        let fresh_reference = String::from_utf8(fresh).map_err(|_| {
            err(
                ErrorCode::IoDenied,
                "error.invalidEncoding",
                &[("relativePath", record.relative_path.clone())],
            )
        })?;
        let composed = identity::compose_for_save(source, note_id, &fresh_reference)?;
        let new_hash = scan::hex_sha256(composed.as_bytes());

        let plan = commit::CommitPlan {
            kind: "save".to_owned(),
            note_id: note_id.to_owned(),
            relative: record.relative_path.clone(),
            composed: composed.clone(),
            new_hash: new_hash.clone(),
            old_hash: Some(fresh_hash.clone()),
            new_revision: expected_revision + 1,
            request_digest,
            resume_prepared: false,
            resume_stage: None,
            resume_old: None,
        };
        self.run_commit(conn, &plan, operation_id)?;
        Ok(SaveResult {
            note_id: note_id.to_owned(),
            local_revision: plan.new_revision,
            content_hash: new_hash,
        })
    }

    /// Reconstruct the exact original composed bytes for a prepared save
    /// resume. Format-reference candidates come only from the three
    /// structurally validated journal paths — the target when its bytes hash
    /// to the row's own old_hash (baseline) or new_hash (this op's staged
    /// result), the journal old snapshot for old_hash, the staged file for
    /// new_hash. Mismatched or missing bytes are never trusted; the final
    /// composition must reproduce the row's new_hash. With no validated
    /// candidate the pending journal, target and every aux file stay
    /// untouched behind a typed refusal. The caller raw request digest was
    /// already matched, so `source` is the original raw text.
    fn recompose_prepared_save(
        &self,
        existing: &ops::OperationRecord,
        source: &str,
    ) -> SeaRes<String> {
        let old_hash = match existing.old_hash.as_deref() {
            Some(hash) => hash,
            None => {
                return Err(err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[],
                ))
            }
        };

        let mut target = match state::guarded_read(&self.root, &existing.note_relative) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.code == ErrorCode::ObjectNotFound => None,
            Err(e) => return Err(e),
        };
        let target_hash = target.as_deref().map(scan::hex_sha256);
        let target_is_old = target_hash.as_deref() == Some(old_hash);
        let target_is_new = target_hash.as_deref() == Some(existing.new_hash.as_str());

        let mut attempts: Vec<String> = Vec::new();

        // Baseline-format reference: exact own old_hash bytes (unreplaced
        // target, else the journal's old snapshot).
        let baseline: Option<(String, Vec<u8>)> = if target_is_old {
            target
                .take()
                .map(|bytes| (existing.note_relative.clone(), bytes))
        } else {
            match existing.old_relative.as_deref() {
                Some(relative) => self
                    .validated_reference(relative, old_hash)?
                    .map(|bytes| (relative.to_owned(), bytes)),
                None => None,
            }
        };
        if let Some((relative, bytes)) = baseline {
            attempts.push(checked_text(&relative, bytes)?);
        }

        // Same-operation artifact reference: exact own new_hash bytes
        // (already-replaced target, else the journal's staged bytes). For an
        // identical-bytes save the baseline branch consumed those bytes.
        let artifact: Option<(String, Vec<u8>)> = if target_is_new {
            target
                .take()
                .map(|bytes| (existing.note_relative.clone(), bytes))
        } else {
            match self.validated_reference(&existing.stage_relative, &existing.new_hash)? {
                Some(bytes) => Some((existing.note_relative.clone(), bytes)),
                None => None,
            }
        };
        if let Some((relative, bytes)) = artifact {
            attempts.push(checked_text(&relative, bytes)?);
        }

        if attempts.is_empty() {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.externalChange",
                &[("relativePath", existing.note_relative.clone())],
            ));
        }
        for candidate in &attempts {
            match identity::compose_for_save(source, &existing.note_id, candidate) {
                Ok(composed) if scan::hex_sha256(composed.as_bytes()) == existing.new_hash => {
                    return Ok(composed);
                }
                _ => {}
            }
        }
        Err(err(
            ErrorCode::IdempotencyMismatch,
            "error.idempotencyMismatch",
            &[],
        ))
    }

    /// A journal-owned aux path is a reference candidate only when its exact
    /// bytes hash to the row's own expected hash; missing or foreign bytes
    /// are never trusted for format inference and are never modified.
    fn validated_reference(&self, relative: &str, expect_hash: &str) -> SeaRes<Option<Vec<u8>>> {
        match state::guarded_read(&self.root, relative) {
            Ok(bytes) if scan::hex_sha256(&bytes) == expect_hash => Ok(Some(bytes)),
            Ok(_) => Ok(None),
            Err(e) if e.code == ErrorCode::ObjectNotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Persist an immutable draft checkpoint as an append-only row in the
    /// existing state.sqlite (`draft_checkpoints` table). Markdown bytes,
    /// note revisions/`notes` rows and the `operations` journal are NEVER
    /// touched, and checkpoint success never means "note saved".
    ///
    /// Contract: canonical UUID operationId; the caller path must match the
    /// authoritative note mapping (from SQLite under the writer gate — the
    /// frontend path is never trusted); baseline hash hex-64, revision within
    /// SQLite i64 bounds; raw source stored verbatim (BOM/CRLF/Unicode/
    /// incomplete YAML), hashed over the EXACT bytes with no normalization.
    /// Replays of the same operationId only succeed with the byte-identical
    /// original request (permanent digest: same ID same payload, otherwise
    /// typed mismatch) AND a stored body that still hashes to its stored
    /// `source_hash` — an externally corrupted source column refuses typed,
    /// the corrupted row is never rewritten. List pages remain actual
    /// metadata-only SELECTs. Transactions commit fully before the result is
    /// returned — a failed insert leaves zero rows (rollback).
    pub fn write_checkpoint(
        &self,
        request: CheckpointRequest,
    ) -> Result<CheckpointResult, ServiceError> {
        let conn = self.require_writable()?;
        if !self.vault_info.writable {
            return Err(err(
                ErrorCode::InvalidInput,
                "error.noteReadOnly",
                &[("relativePath", request.relative_path.clone())],
            ));
        }
        Self::require_operation_id(&request.operation_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;

        trash::refuse_operation_reuse(conn, &request.operation_id)?;
        // Authoritative note mapping from SQLite under the gate; the mapped
        // identity itself must be a canonical UUID (mapping is untrusted).
        let record = state::load_notes(conn)?
            .into_iter()
            .find(|n| n.note_id == request.note_id)
            .ok_or_else(|| err(ErrorCode::ObjectNotFound, "error.noteNotFound", &[]))?;
        if !commit::is_canonical_uuid(&record.note_id) {
            return Err(draft_corrupt_refusal());
        }

        // Path shape plus EXACT ownership: no silent canonicalization — the
        // caller spelling must already be the valid canonical form and must
        // be the one note the mapping owns.
        let canonical = pathcheck::validate_new_note_path(&request.relative_path)?;
        if canonical != request.relative_path || canonical != record.relative_path {
            return Err(err(
                ErrorCode::InvalidInput,
                "error.checkpointPathMismatch",
                &[("relativePath", canonical)],
            ));
        }

        // Baseline sanity: hex-64 hash and IPC-safe numeric bounds.
        if !drafts::is_sha256_hex(&request.base_content_hash) {
            return Err(err(
                ErrorCode::InvalidInput,
                "error.checkpointHashMismatch",
                &[],
            ));
        }
        if request.base_revision > drafts::MAX_SAFE_INTEGER {
            return Err(err(ErrorCode::InvalidInput, "error.numericRange", &[]));
        }

        // Permanent raw request digest before any further checks; binds the
        // whole original request (id, note, path, source bytes, baseline).
        let source_hash = scan::hex_sha256(request.source.as_bytes());
        let digest = drafts::request_digest(
            &request.operation_id,
            &request.note_id,
            &record.relative_path,
            &source_hash,
            request.base_revision,
            &request.base_content_hash,
        );

        // Idempotent replay: same ID + same payload returns the original
        // result (integrity-verified first, STORED BODY included), same ID +
        // other payload typed refusal.
        if let Some(existing) = drafts::find_by_draft_id(conn, &request.operation_id)? {
            if existing.request_digest != digest {
                return Err(err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[("operationId", request.operation_id)],
                ));
            }
            // The metadata digest alone cannot detect an externally corrupted
            // source column: on replay the stored body is read and verified
            // against source_hash too (lists stay metadata-only SELECTs).
            let stored_source = drafts::fetch_source(conn, &request.operation_id)?;
            drafts::validate_with_source(&existing, &stored_source)?;
            return Ok(drafts::result(&existing));
        }

        let row = drafts::DraftRow {
            draft_id: request.operation_id.clone(),
            note_id: record.note_id.clone(),
            relative_path: record.relative_path.clone(),
            source_hash,
            base_revision: request.base_revision,
            base_content_hash: request.base_content_hash,
            request_digest: digest,
            saved_at: drafts::now_rfc3339(),
            sequence: None,
        };
        let sequence = drafts::insert(conn, &row, &request.source, &mut |want| {
            self.take_failpoint(want)
        })?;
        let mut stored = row;
        stored.sequence = Some(sequence);
        Ok(drafts::result(&stored))
    }

    /// Sequence-DESC listing of draft checkpoints, metadata only (no draft
    /// bodies), with a stable `before` cursor and default 50 / hard cap 200.
    /// Rows are integrity-validated before exposure; corrupted rows refuse
    /// typed instead of fabricating results.
    pub fn list_drafts(
        &self,
        note_id: Option<&str>,
        before_sequence: Option<u64>,
        limit: Option<usize>,
    ) -> Result<DraftPage, ServiceError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))?;
        if self.fatal_findings > 0 {
            return Err(err(
                ErrorCode::RescanRequired,
                "finding.duplicateNoteId",
                &[],
            ));
        }
        let limit = drafts::normalize_limit(limit)?;
        if let Some(before) = before_sequence {
            if before > drafts::MAX_SAFE_INTEGER {
                return Err(err(ErrorCode::InvalidInput, "error.numericRange", &[]));
            }
        }
        drafts::list_page(conn, note_id, before_sequence, limit)
    }

    /// Load one stored draft in full, only after verifying stored
    /// metadata integrity first, then loading the raw source bytes and
    /// checking their exact SHA-256 against `source_hash`; any mismatch is a
    /// typed refusal — never a fabricated body.
    pub fn read_draft(&self, draft_id: &str) -> Result<NoteDraft, ServiceError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))?;
        Self::require_operation_id(draft_id)?;
        let row = drafts::find_by_draft_id(conn, draft_id)?
            .ok_or_else(|| err(ErrorCode::ObjectNotFound, "error.draftNotFound", &[]))?;
        drafts::validate_meta(&row)?;
        let source = drafts::fetch_source(conn, draft_id)?;
        drafts::validate_with_source(&row, &source)?;
        Ok(drafts::draft(row, source))
    }

    /// Replay the journal after a crash to a consistent state. The shell calls
    /// this explicitly before presenting a vault; open never auto-drains.
    pub fn recover(&self) -> Result<crate::RecoverOutcome, ServiceError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        if !self.vault_info.writable {
            let mut pending: Vec<String> = ops::list_prepared(conn)?
                .into_iter()
                .map(|op| op.note_id)
                .collect();
            pending.extend(trash::protected(conn)?.1);
            pending.extend(rename::pending(conn)?);
            pending.sort();
            pending.dedup();
            return Ok(RecoverOutcome {
                applied: Vec::new(),
                pending,
            });
        }
        let core = commit::Core {
            root: &self.root,
            conn,
            vault_id: &self.vault_info.vault_id,
            archive: self.revision_archive.as_deref(),
        };
        let mut outcome = commit::perform_recovery(&core)?;
        let trash_outcome = self.recover_trash()?;
        outcome.applied.extend(trash_outcome.applied);
        outcome.pending.extend(trash_outcome.pending);
        let rename_outcome = self.recover_renames(conn)?;
        outcome.applied.extend(rename_outcome.applied);
        outcome.pending.extend(rename_outcome.pending);
        let notes = state::load_notes(conn)?;
        *self.notes.borrow_mut() = notes;
        Ok(outcome)
    }

    fn plan_from_record(
        &self,
        existing: &ops::OperationRecord,
        source: &str,
        copy_from: Option<&str>,
    ) -> SeaRes<commit::CommitPlan> {
        // Journal rows are untrusted: structural/path ownership validation
        // happens before any disk or DB action is derived from them.
        if !commit::validate_record(existing) {
            return Err(err(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
                &[],
            ));
        }
        let composed = if existing.kind == "create" {
            match copy_from {
                Some(id) => identity::compose_for_copy(source, id, &existing.note_id)?,
                None => identity::compose_for_create(source, &existing.note_id)?,
            }
        } else {
            self.recompose_prepared_save(existing, source)?
        };
        let new_hash = scan::hex_sha256(composed.as_bytes());
        if existing.new_hash != new_hash {
            return Err(err(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
                &[],
            ));
        }
        Ok(commit::CommitPlan {
            kind: existing.kind.clone(),
            note_id: existing.note_id.clone(),
            relative: existing.note_relative.clone(),
            composed,
            new_hash,
            old_hash: existing.old_hash.clone(),
            new_revision: existing.new_revision.unwrap_or(0),
            request_digest: existing.request_digest.clone(),
            resume_prepared: true,
            resume_stage: Some(existing.stage_relative.clone()),
            resume_old: existing.old_relative.clone(),
        })
    }

    /// For a committed row: simply replay the original computed artifacts.
    /// For a prepared row: resume the durable protocol from the journal state.
    fn replay_or_resume(
        &self,
        conn: &rusqlite::Connection,
        existing: &ops::OperationRecord,
        source: &str,
        copy_from: Option<&str>,
    ) -> SeaRes<(String, u64)> {
        let plan = self.plan_from_record(existing, source, copy_from)?;
        if existing.status == "committed" {
            return Ok((plan.composed, plan.new_revision));
        }
        self.run_commit(conn, &plan, &existing.operation_id)?;
        Ok((plan.composed, plan.new_revision))
    }

    fn run_commit(
        &self,
        conn: &rusqlite::Connection,
        plan: &commit::CommitPlan,
        operation_id: &str,
    ) -> SeaRes<()> {
        let core = commit::Core {
            root: &self.root,
            conn,
            vault_id: &self.vault_info.vault_id,
            archive: self.revision_archive.as_deref(),
        };
        commit::execute_commit(&core, plan, operation_id, &mut |want| {
            self.take_failpoint(want)
        })?;
        self.upsert_note(NoteRecord {
            note_id: plan.note_id.clone(),
            relative_path: plan.relative.clone(),
            content_hash: plan.new_hash.clone(),
            local_revision: plan.new_revision,
            read_only: false,
            id_embedded: true,
        });
        Ok(())
    }

    fn upsert_note(&self, record: NoteRecord) {
        let mut notes = self.notes.borrow_mut();
        if let Some(slot) = notes
            .iter_mut()
            .find(|n| n.relative_path == record.relative_path)
        {
            *slot = record;
        } else {
            notes.push(record);
        }
    }
}

/// A validated candidate is text only when its bytes decode as UTF-8; a
/// mislabeled candidate means the authoritative bytes are gone — typed
/// refusal, no guessing, no artifact writes.
fn checked_text(relative: &str, bytes: Vec<u8>) -> SeaRes<String> {
    String::from_utf8(bytes).map_err(|_| {
        err(
            ErrorCode::VersionConflict,
            "error.externalChange",
            &[("relativePath", relative.to_owned())],
        )
    })
}
