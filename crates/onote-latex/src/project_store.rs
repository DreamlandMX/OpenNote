//! M13 LaTeX project record (F45).
//!
//! The record gives every project file a stable `fileId` and a revision, and
//! remembers recent save receipts, so the editor never names a disk path and a
//! retried save is answered instead of applied twice. It is pure data: the host
//! reads and writes it next to the project (`.opennote/latex/<id>/project.json`)
//! and does all I/O.
//!
//! A save is journaled in the record itself:
//!
//! 1. `begin_save` checks the revision and returns a record carrying `pending`;
//!    the host writes that record first;
//! 2. the host replaces the source file (compare-and-swap on the old hash);
//! 3. `complete_save` (or `abandon_save`) produces the final record.
//!
//! If the app stops between steps, `resolve_pending` decides from the bytes now
//! on disk whether the save happened, so neither the record nor the source is
//! guessed and a newer external edit is never taken for ours.

use serde::{Deserialize, Serialize};

/// Format written by this build. A newer format is read-only here.
pub const RECORD_FORMAT_VERSION: u32 = 1;
/// Save receipts kept for retries; older ones are dropped first.
pub const MAX_RECEIPTS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedFile {
    pub file_id: String,
    /// Vault-relative path with `/` separators.
    pub path: String,
    pub sha256: String,
    /// Increases whenever the bytes this record knows change.
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReceipt {
    pub operation_id: String,
    pub file_id: String,
    pub sha256: String,
    pub revision: u64,
}

/// A save that was started but not yet confirmed in the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSave {
    pub operation_id: String,
    pub file_id: String,
    pub from_sha256: String,
    pub to_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub format_version: u32,
    pub project_id: String,
    /// Vault-relative project folder; empty for the vault root.
    pub root: String,
    pub entry_file_id: String,
    pub files: Vec<TrackedFile>,
    #[serde(default)]
    pub receipts: Vec<SaveReceipt>,
    #[serde(default)]
    pub pending: Option<PendingSave>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The record was written by a newer build; it is not modified.
    NewerFormat,
    /// The entry is not one of the listed `.tex` files.
    EntryInvalid,
    UnknownFile,
    /// The file changed since the editor loaded `expected`.
    RevisionConflict {
        current: u64,
    },
    /// The operation id was already used for a different save.
    OperationReused,
    /// An unresolved save must be settled with `resolve_pending` first.
    SavePending,
}

impl StoreError {
    /// The message key the UI resolves.
    pub fn message_key(&self) -> &'static str {
        match self {
            StoreError::NewerFormat => "error.projectRecordNewer",
            StoreError::EntryInvalid => "error.projectEntryInvalid",
            StoreError::UnknownFile => "error.projectFileUnknown",
            StoreError::RevisionConflict { .. } => "error.projectFileChanged",
            StoreError::OperationReused => "error.operationReused",
            StoreError::SavePending => "error.projectSavePending",
        }
    }
}

/// What a save request needs from the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveStep {
    /// Write `record` (it carries `pending`), then replace the source whose
    /// current hash must still be `expected_disk_sha256`.
    Write {
        record: ProjectRecord,
        path: String,
        expected_disk_sha256: String,
    },
    /// This operation already completed; answer with its receipt.
    AlreadyDone(SaveReceipt),
    /// The bytes equal what the record already has; nothing to write.
    Unchanged(TrackedFile),
}

/// How an interrupted save was settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// No save was pending.
    Nothing,
    /// The new bytes are on disk; the record now says so.
    Completed,
    /// The old bytes are still on disk; the save is dropped.
    Abandoned,
    /// Neither: someone else changed the file. The save is dropped and the
    /// file's next `sync_files` reports the external bytes.
    External,
}

/// What `sync_files` changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

fn is_tex(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".tex")
}

impl ProjectRecord {
    /// A record for a newly registered project. `listing` is `(path, sha256)`
    /// for every file; `new_id` supplies fresh file ids.
    pub fn new(
        project_id: &str,
        root: &str,
        listing: &[(String, String)],
        entry_path: &str,
        mut new_id: impl FnMut() -> String,
    ) -> Result<Self, StoreError> {
        if !is_tex(entry_path) || !listing.iter().any(|(path, _)| path == entry_path) {
            return Err(StoreError::EntryInvalid);
        }
        let files: Vec<TrackedFile> = listing
            .iter()
            .map(|(path, sha256)| TrackedFile {
                file_id: new_id(),
                path: path.clone(),
                sha256: sha256.clone(),
                revision: 1,
            })
            .collect();
        let entry_file_id = files
            .iter()
            .find(|file| file.path == entry_path)
            .map(|file| file.file_id.clone())
            .ok_or(StoreError::EntryInvalid)?;
        Ok(Self {
            format_version: RECORD_FORMAT_VERSION,
            project_id: project_id.to_owned(),
            root: root.to_owned(),
            entry_file_id,
            files,
            receipts: Vec::new(),
            pending: None,
        })
    }

    fn writable(&self) -> Result<(), StoreError> {
        if self.format_version > RECORD_FORMAT_VERSION {
            return Err(StoreError::NewerFormat);
        }
        Ok(())
    }

    pub fn file(&self, file_id: &str) -> Option<&TrackedFile> {
        self.files.iter().find(|file| file.file_id == file_id)
    }

    /// Brings the record in line with a fresh listing: paths keep their ids,
    /// new paths get new ids, changed bytes raise the revision, missing paths
    /// are removed. The entry keeps its id while its path exists.
    pub fn sync_files(
        &mut self,
        listing: &[(String, String)],
        mut new_id: impl FnMut() -> String,
    ) -> Result<SyncReport, StoreError> {
        self.writable()?;
        if self.pending.is_some() {
            return Err(StoreError::SavePending);
        }
        let mut report = SyncReport::default();
        self.files.retain(|file| {
            let keep = listing.iter().any(|(path, _)| *path == file.path);
            if !keep {
                report.removed.push(file.path.clone());
            }
            keep
        });
        for (path, sha256) in listing {
            match self.files.iter_mut().find(|file| file.path == *path) {
                Some(file) if file.sha256 != *sha256 => {
                    file.sha256 = sha256.clone();
                    file.revision += 1;
                    report.changed.push(path.clone());
                }
                Some(_) => {}
                None => {
                    self.files.push(TrackedFile {
                        file_id: new_id(),
                        path: path.clone(),
                        sha256: sha256.clone(),
                        revision: 1,
                    });
                    report.added.push(path.clone());
                }
            }
        }
        self.files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(report)
    }

    /// Whether the entry file still exists in the record.
    pub fn entry(&self) -> Option<&TrackedFile> {
        self.file(&self.entry_file_id)
    }

    /// Step 1 of a save; see the module docs.
    pub fn begin_save(
        &self,
        file_id: &str,
        expected_revision: u64,
        new_sha256: &str,
        operation_id: &str,
    ) -> Result<SaveStep, StoreError> {
        self.writable()?;
        if let Some(receipt) = self
            .receipts
            .iter()
            .find(|receipt| receipt.operation_id == operation_id)
        {
            if receipt.file_id == file_id && receipt.sha256 == new_sha256 {
                return Ok(SaveStep::AlreadyDone(receipt.clone()));
            }
            return Err(StoreError::OperationReused);
        }
        if self.pending.is_some() {
            return Err(StoreError::SavePending);
        }
        let file = self.file(file_id).ok_or(StoreError::UnknownFile)?;
        if file.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                current: file.revision,
            });
        }
        if file.sha256 == new_sha256 {
            return Ok(SaveStep::Unchanged(file.clone()));
        }
        let mut record = self.clone();
        record.pending = Some(PendingSave {
            operation_id: operation_id.to_owned(),
            file_id: file_id.to_owned(),
            from_sha256: file.sha256.clone(),
            to_sha256: new_sha256.to_owned(),
        });
        Ok(SaveStep::Write {
            record,
            path: file.path.clone(),
            expected_disk_sha256: file.sha256.clone(),
        })
    }

    /// Step 3 after the source was replaced: records the new bytes.
    pub fn complete_save(&mut self) -> Option<SaveReceipt> {
        let pending = self.pending.take()?;
        let file = self
            .files
            .iter_mut()
            .find(|file| file.file_id == pending.file_id)?;
        file.sha256 = pending.to_sha256.clone();
        file.revision += 1;
        let receipt = SaveReceipt {
            operation_id: pending.operation_id,
            file_id: pending.file_id,
            sha256: pending.to_sha256,
            revision: file.revision,
        };
        self.receipts.push(receipt.clone());
        if self.receipts.len() > MAX_RECEIPTS {
            let excess = self.receipts.len() - MAX_RECEIPTS;
            self.receipts.drain(..excess);
        }
        Some(receipt)
    }

    /// Step 3 when the source was not replaced.
    pub fn abandon_save(&mut self) {
        self.pending = None;
    }

    /// The path of the file a pending save targets, for the host to hash.
    pub fn pending_path(&self) -> Option<&str> {
        let pending = self.pending.as_ref()?;
        self.file(&pending.file_id).map(|file| file.path.as_str())
    }

    /// Settles an interrupted save from the bytes now on disk (`None`: the
    /// file is gone).
    pub fn resolve_pending(&mut self, disk_sha256: Option<&str>) -> Recovery {
        let Some(pending) = self.pending.as_ref() else {
            return Recovery::Nothing;
        };
        if disk_sha256 == Some(pending.to_sha256.as_str()) {
            self.complete_save();
            Recovery::Completed
        } else if disk_sha256 == Some(pending.from_sha256.as_str()) {
            self.abandon_save();
            Recovery::Abandoned
        } else {
            self.abandon_save();
            Recovery::External
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("f{next}")
        }
    }

    fn listing(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(path, sha)| ((*path).to_owned(), (*sha).to_owned()))
            .collect()
    }

    fn record() -> ProjectRecord {
        ProjectRecord::new(
            "p1",
            "thesis",
            &listing(&[("thesis/main.tex", "a"), ("thesis/refs.bib", "b")]),
            "thesis/main.tex",
            ids(),
        )
        .unwrap()
    }

    fn saved(
        record: &ProjectRecord,
        file: &str,
        revision: u64,
        sha: &str,
        op: &str,
    ) -> ProjectRecord {
        match record.begin_save(file, revision, sha, op).unwrap() {
            SaveStep::Write { mut record, .. } => {
                record.complete_save().unwrap();
                record
            }
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn the_entry_must_be_a_listed_tex_file() {
        let files = listing(&[("p/main.tex", "a"), ("p/refs.bib", "b")]);
        assert_eq!(
            ProjectRecord::new("p", "p", &files, "p/refs.bib", ids()).unwrap_err(),
            StoreError::EntryInvalid
        );
        assert_eq!(
            ProjectRecord::new("p", "p", &files, "p/other.tex", ids()).unwrap_err(),
            StoreError::EntryInvalid
        );
        let record = ProjectRecord::new("p", "p", &files, "p/main.tex", ids()).unwrap();
        assert_eq!(record.entry().unwrap().path, "p/main.tex");
    }

    #[test]
    fn a_save_is_journaled_then_completed_with_a_receipt() {
        let record = record();
        let step = record.begin_save("f1", 1, "a2", "op1").unwrap();
        let SaveStep::Write {
            mut record,
            path,
            expected_disk_sha256,
        } = step
        else {
            panic!("expected a write");
        };
        assert_eq!(
            (path.as_str(), expected_disk_sha256.as_str()),
            ("thesis/main.tex", "a")
        );
        assert!(
            record.pending.is_some(),
            "the journal is written before the source"
        );
        let receipt = record.complete_save().unwrap();
        assert_eq!((receipt.revision, receipt.sha256.as_str()), (2, "a2"));
        assert_eq!(record.file("f1").unwrap().revision, 2);
        assert!(record.pending.is_none());
    }

    #[test]
    fn a_retry_is_answered_from_its_receipt_and_a_reused_id_is_refused() {
        let record = saved(&record(), "f1", 1, "a2", "op1");
        assert!(matches!(
            record.begin_save("f1", 1, "a2", "op1").unwrap(),
            SaveStep::AlreadyDone(receipt) if receipt.revision == 2
        ));
        assert_eq!(
            record.begin_save("f1", 2, "other", "op1").unwrap_err(),
            StoreError::OperationReused
        );
    }

    #[test]
    fn a_stale_revision_is_a_conflict() {
        let record = saved(&record(), "f1", 1, "a2", "op1");
        assert_eq!(
            record.begin_save("f1", 1, "a3", "op2").unwrap_err(),
            StoreError::RevisionConflict { current: 2 }
        );
        assert_eq!(
            record.begin_save("nope", 1, "x", "op3").unwrap_err(),
            StoreError::UnknownFile
        );
    }

    #[test]
    fn saving_the_same_bytes_writes_nothing() {
        assert!(matches!(
            record().begin_save("f1", 1, "a", "op1").unwrap(),
            SaveStep::Unchanged(file) if file.revision == 1
        ));
    }

    #[test]
    fn an_interrupted_save_is_settled_from_the_disk() {
        let pending =
            |record: &ProjectRecord| match record.begin_save("f1", 1, "a2", "op1").unwrap() {
                SaveStep::Write { record, .. } => record,
                _ => panic!(),
            };
        let mut done = pending(&record());
        assert_eq!(done.resolve_pending(Some("a2")), Recovery::Completed);
        assert_eq!(done.file("f1").unwrap().revision, 2);
        assert!(done
            .receipts
            .iter()
            .any(|receipt| receipt.operation_id == "op1"));

        let mut not_done = pending(&record());
        assert_eq!(not_done.resolve_pending(Some("a")), Recovery::Abandoned);
        assert_eq!(not_done.file("f1").unwrap().revision, 1);

        let mut external = pending(&record());
        assert_eq!(external.resolve_pending(Some("zzz")), Recovery::External);
        assert!(external.pending.is_none());
        // The external bytes are then picked up as a change, not as our save.
        let report = external
            .sync_files(
                &listing(&[("thesis/main.tex", "zzz"), ("thesis/refs.bib", "b")]),
                ids(),
            )
            .unwrap();
        assert_eq!(report.changed, vec!["thesis/main.tex".to_owned()]);
        assert!(external.receipts.is_empty());

        let mut gone = pending(&record());
        assert_eq!(gone.resolve_pending(None), Recovery::External);
    }

    #[test]
    fn a_pending_save_blocks_further_saves_and_syncs() {
        let SaveStep::Write { mut record, .. } = record().begin_save("f1", 1, "a2", "op1").unwrap()
        else {
            panic!();
        };
        assert_eq!(
            record.begin_save("f2", 1, "b2", "op2").unwrap_err(),
            StoreError::SavePending
        );
        assert_eq!(
            record.sync_files(&[], ids()).unwrap_err(),
            StoreError::SavePending
        );
    }

    #[test]
    fn sync_keeps_ids_for_known_paths() {
        let mut record = record();
        let mut fresh = {
            let mut next = 100;
            move || {
                next += 1;
                format!("n{next}")
            }
        };
        let report = record
            .sync_files(
                &listing(&[("thesis/main.tex", "a"), ("thesis/ch1.tex", "c")]),
                &mut fresh,
            )
            .unwrap();
        assert_eq!(report.added, vec!["thesis/ch1.tex".to_owned()]);
        assert_eq!(report.removed, vec!["thesis/refs.bib".to_owned()]);
        assert_eq!(record.entry().unwrap().file_id, "f1");
        assert_eq!(
            record
                .files
                .iter()
                .find(|f| f.path == "thesis/ch1.tex")
                .unwrap()
                .file_id,
            "n101"
        );
    }

    #[test]
    fn a_newer_record_is_read_only() {
        let mut record = record();
        record.format_version = RECORD_FORMAT_VERSION + 1;
        assert_eq!(
            record.begin_save("f1", 1, "a2", "op").unwrap_err(),
            StoreError::NewerFormat
        );
        assert_eq!(
            record.sync_files(&[], ids()).unwrap_err(),
            StoreError::NewerFormat
        );
    }

    #[test]
    fn receipts_are_bounded() {
        let mut record = record();
        for index in 0..(MAX_RECEIPTS + 5) {
            let revision = record.file("f1").unwrap().revision;
            record = saved(
                &record,
                "f1",
                revision,
                &format!("s{index}"),
                &format!("op{index}"),
            );
        }
        assert_eq!(record.receipts.len(), MAX_RECEIPTS);
        assert_eq!(record.receipts[0].operation_id, "op5");
    }
}
