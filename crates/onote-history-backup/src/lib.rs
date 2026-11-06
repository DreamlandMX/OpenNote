use chrono::Utc;
use onote_domain::{
    ErrorCode, HistoryListing, HistoryRevision, NoteDocument, SaveResult, ServiceError,
};
use onote_vault_storage::{RevisionArchive, VaultStorage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

mod history_retention;
mod reference_check;
mod safe_io;
mod snapshot;
mod trash;
pub use history_retention::{
    HistoryCleanupPreview, HistoryCleanupProgress, HistoryCleanupResult, HistoryPreservationReason,
    HistoryPreservedRevision, HistoryRetentionCandidate, HistoryRetentionPlan,
    HistoryRetentionSettings, PendingHistoryCleanup, DEFAULT_HISTORY_RETENTION_DAYS,
};
pub use snapshot::{
    BackupStore, DailyBackupResult, PendingSnapshotCleanup, PendingSnapshotUpgrade,
    PreservedSnapshot, SnapshotCleanupResult, SnapshotOverwritePlan, SnapshotOverwriteResult,
    SnapshotPreservationReason, SnapshotReferenceIssue, SnapshotReferenceIssueReason,
    SnapshotReferenceReport, SnapshotRestoreCancellation, SnapshotRestorePlan,
    SnapshotRestoreResult, SnapshotRetentionPlan, SnapshotUpgradeAbandonment, SnapshotUpgradePlan,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRevisionContent {
    pub document: NoteDocument,
    pub reference_report: SnapshotReferenceReport,
}

fn error(code: ErrorCode, key: &str) -> ServiceError {
    ServiceError {
        code,
        message_key: key.into(),
        params: Default::default(),
        retryable: false,
        operation_id: None,
        retry_after_seconds: None,
    }
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn canonical_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
fn history_id(note: &NoteDocument) -> String {
    format!("r{}-{}", note.local_revision, note.content_hash)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    revision: HistoryRevision,
    note: NoteDocument,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    format_version: u32,
    record: Record,
    checksum: String,
}

#[derive(Debug)]
pub struct HistoryStore {
    root: PathBuf,
    vault_id: String,
    recent: Mutex<BTreeMap<String, HistoryRevision>>,
}

impl HistoryStore {
    pub fn open(root: impl AsRef<Path>, vault_id: &str) -> Result<Self, ServiceError> {
        if !canonical_uuid(vault_id) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidVaultId"));
        }
        let root = fs::canonicalize(root).map_err(safe_io::io)?;
        safe_io::check(&root, &root.join(".opennote"))?;
        Ok(Self {
            root,
            vault_id: vault_id.into(),
            recent: Mutex::new(BTreeMap::new()),
        })
    }

    fn remember(&self, revision: &HistoryRevision) -> Result<(), ServiceError> {
        self.recent
            .lock()
            .map_err(|_| error(ErrorCode::IoDenied, "error.historyLock"))?
            .insert(revision.note_id.clone(), revision.clone());
        Ok(())
    }

    fn directory(&self, note_id: &str) -> Result<PathBuf, ServiceError> {
        if !canonical_uuid(note_id) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidNoteId"));
        }
        Ok(self.root.join(".opennote/history").join(note_id))
    }

    fn path(&self, note_id: &str, revision_id: &str) -> Result<PathBuf, ServiceError> {
        let Some((revision, content_hash)) = revision_id
            .strip_prefix('r')
            .and_then(|s| s.split_once('-'))
        else {
            return Err(error(ErrorCode::InvalidInput, "error.invalidRevisionId"));
        };
        let number = revision
            .parse::<u64>()
            .map_err(|_| error(ErrorCode::InvalidInput, "error.invalidRevisionId"))?;
        if number.to_string() != revision
            || content_hash.len() != 64
            || !content_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error(ErrorCode::InvalidInput, "error.invalidRevisionId"));
        }
        Ok(self.directory(note_id)?.join(format!("{revision_id}.json")))
    }

    fn validate_note(&self, note: &NoteDocument) -> Result<(), ServiceError> {
        if note.vault_id != self.vault_id
            || !canonical_uuid(&note.note_id)
            || note.local_revision > 9_007_199_254_740_991
            || onote_vault_storage::validate_new_note_path(&note.relative_path)?
                != note.relative_path
            || hash(note.source.as_bytes()) != note.content_hash
        {
            return Err(error(ErrorCode::HistoryMissing, "error.historyCorrupt"));
        }
        Ok(())
    }

    fn record(&self, note_id: &str, revision_id: &str) -> Result<Record, ServiceError> {
        let bytes = safe_io::read(&self.root, &self.path(note_id, revision_id)?)?;
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| error(ErrorCode::HistoryMissing, "error.historyCorrupt"))?;
        if envelope.format_version != 1 {
            return Err(error(ErrorCode::UnsupportedSchema, "error.historySchema"));
        }
        let record = envelope.record;
        self.validate_note(&record.note)?;
        let payload = serde_json::to_vec(&record)
            .map_err(|_| error(ErrorCode::HistoryMissing, "error.historyCorrupt"))?;
        if record.note.note_id != note_id
            || record.revision.revision_id != revision_id
            || history_id(&record.note) != revision_id
            || record.revision.note_id != note_id
            || record.revision.content_hash != record.note.content_hash
            || record.revision.local_revision != record.note.local_revision
            || record.revision.relative_path != record.note.relative_path
            || chrono::DateTime::parse_from_rfc3339(&record.revision.saved_at).is_err()
            || hash(&payload) != envelope.checksum
        {
            return Err(error(ErrorCode::HistoryMissing, "error.historyCorrupt"));
        }
        Ok(record)
    }

    pub fn capture(&self, note: &NoteDocument) -> Result<HistoryRevision, ServiceError> {
        self.validate_note(note)?;
        let revision_id = history_id(note);
        let path = self.path(&note.note_id, &revision_id)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let existing = self.record(&note.note_id, &revision_id)?;
                if existing.note != *note {
                    return Err(error(ErrorCode::HistoryMissing, "error.historyCollision"));
                }
                self.remember(&existing.revision)?;
                return Ok(existing.revision);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(safe_io::io(e)),
        }
        let revision = HistoryRevision {
            revision_id,
            note_id: note.note_id.clone(),
            relative_path: note.relative_path.clone(),
            local_revision: note.local_revision,
            content_hash: note.content_hash.clone(),
            saved_at: Utc::now().to_rfc3339(),
        };
        let record = Record {
            revision: revision.clone(),
            note: note.clone(),
        };
        let checksum = hash(
            &serde_json::to_vec(&record)
                .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?,
        );
        let bytes = serde_json::to_vec(&Envelope {
            format_version: 1,
            record,
            checksum,
        })
        .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?;
        if let Err(publication) = safe_io::publish(&self.root, &path, &bytes) {
            if let Ok(existing) = self.record(&note.note_id, &revision.revision_id) {
                if existing.note == *note {
                    self.remember(&existing.revision)?;
                    return Ok(existing.revision);
                }
            }
            return Err(publication);
        }
        self.remember(&revision)?;
        Ok(revision)
    }

    pub fn list(&self, note_id: &str) -> Result<Vec<HistoryRevision>, ServiceError> {
        let listing = self.list_available(note_id)?;
        if !listing.unavailable_revision_ids.is_empty() {
            return Err(error(ErrorCode::HistoryMissing, "error.historyCorrupt"));
        }
        Ok(listing.items)
    }

    pub fn list_available(&self, note_id: &str) -> Result<HistoryListing, ServiceError> {
        let directory = self.directory(note_id)?;
        match safe_io::check(&self.root, &directory) {
            Err(e) if e.code == ErrorCode::HistoryMissing => {
                return Ok(HistoryListing {
                    items: Vec::new(),
                    unavailable_revision_ids: Vec::new(),
                })
            }
            Err(e) => return Err(e),
            Ok(()) => (),
        }
        let mut revisions = Vec::new();
        let mut unavailable_revision_ids = Vec::new();
        for entry in fs::read_dir(directory).map_err(safe_io::io)? {
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| error(ErrorCode::HistoryMissing, "error.historyInvalidFile"))?;
            if name.starts_with(".pending-") {
                continue;
            }
            let id = name.strip_suffix(".json").unwrap_or(name);
            match self.record(note_id, id) {
                Ok(record) => revisions.push(record.revision),
                Err(e) if e.code == ErrorCode::UnsupportedSchema => return Err(e),
                Err(_) => unavailable_revision_ids.push(id.to_owned()),
            }
        }
        revisions.sort_by(|a, b| {
            b.local_revision
                .cmp(&a.local_revision)
                .then_with(|| a.revision_id.cmp(&b.revision_id))
        });
        unavailable_revision_ids.sort();
        Ok(HistoryListing {
            items: revisions,
            unavailable_revision_ids,
        })
    }

    pub fn read(&self, note_id: &str, revision_id: &str) -> Result<NoteDocument, ServiceError> {
        Ok(self.record(note_id, revision_id)?.note)
    }

    pub fn read_with_references(
        &self,
        note_id: &str,
        revision_id: &str,
    ) -> Result<HistoryRevisionContent, ServiceError> {
        let document = self.record(note_id, revision_id)?.note;
        let mut issues = Vec::new();
        for reference in reference_check::markdown_targets(&document.source) {
            let reason = match reference_check::local_path(&document.relative_path, &reference) {
                None => continue,
                Some(Err(())) => SnapshotReferenceIssueReason::Unsafe,
                Some(Ok(relative_path)) => {
                    let path = self.root.join(&relative_path);
                    match safe_io::check(&self.root, &path) {
                        Err(failure) if failure.code == ErrorCode::HistoryMissing => {
                            SnapshotReferenceIssueReason::Missing
                        }
                        Err(_) => SnapshotReferenceIssueReason::Unsafe,
                        Ok(()) => match fs::symlink_metadata(&path) {
                            Ok(metadata) if metadata.is_file() => continue,
                            Ok(_) => SnapshotReferenceIssueReason::Missing,
                            Err(error_value)
                                if error_value.kind() == std::io::ErrorKind::NotFound =>
                            {
                                SnapshotReferenceIssueReason::Missing
                            }
                            Err(_) => SnapshotReferenceIssueReason::Unsafe,
                        },
                    }
                }
            };
            issues.push(SnapshotReferenceIssue {
                note_id: document.note_id.clone(),
                note_path: document.relative_path.clone(),
                reference,
                reason,
            });
        }
        issues.sort();
        issues.dedup();
        Ok(HistoryRevisionContent {
            document,
            reference_report: SnapshotReferenceReport {
                complete: issues.is_empty(),
                issues,
            },
        })
    }

    pub fn restore(
        &self,
        storage: &mut VaultStorage,
        note_id: &str,
        revision_id: &str,
        expected_revision: u64,
        operation_id: &str,
    ) -> Result<SaveResult, ServiceError> {
        if storage.vault().vault_id != self.vault_id {
            return Err(error(ErrorCode::InvalidInput, "error.vaultSessionChanged"));
        }
        let historical = self.read(note_id, revision_id)?;
        let current = storage.read(note_id)?;
        if current.local_revision == expected_revision {
            self.capture(&current)?;
        }
        storage.restore_revision(
            note_id,
            &historical.source,
            expected_revision,
            revision_id,
            operation_id,
        )
    }
}

impl RevisionArchive for HistoryStore {
    fn force_preserve(&self, revision: &NoteDocument) -> Result<(), ServiceError> {
        self.capture(revision).map(|_| ())
    }
    fn preserve(&self, revision: &NoteDocument) -> Result<(), ServiceError> {
        self.validate_note(revision)?;
        let recent = self
            .recent
            .lock()
            .map_err(|_| error(ErrorCode::IoDenied, "error.historyLock"))?
            .get(&revision.note_id)
            .cloned();
        let recent = recent.or_else(|| {
            self.list(&revision.note_id)
                .ok()
                .and_then(|items| items.into_iter().next())
        });
        if let Some(recent) = recent {
            if self.record(&revision.note_id, &recent.revision_id).is_ok()
                && recent.local_revision <= revision.local_revision
            {
                let saved_at = chrono::DateTime::parse_from_rfc3339(&recent.saved_at)
                    .map_err(|_| error(ErrorCode::HistoryMissing, "error.historyCorrupt"))?;
                let elapsed = Utc::now().signed_duration_since(saved_at);
                if elapsed.num_seconds() >= 0 && elapsed.num_seconds() < 300 {
                    return Ok(());
                }
            }
        }
        self.capture(revision).map(|_| ())
    }
    fn verify(&self, note_id: &str, revision: u64, content_hash: &str) -> Result<(), ServiceError> {
        let id = format!("r{revision}-{content_hash}");
        self.read(note_id, &id).map(|_| ())
    }
}
