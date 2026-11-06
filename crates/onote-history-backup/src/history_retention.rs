use super::*;
use serde::de::DeserializeOwned;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 30;
const PREFERENCES_FORMAT_VERSION: u32 = 1;
const PREFERENCES_FILE: &str = "preferences.json";
const CLEANUP_FORMAT_VERSION: u32 = 1;
const MAX_PREVIEW_AGE_SECONDS: i64 = 24 * 60 * 60;
const MAX_CLOCK_SKEW_SECONDS: i64 = 5 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRetentionSettings {
    pub retention_days: u32,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPreferences {
    retention_days: u32,
    revision: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferencesEnvelope {
    format_version: u32,
    settings: StoredPreferences,
    checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRetentionCandidate {
    pub revision: HistoryRevision,
    pub file_hash: String,
    pub byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryPreservationReason {
    PendingOperation,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPreservedRevision {
    pub note_id: String,
    pub revision_id: String,
    pub reason: HistoryPreservationReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRetentionPlan {
    pub plan_id: String,
    pub retention_days: u32,
    pub settings_revision: u64,
    pub generated_at: String,
    pub cutoff: String,
    pub retained_count: u64,
    pub candidates: Vec<HistoryRetentionCandidate>,
    pub preserved: Vec<HistoryPreservedRevision>,
    pub candidate_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingHistoryCleanup {
    pub operation_id: String,
    pub plan: HistoryRetentionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryCleanupPreview {
    pub current: HistoryRetentionPlan,
    pub pending: Vec<PendingHistoryCleanup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryCleanupResult {
    pub operation_id: String,
    pub status: String,
    pub removed: Vec<HistoryRetentionCandidate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remaining: Vec<HistoryRetentionCandidate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCleanupProgress {
    pub removed: u64,
    pub total: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupRequest {
    format_version: u32,
    operation_id: String,
    vault_id: String,
    plan: HistoryRetentionPlan,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupReceipt {
    request_hash: String,
    result: HistoryCleanupResult,
}

fn corrupt_preferences() -> ServiceError {
    error(ErrorCode::HistoryMissing, "error.historyPreferencesCorrupt")
}

fn corrupt_cleanup() -> ServiceError {
    error(ErrorCode::BackupIncomplete, "error.historyCleanupCorrupt")
}

fn default_settings() -> HistoryRetentionSettings {
    HistoryRetentionSettings {
        retention_days: DEFAULT_HISTORY_RETENTION_DAYS,
        revision: 0,
    }
}

fn validate_days(days: u32) -> Result<(), ServiceError> {
    if days == 0 {
        return Err(error(
            ErrorCode::InvalidInput,
            "error.historyRetentionInvalid",
        ));
    }
    Ok(())
}

fn read_json<T: DeserializeOwned>(root: &Path, path: &Path) -> Result<T, ServiceError> {
    let bytes = safe_io::read(root, path)?;
    serde_json::from_slice(&bytes).map_err(|_| corrupt_cleanup())
}

fn remaining_candidates(
    candidates: &[HistoryRetentionCandidate],
    removed: &[HistoryRetentionCandidate],
) -> Vec<HistoryRetentionCandidate> {
    let removed: HashSet<_> = removed
        .iter()
        .map(|candidate| (&candidate.revision.note_id, &candidate.revision.revision_id))
        .collect();
    candidates
        .iter()
        .filter(|candidate| {
            !removed.contains(&(&candidate.revision.note_id, &candidate.revision.revision_id))
        })
        .cloned()
        .collect()
}

impl HistoryStore {
    fn check_storage(&self, storage: &VaultStorage) -> Result<(), ServiceError> {
        if storage.vault().vault_id != self.vault_id
            || fs::canonicalize(storage.root_path()).map_err(safe_io::io)? != self.root
        {
            return Err(error(ErrorCode::InvalidInput, "error.vaultSessionChanged"));
        }
        Ok(())
    }

    fn history_root(&self) -> PathBuf {
        self.root.join(".opennote/history")
    }

    fn preferences_path(&self) -> PathBuf {
        self.history_root().join(PREFERENCES_FILE)
    }

    fn cleanup_journal(&self, operation_id: &str) -> PathBuf {
        self.history_root().join(format!(".cleanup-{operation_id}"))
    }

    pub fn retention_settings(&self) -> Result<HistoryRetentionSettings, ServiceError> {
        let path = self.preferences_path();
        let bytes = match safe_io::read(&self.root, &path) {
            Err(failure) if failure.code == ErrorCode::HistoryMissing => {
                return Ok(default_settings())
            }
            Err(failure) => return Err(failure),
            Ok(bytes) => bytes,
        };
        let envelope: PreferencesEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| corrupt_preferences())?;
        let payload = serde_json::to_vec(&envelope.settings).map_err(|_| corrupt_preferences())?;
        if envelope.format_version != PREFERENCES_FORMAT_VERSION
            || envelope.settings.revision == 0
            || hash(&payload) != envelope.checksum
        {
            return Err(corrupt_preferences());
        }
        validate_days(envelope.settings.retention_days)?;
        Ok(HistoryRetentionSettings {
            retention_days: envelope.settings.retention_days,
            revision: envelope.settings.revision,
        })
    }

    pub fn set_retention(
        &self,
        storage: &VaultStorage,
        retention_days: u32,
        expected_revision: u64,
    ) -> Result<HistoryRetentionSettings, ServiceError> {
        self.check_storage(storage)?;
        storage.with_history_write(|| self.write_retention(retention_days, expected_revision))
    }

    fn write_retention(
        &self,
        retention_days: u32,
        expected_revision: u64,
    ) -> Result<HistoryRetentionSettings, ServiceError> {
        validate_days(retention_days)?;
        let current = self.retention_settings()?;
        if current.revision != expected_revision {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.historyRetentionChanged",
            ));
        }
        let stored = StoredPreferences {
            retention_days,
            revision: current
                .revision
                .checked_add(1)
                .ok_or_else(|| error(ErrorCode::InvalidInput, "error.historyRetentionInvalid"))?,
        };
        let checksum = hash(
            &serde_json::to_vec(&stored)
                .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?,
        );
        let bytes = serde_json::to_vec(&PreferencesEnvelope {
            format_version: PREFERENCES_FORMAT_VERSION,
            settings: stored.clone(),
            checksum,
        })
        .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?;
        safe_io::replace(&self.root, &self.preferences_path(), &bytes)?;
        Ok(HistoryRetentionSettings {
            retention_days: stored.retention_days,
            revision: stored.revision,
        })
    }

    fn plan_at(
        &self,
        storage: &VaultStorage,
        settings: &HistoryRetentionSettings,
        generated_at: chrono::DateTime<Utc>,
    ) -> Result<HistoryRetentionPlan, ServiceError> {
        self.check_storage(storage)?;
        validate_days(settings.retention_days)?;
        let cutoff = generated_at
            .checked_sub_signed(chrono::Duration::days(i64::from(settings.retention_days)))
            .ok_or_else(|| error(ErrorCode::InvalidInput, "error.historyRetentionInvalid"))?;
        let protected: HashSet<_> = storage.protected_note_ids()?.into_iter().collect();
        let history_root = self.history_root();
        let mut candidates = Vec::new();
        let mut preserved = Vec::new();
        let mut retained_count = 0u64;
        match safe_io::check(&self.root, &history_root) {
            Err(failure) if failure.code == ErrorCode::HistoryMissing => (),
            Err(failure) => return Err(failure),
            Ok(()) => {
                if !fs::symlink_metadata(&history_root)
                    .map_err(safe_io::io)?
                    .is_dir()
                {
                    return Err(error(ErrorCode::HistoryMissing, "error.historyInvalidFile"));
                }
                for entry in fs::read_dir(&history_root).map_err(safe_io::io)? {
                    let entry = entry.map_err(safe_io::io)?;
                    safe_io::check(&self.root, &entry.path())?;
                    let name = entry.file_name();
                    let Some(note_id) = name.to_str().filter(|id| canonical_uuid(id)) else {
                        continue;
                    };
                    if !entry.metadata().map_err(safe_io::io)?.is_dir() {
                        continue;
                    }
                    for revision_entry in fs::read_dir(entry.path()).map_err(safe_io::io)? {
                        let revision_entry = revision_entry.map_err(safe_io::io)?;
                        safe_io::check(&self.root, &revision_entry.path())?;
                        let revision_name = revision_entry.file_name();
                        let Some(revision_name) = revision_name.to_str() else {
                            preserved.push(HistoryPreservedRevision {
                                note_id: note_id.into(),
                                revision_id: "<invalid-name>".into(),
                                reason: HistoryPreservationReason::Unreadable,
                            });
                            continue;
                        };
                        if revision_name.starts_with(".pending-") {
                            continue;
                        }
                        let Some(revision_id) = revision_name.strip_suffix(".json") else {
                            continue;
                        };
                        let record = match self.record(note_id, revision_id) {
                            Ok(record) => record,
                            Err(_) => {
                                preserved.push(HistoryPreservedRevision {
                                    note_id: note_id.into(),
                                    revision_id: revision_id.into(),
                                    reason: HistoryPreservationReason::Unreadable,
                                });
                                continue;
                            }
                        };
                        if protected.contains(note_id) {
                            if chrono::DateTime::parse_from_rfc3339(&record.revision.saved_at)
                                .map_err(|_| corrupt_cleanup())?
                                < cutoff
                            {
                                preserved.push(HistoryPreservedRevision {
                                    note_id: note_id.into(),
                                    revision_id: revision_id.into(),
                                    reason: HistoryPreservationReason::PendingOperation,
                                });
                            } else {
                                retained_count =
                                    retained_count.checked_add(1).ok_or_else(corrupt_cleanup)?;
                            }
                            continue;
                        }
                        let saved_at =
                            chrono::DateTime::parse_from_rfc3339(&record.revision.saved_at)
                                .map_err(|_| corrupt_cleanup())?;
                        if saved_at >= cutoff {
                            retained_count =
                                retained_count.checked_add(1).ok_or_else(corrupt_cleanup)?;
                            continue;
                        }
                        let path = self.path(note_id, revision_id)?;
                        let bytes = safe_io::read(&self.root, &path)?;
                        candidates.push(HistoryRetentionCandidate {
                            byte_length: bytes.len() as u64,
                            file_hash: hash(&bytes),
                            revision: record.revision,
                        });
                    }
                }
            }
        }
        candidates.sort_by(|a, b| {
            a.revision
                .note_id
                .cmp(&b.revision.note_id)
                .then_with(|| a.revision.saved_at.cmp(&b.revision.saved_at))
                .then_with(|| a.revision.revision_id.cmp(&b.revision.revision_id))
        });
        preserved.sort_by(|a, b| {
            a.note_id
                .cmp(&b.note_id)
                .then_with(|| a.revision_id.cmp(&b.revision_id))
        });
        let candidate_bytes = candidates
            .iter()
            .try_fold(0u64, |total, candidate| {
                total.checked_add(candidate.byte_length)
            })
            .ok_or_else(corrupt_cleanup)?;
        let mut plan = HistoryRetentionPlan {
            plan_id: String::new(),
            retention_days: settings.retention_days,
            settings_revision: settings.revision,
            generated_at: generated_at.to_rfc3339(),
            cutoff: cutoff.to_rfc3339(),
            retained_count,
            candidates,
            preserved,
            candidate_bytes,
        };
        plan.plan_id = hash(
            &serde_json::to_vec(&(&self.root, &self.vault_id, &plan))
                .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?,
        );
        Ok(plan)
    }

    pub fn preview_cleanup(
        &self,
        storage: &VaultStorage,
    ) -> Result<HistoryCleanupPreview, ServiceError> {
        let settings = self.retention_settings()?;
        let current = self.plan_at(storage, &settings, Utc::now())?;
        Ok(HistoryCleanupPreview {
            current,
            pending: self.pending_cleanups()?,
        })
    }

    fn cleanup_request(
        &self,
        journal: &Path,
        operation_id: &str,
        plan: &HistoryRetentionPlan,
    ) -> Result<(CleanupRequest, String), ServiceError> {
        let request: CleanupRequest = read_json(&self.root, &journal.join("request.json"))?;
        if request.format_version != CLEANUP_FORMAT_VERSION
            || request.operation_id != operation_id
            || request.vault_id != self.vault_id
            || request.plan != *plan
        {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        self.validate_plan_id(plan)?;
        let request_hash = hash(
            &serde_json::to_vec(&request)
                .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?,
        );
        Ok((request, request_hash))
    }

    fn validate_plan_id(&self, plan: &HistoryRetentionPlan) -> Result<(), ServiceError> {
        let mut unsigned = plan.clone();
        unsigned.plan_id.clear();
        let expected = hash(
            &serde_json::to_vec(&(&self.root, &self.vault_id, &unsigned))
                .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?,
        );
        if expected != plan.plan_id {
            return Err(corrupt_cleanup());
        }
        Ok(())
    }

    pub fn pending_cleanups(&self) -> Result<Vec<PendingHistoryCleanup>, ServiceError> {
        let history_root = self.history_root();
        match safe_io::check(&self.root, &history_root) {
            Err(failure) if failure.code == ErrorCode::HistoryMissing => return Ok(Vec::new()),
            Err(failure) => return Err(failure),
            Ok(()) => (),
        }
        let mut pending = Vec::new();
        for entry in fs::read_dir(&history_root).map_err(safe_io::io)? {
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(operation_id) = name
                .to_str()
                .and_then(|name| name.strip_prefix(".cleanup-"))
                .filter(|id| canonical_uuid(id))
            else {
                continue;
            };
            safe_io::check(&self.root, &entry.path())?;
            let request_path = entry.path().join("request.json");
            if !request_path.exists() {
                continue;
            }
            let request: CleanupRequest = read_json(&self.root, &request_path)?;
            if request.vault_id != self.vault_id {
                continue;
            }
            let (_, request_hash) =
                self.cleanup_request(&entry.path(), operation_id, &request.plan)?;
            let result_path = entry.path().join("result.json");
            if result_path.exists() {
                self.cleanup_result(&entry.path(), &request, &request_hash)?;
            } else {
                pending.push(PendingHistoryCleanup {
                    operation_id: operation_id.into(),
                    plan: request.plan,
                });
            }
        }
        pending.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        Ok(pending)
    }

    fn other_cleanup_pending(&self, operation_id: &str) -> Result<(), ServiceError> {
        if self
            .pending_cleanups()?
            .iter()
            .any(|pending| pending.operation_id != operation_id)
        {
            return Err(error(ErrorCode::FileLocked, "error.historyCleanupBusy"));
        }
        Ok(())
    }

    fn verify_plan(
        &self,
        storage: &VaultStorage,
        plan: &HistoryRetentionPlan,
        enforce_fresh: bool,
    ) -> Result<(), ServiceError> {
        let generated_at = chrono::DateTime::parse_from_rfc3339(&plan.generated_at)
            .map_err(|_| error(ErrorCode::VersionConflict, "error.restorePlanChanged"))?
            .with_timezone(&Utc);
        let now = Utc::now();
        if enforce_fresh
            && (generated_at > now + chrono::Duration::seconds(MAX_CLOCK_SKEW_SECONDS)
                || now - generated_at > chrono::Duration::seconds(MAX_PREVIEW_AGE_SECONDS))
        {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.restorePlanChanged",
            ));
        }
        let settings = self.retention_settings()?;
        if settings.retention_days != plan.retention_days
            || settings.revision != plan.settings_revision
        {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.restorePlanChanged",
            ));
        }
        self.validate_plan_id(plan)?;
        if self.plan_at(storage, &settings, generated_at)? != *plan {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.restorePlanChanged",
            ));
        }
        Ok(())
    }

    fn verify_resume_plan(
        &self,
        storage: &VaultStorage,
        plan: &HistoryRetentionPlan,
    ) -> Result<(), ServiceError> {
        chrono::DateTime::parse_from_rfc3339(&plan.generated_at)
            .map_err(|_| error(ErrorCode::VersionConflict, "error.restorePlanChanged"))?;
        let settings = self.retention_settings()?;
        if settings.retention_days != plan.retention_days
            || settings.revision != plan.settings_revision
        {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.restorePlanChanged",
            ));
        }
        self.validate_plan_id(plan)?;
        let protected: HashSet<_> = storage.protected_note_ids()?.into_iter().collect();
        if plan
            .candidates
            .iter()
            .any(|candidate| protected.contains(&candidate.revision.note_id))
        {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.restorePlanChanged",
            ));
        }
        Ok(())
    }

    fn verify_candidate(&self, candidate: &HistoryRetentionCandidate) -> Result<(), ServiceError> {
        let path = self.path(&candidate.revision.note_id, &candidate.revision.revision_id)?;
        let bytes = safe_io::read(&self.root, &path)?;
        if bytes.len() as u64 != candidate.byte_length
            || hash(&bytes) != candidate.file_hash
            || self
                .record(&candidate.revision.note_id, &candidate.revision.revision_id)?
                .revision
                != candidate.revision
        {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.restorePlanChanged",
            ));
        }
        Ok(())
    }

    fn cleanup_result(
        &self,
        journal: &Path,
        request: &CleanupRequest,
        request_hash: &str,
    ) -> Result<HistoryCleanupResult, ServiceError> {
        let receipt: CleanupReceipt = read_json(&self.root, &journal.join("result.json"))?;
        let mut remaining = request.plan.candidates.iter();
        if receipt.request_hash != request_hash
            || receipt.result.operation_id != request.operation_id
            || !matches!(receipt.result.status.as_str(), "completed" | "cancelled")
            || (receipt.result.status == "completed"
                && receipt.result.removed != request.plan.candidates)
            || !receipt
                .result
                .removed
                .iter()
                .all(|removed| remaining.any(|candidate| candidate == removed))
            || receipt.result.remaining
                != remaining_candidates(&request.plan.candidates, &receipt.result.removed)
        {
            return Err(corrupt_cleanup());
        }
        Ok(receipt.result)
    }

    fn finish_cleanup(
        &self,
        journal: &Path,
        request_hash: String,
        operation_id: &str,
        status: &str,
        removed: Vec<HistoryRetentionCandidate>,
        candidates: &[HistoryRetentionCandidate],
    ) -> Result<HistoryCleanupResult, ServiceError> {
        let remaining = remaining_candidates(candidates, &removed);
        let result = HistoryCleanupResult {
            operation_id: operation_id.into(),
            status: status.into(),
            removed,
            remaining,
        };
        safe_io::publish(
            &self.root,
            &journal.join("result.json"),
            &serde_json::to_vec(&CleanupReceipt {
                request_hash,
                result: result.clone(),
            })
            .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?,
        )?;
        Ok(result)
    }

    pub fn cancel_cleanup(
        &self,
        storage: &VaultStorage,
        operation_id: &str,
    ) -> Result<HistoryCleanupResult, ServiceError> {
        self.check_storage(storage)?;
        if !canonical_uuid(operation_id) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidOperationId"));
        }
        storage.with_history_write(|| {
            let journal = self.cleanup_journal(operation_id);
            let request: CleanupRequest = read_json(&self.root, &journal.join("request.json"))?;
            let (request, request_hash) =
                self.cleanup_request(&journal, operation_id, &request.plan)?;
            if journal.join("result.json").exists() {
                return self.cleanup_result(&journal, &request, &request_hash);
            }
            let mut removed = Vec::new();
            for candidate in &request.plan.candidates {
                let path =
                    self.path(&candidate.revision.note_id, &candidate.revision.revision_id)?;
                match fs::symlink_metadata(&path) {
                    Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                        removed.push(candidate.clone())
                    }
                    Err(failure) => return Err(safe_io::io(failure)),
                    Ok(_) => safe_io::check(&self.root, &path)?,
                }
            }
            self.finish_cleanup(
                &journal,
                request_hash,
                operation_id,
                "cancelled",
                removed,
                &request.plan.candidates,
            )
        })
    }

    pub fn apply_cleanup(
        &self,
        storage: &VaultStorage,
        plan: &HistoryRetentionPlan,
        operation_id: &str,
        cancel: &AtomicBool,
        mut progress: impl FnMut(HistoryCleanupProgress),
    ) -> Result<HistoryCleanupResult, ServiceError> {
        self.check_storage(storage)?;
        if !canonical_uuid(operation_id) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidOperationId"));
        }
        storage.with_history_write(|| {
            let journal = self.cleanup_journal(operation_id);
            let request_path = journal.join("request.json");
            let resuming = request_path.exists();
            if resuming {
                let (request, request_hash) = self.cleanup_request(&journal, operation_id, plan)?;
                let result_path = journal.join("result.json");
                if result_path.exists() {
                    return self.cleanup_result(&journal, &request, &request_hash);
                }
                self.verify_resume_plan(storage, plan)?;
            } else {
                self.other_cleanup_pending(operation_id)?;
                self.verify_plan(storage, plan, true)?;
                safe_io::ensure_dir(&self.root, &journal)?;
                let request = CleanupRequest {
                    format_version: CLEANUP_FORMAT_VERSION,
                    operation_id: operation_id.into(),
                    vault_id: self.vault_id.clone(),
                    plan: plan.clone(),
                };
                safe_io::publish(
                    &self.root,
                    &request_path,
                    &serde_json::to_vec(&request)
                        .map_err(|_| error(ErrorCode::IoDenied, "error.historySerialize"))?,
                )?;
            }
            let (request, request_hash) = self.cleanup_request(&journal, operation_id, plan)?;
            // Recheck immediately before deletion; no newly pending note may lose
            // the source revision an unfinished restore still needs.
            self.verify_resume_plan(storage, &request.plan)?;
            let mut removed = Vec::new();
            let total = request.plan.candidates.len() as u64;
            progress(HistoryCleanupProgress { removed: 0, total });
            for candidate in &request.plan.candidates {
                if cancel.load(Ordering::SeqCst) {
                    // Include deletions completed before a process interruption, without deleting anything else.
                    for candidate in &request.plan.candidates[removed.len()..] {
                        let path = self
                            .path(&candidate.revision.note_id, &candidate.revision.revision_id)?;
                        match fs::symlink_metadata(path) {
                            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                                removed.push(candidate.clone())
                            }
                            Err(failure) => return Err(safe_io::io(failure)),
                            Ok(_) => (),
                        }
                    }
                    return self.finish_cleanup(
                        &journal,
                        request_hash,
                        operation_id,
                        "cancelled",
                        removed,
                        &request.plan.candidates,
                    );
                }
                let path =
                    self.path(&candidate.revision.note_id, &candidate.revision.revision_id)?;
                match fs::symlink_metadata(&path) {
                    Ok(_) => {
                        self.verify_candidate(candidate)?;
                        safe_io::check(&self.root, &path)?;
                        fs::remove_file(path).map_err(safe_io::io)?;
                        #[cfg(test)]
                        if take_cleanup_test_stop() {
                            return Err(error(
                                ErrorCode::RestoreInterrupted,
                                "error.historyCleanupInterrupted",
                            ));
                        }
                    }
                    Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => (),
                    Err(failure) => return Err(safe_io::io(failure)),
                }
                removed.push(candidate.clone());
                progress(HistoryCleanupProgress {
                    removed: removed.len() as u64,
                    total,
                });
            }
            self.finish_cleanup(
                &journal,
                request_hash,
                operation_id,
                "completed",
                removed,
                &request.plan.candidates,
            )
        })
    }
}

#[cfg(test)]
thread_local! {
    static CLEANUP_TEST_STOP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn take_cleanup_test_stop() -> bool {
    CLEANUP_TEST_STOP.with(|stop| stop.replace(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use onote_vault_storage::Failpoint;
    use std::sync::Arc;

    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn age_revision(
        dir: &tempfile::TempDir,
        revision: &HistoryRevision,
        days: i64,
    ) -> HistoryRevision {
        let path = dir
            .path()
            .join(".opennote/history")
            .join(&revision.note_id)
            .join(format!("{}.json", revision.revision_id));
        let mut envelope: Envelope = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope.record.revision.saved_at =
            (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        envelope.checksum = hash(&serde_json::to_vec(&envelope.record).unwrap());
        fs::write(path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        envelope.record.revision
    }

    #[test]
    fn retention_defaults_to_thirty_days_and_cleanup_replays_from_its_journal() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "retention").unwrap();
        let history = Arc::new(HistoryStore::open(dir.path(), &storage.vault().vault_id).unwrap());
        storage.set_revision_archive(history.clone());
        let note = storage.create_note("keep.md", "old body", &op()).unwrap();
        let old = history.capture(&note).unwrap();
        let old = age_revision(&dir, &old, 31);
        storage
            .save(&note.note_id, "new body", note.local_revision, &op())
            .unwrap();
        assert_eq!(
            history.retention_settings().unwrap(),
            HistoryRetentionSettings {
                retention_days: 30,
                revision: 0
            }
        );
        let preview = history.preview_cleanup(&storage).unwrap();
        assert_eq!(preview.current.candidates.len(), 1);
        assert_eq!(preview.current.candidates[0].revision, old);
        let saved = history.set_retention(&storage, 30, 0).unwrap();
        assert_eq!(saved.revision, 1);
        let plan = history.preview_cleanup(&storage).unwrap().current;
        let operation = op();
        CLEANUP_TEST_STOP.with(|stop| stop.set(true));
        assert_eq!(
            history
                .apply_cleanup(&storage, &plan, &operation, &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .code,
            ErrorCode::RestoreInterrupted
        );
        assert_eq!(history.pending_cleanups().unwrap().len(), 1);
        let resumed = history
            .apply_cleanup(&storage, &plan, &operation, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(resumed.removed, plan.candidates);
        assert_eq!(
            history
                .apply_cleanup(&storage, &plan, &operation, &AtomicBool::new(false), |_| {})
                .unwrap(),
            resumed
        );
        assert!(history
            .list(&note.note_id)
            .unwrap()
            .iter()
            .all(|version| version != &old));
        assert!(storage
            .read(&note.note_id)
            .unwrap()
            .source
            .ends_with("new body"));
    }

    #[test]
    fn cleanup_preserves_revisions_needed_by_an_interrupted_restore() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "retention").unwrap();
        let history = Arc::new(HistoryStore::open(dir.path(), &storage.vault().vault_id).unwrap());
        storage.set_revision_archive(history.clone());
        let note = storage
            .create_note("restore.md", "historical", &op())
            .unwrap();
        let old = history.capture(&note).unwrap();
        let old = age_revision(&dir, &old, 31);
        let current = storage
            .save(&note.note_id, "current", note.local_revision, &op())
            .unwrap();
        storage.push_failpoint(Failpoint::AfterReplace);
        assert!(history
            .restore(
                &mut storage,
                &note.note_id,
                &old.revision_id,
                current.local_revision,
                &op(),
            )
            .is_err());
        let plan = history.preview_cleanup(&storage).unwrap().current;
        assert!(plan.candidates.is_empty());
        assert!(plan.preserved.iter().any(|preserved| {
            preserved.revision_id == old.revision_id
                && preserved.reason == HistoryPreservationReason::PendingOperation
        }));
    }

    #[test]
    fn changed_history_file_invalidates_cleanup_without_deleting_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "retention").unwrap();
        let history = Arc::new(HistoryStore::open(dir.path(), &storage.vault().vault_id).unwrap());
        storage.set_revision_archive(history.clone());
        let note = storage.create_note("changed.md", "old", &op()).unwrap();
        let old = history.capture(&note).unwrap();
        let old = age_revision(&dir, &old, 31);
        let plan = history.preview_cleanup(&storage).unwrap().current;
        let path = dir
            .path()
            .join(".opennote/history")
            .join(&note.note_id)
            .join(format!("{}.json", old.revision_id));
        fs::write(&path, b"{}").unwrap();
        assert_eq!(
            history
                .apply_cleanup(&storage, &plan, &op(), &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .code,
            ErrorCode::VersionConflict
        );
        assert_eq!(fs::read(path).unwrap(), b"{}");
    }

    fn cleanup_fixture() -> (tempfile::TempDir, VaultStorage, Arc<HistoryStore>) {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "retention").unwrap();
        let history = Arc::new(HistoryStore::open(dir.path(), &storage.vault().vault_id).unwrap());
        storage.set_revision_archive(history.clone());
        for path in ["a.md", "b.md"] {
            let note = storage.create_note(path, "original", &op()).unwrap();
            let revision = history.capture(&note).unwrap();
            age_revision(&dir, &revision, 31);
        }
        (dir, storage, history)
    }

    #[test]
    fn readonly_formats_and_stale_metadata_refuse_history_mutations() {
        for schema in [1, 2, 3] {
            let (dir, storage, history) = cleanup_fixture();
            let plan = history.preview_cleanup(&storage).unwrap().current;
            let path = dir.path().join(".opennote/vault.json");
            let mut metadata: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            metadata["schemaVersion"] = schema.into();
            metadata["writable"] = false.into();
            fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
            assert!(history.set_retention(&storage, 1, 0).is_err());
            drop(storage);
            let readonly = VaultStorage::open(dir.path()).unwrap();
            assert!(!readonly.vault().writable);
            let before = fs::read(&path).unwrap();
            assert!(history.set_retention(&readonly, 1, 0).is_err());
            assert!(history
                .apply_cleanup(&readonly, &plan, &op(), &AtomicBool::new(false), |_| {})
                .is_err());
            assert!(!history.preferences_path().exists());
            assert_eq!(fs::read(path).unwrap(), before);
            for candidate in plan.candidates {
                history.verify_candidate(&candidate).unwrap();
            }
        }
    }

    #[test]
    fn history_maintenance_obeys_writer_lock_revision_and_source_identity() {
        let (dir, storage, history) = cleanup_fixture();
        let plan = history.preview_cleanup(&storage).unwrap().current;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.path().join(".opennote/writer.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        assert_eq!(
            history.set_retention(&storage, 1, 0).unwrap_err().code,
            ErrorCode::FileLocked
        );
        assert_eq!(
            history
                .apply_cleanup(&storage, &plan, &op(), &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .code,
            ErrorCode::FileLocked
        );
        drop(lock);
        history.set_retention(&storage, 45, 0).unwrap();
        let other = HistoryStore::open(dir.path(), &storage.vault().vault_id).unwrap();
        assert_eq!(
            other.set_retention(&storage, 60, 0).unwrap_err().code,
            ErrorCode::VersionConflict
        );
        let clone_root = tempfile::tempdir().unwrap();
        let clone = VaultStorage::create(clone_root.path(), "other").unwrap();
        assert!(history.set_retention(&clone, 1, 1).is_err());
        assert_eq!(history.retention_settings().unwrap().retention_days, 45);
    }

    #[test]
    fn interrupted_cleanup_can_be_stopped_after_settings_change_without_deleting_more() {
        let (_dir, storage, history) = cleanup_fixture();
        let plan = history.preview_cleanup(&storage).unwrap().current;
        let operation = op();
        CLEANUP_TEST_STOP.with(|stop| stop.set(true));
        assert_eq!(
            history
                .apply_cleanup(&storage, &plan, &operation, &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .code,
            ErrorCode::RestoreInterrupted
        );
        history.set_retention(&storage, 1, 0).unwrap();
        assert_eq!(
            history
                .apply_cleanup(&storage, &plan, &operation, &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .code,
            ErrorCode::VersionConflict
        );
        let result = history.cancel_cleanup(&storage, &operation).unwrap();
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.removed.len(), 1);
        assert!(history.pending_cleanups().unwrap().is_empty());
        assert_eq!(
            history.cancel_cleanup(&storage, &operation).unwrap(),
            result
        );
        let plan = history.preview_cleanup(&storage).unwrap().current;
        assert_eq!(plan.candidates.len(), 1);
        let complete = history
            .apply_cleanup(&storage, &plan, &op(), &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(complete.removed.len(), 1);
    }

    #[test]
    fn cancellation_preserves_remaining_history_and_replays_the_partial_result() {
        let (_dir, storage, history) = cleanup_fixture();
        let plan = history.preview_cleanup(&storage).unwrap().current;
        let cancel = AtomicBool::new(false);
        let operation = op();
        let result = history
            .apply_cleanup(&storage, &plan, &operation, &cancel, |progress| {
                if progress.removed == 1 {
                    cancel.store(true, Ordering::SeqCst);
                }
            })
            .unwrap();
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.removed.len(), 1);
        assert_eq!(
            history
                .preview_cleanup(&storage)
                .unwrap()
                .current
                .candidates
                .len(),
            1
        );
        assert_eq!(
            history
                .apply_cleanup(&storage, &plan, &operation, &AtomicBool::new(false), |_| {})
                .unwrap(),
            result
        );
    }
}
