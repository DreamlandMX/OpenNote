use super::*;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotOverwritePlan {
    pub operation_id: String,
    pub source: PathBuf,
    pub restore: SnapshotRestorePlan,
    pub safety_snapshot: Snapshot,
    pub previous_root: PathBuf,
    pub note_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotOverwriteResult {
    pub operation_id: String,
    pub snapshot_id: String,
    pub manifest_hash: String,
    pub target: PathBuf,
    pub previous_root: PathBuf,
    pub safety_snapshot_id: String,
    pub note_count: u64,
    pub completed_at: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    format_version: u32,
    operation_id: String,
    source: PathBuf,
    destination: PathBuf,
    vault_id: String,
    plan: SnapshotOverwritePlan,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StageReceipt {
    request_hash: String,
    restore: SnapshotRestoreResult,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    request_hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    request_hash: String,
    result: SnapshotOverwriteResult,
}

fn interrupted() -> ServiceError {
    error(ErrorCode::IoDenied, "error.overwriteInterrupted")
}

fn changed() -> ServiceError {
    error(ErrorCode::VersionConflict, "error.overwriteChanged")
}

fn rollback_required() -> ServiceError {
    error(ErrorCode::IoDenied, "error.overwriteRollbackRequired")
}

fn upgrade_required() -> ServiceError {
    error(
        ErrorCode::UnsupportedSchema,
        "error.overwriteUpgradeRequired",
    )
}

fn read<T: Serialize + DeserializeOwned>(root: &Path, path: &Path) -> Result<T, ServiceError> {
    safe_io::check(root, path)?;
    if fs::metadata(path).map_err(safe_io::io)?.len() > 1024 * 1024 {
        return Err(interrupted());
    }
    restore::read_record(root, path).map_err(|_| interrupted())
}

fn exists(path: &Path) -> Result<bool, ServiceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(failure) => Err(safe_io::io(failure)),
    }
}

fn marker_bytes(request_hash: &str) -> Result<Vec<u8>, ServiceError> {
    restore::record_bytes(&Marker {
        request_hash: request_hash.into(),
    })
}

impl BackupStore {
    fn overwrite_journal(&self, operation: &str) -> Result<PathBuf, ServiceError> {
        self.directory(operation)?;
        Ok(self
            .destination
            .join(format!(".overwrite-journal-{operation}")))
    }

    fn overwrite_paths(
        &self,
        operation: &str,
    ) -> Result<(PathBuf, PathBuf, PathBuf), ServiceError> {
        self.directory(operation)?;
        let parent = self
            .source
            .parent()
            .ok_or_else(|| error(ErrorCode::InvalidInput, "error.overwriteChanged"))?;
        Ok((
            parent.join(format!(".opennote-overwrite-new-{operation}")),
            parent.join(format!(".opennote-overwrite-old-{operation}")),
            parent.join(format!(".opennote-restore-{operation}")),
        ))
    }

    fn validate_request_shape(
        &self,
        operation: &str,
        request: &Request,
    ) -> Result<(), ServiceError> {
        let parent = request.source.parent().ok_or_else(|| interrupted())?;
        let plan = &request.plan;
        if !request.source.is_absolute()
            || request.source.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || request.format_version != 1
            || request.operation_id != operation
            || request.destination != self.destination
            || !canonical_uuid(&request.vault_id)
            || plan.operation_id != operation
            || plan.source != request.source
            || plan.previous_root != parent.join(format!(".opennote-overwrite-old-{operation}"))
            || plan.restore.target != parent.join(format!(".opennote-overwrite-new-{operation}"))
            || plan.note_count != plan.restore.note_count
            || plan.restore.snapshot.vault_id != request.vault_id
            || plan.safety_snapshot.vault_id != request.vault_id
            || plan.restore.snapshot.schema_version != 3
            || plan.safety_snapshot.schema_version != 3
            || plan.restore.snapshot.completion_state != "complete"
            || plan.safety_snapshot.completion_state != "complete"
            || !canonical_uuid(&plan.restore.snapshot.snapshot_id)
            || !canonical_uuid(&plan.safety_snapshot.snapshot_id)
            || plan.restore.snapshot.snapshot_id == plan.safety_snapshot.snapshot_id
            || !valid_hash(&plan.restore.snapshot.manifest_hash)
            || !valid_hash(&plan.safety_snapshot.manifest_hash)
            || plan.restore.plan_id
                != hash(
                    &serde_json::to_vec(&(
                        &plan.restore.snapshot.manifest_hash,
                        &plan.restore.target,
                    ))
                    .map_err(|_| interrupted())?,
                )
        {
            return Err(interrupted());
        }
        Ok(())
    }

    fn ensure_overwrite_name_available(&self, operation: &str) -> Result<(), ServiceError> {
        let (stage, previous, restore_stage) = self.overwrite_paths(operation)?;
        for path in [
            self.overwrite_journal(operation)?,
            stage,
            previous,
            restore_stage,
            self.destination.join(operation),
            self.destination
                .join(format!(".snapshot-request-{operation}.json")),
            self.destination.join(format!(".staging-{operation}")),
            self.destination
                .join(format!(".restore-journal-{operation}")),
            self.destination
                .join(format!(".cleanup-journal-{operation}")),
            self.destination
                .join(format!(".upgrade-journal-{operation}")),
            self.destination
                .join(format!(".snapshot-removed-{operation}.json")),
            self.destination
                .join(format!(".snapshot-unchanged-{operation}.json")),
        ] {
            if exists(&path)? {
                return Err(error(ErrorCode::PathCollision, "error.overwriteChanged"));
            }
        }
        Ok(())
    }

    pub(super) fn not_overwrite_operation(&self, operation: &str) -> Result<(), ServiceError> {
        self.directory(operation)?;
        if exists(&self.overwrite_journal(operation)?)? {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        Ok(())
    }

    pub(super) fn overwrite_owns_restore_target(
        &self,
        operation: &str,
        target: &Path,
    ) -> Result<bool, ServiceError> {
        let journal = self.overwrite_journal(operation)?;
        if !exists(&journal.join("request.json"))? {
            return Ok(false);
        }
        let (_, request, _) = self.overwrite_request(operation)?;
        Ok(request.plan.restore.target == target)
    }

    fn overwrite_request(
        &self,
        operation: &str,
    ) -> Result<(PathBuf, Request, String), ServiceError> {
        let journal = self.overwrite_journal(operation)?;
        let request: Request = read(&self.destination, &journal.join("request.json"))?;
        self.validate_request_shape(operation, &request)?;
        if request.source != self.source
            || request.vault_id != self.vault_id
            || request.plan.source != self.source
            || request.plan.restore.snapshot.vault_id != self.vault_id
            || request.plan.safety_snapshot.vault_id != self.vault_id
        {
            return Err(interrupted());
        }
        let request_hash = hash(&serde_json::to_vec(&request).map_err(|_| interrupted())?);
        Ok((journal, request, request_hash))
    }

    fn validate_receipt_shape(
        &self,
        request: &Request,
        request_hash: &str,
        receipt: &Receipt,
    ) -> Result<(), ServiceError> {
        let result = &receipt.result;
        if receipt.request_hash != request_hash
            || result.operation_id != request.operation_id
            || result.snapshot_id != request.plan.restore.snapshot.snapshot_id
            || result.manifest_hash != request.plan.restore.snapshot.manifest_hash
            || result.target != request.source
            || result.previous_root != request.plan.previous_root
            || result.safety_snapshot_id != request.plan.safety_snapshot.snapshot_id
            || result.note_count != request.plan.note_count
            || chrono::DateTime::parse_from_rfc3339(&result.completed_at).is_err()
        {
            return Err(interrupted());
        }
        Ok(())
    }

    fn overwrite_receipt(
        &self,
        journal: &Path,
        request: &Request,
        request_hash: &str,
    ) -> Result<Receipt, ServiceError> {
        let receipt: Receipt = read(&self.destination, &journal.join("published.json"))?;
        self.validate_receipt_shape(request, request_hash, &receipt)?;
        if request.source != self.source || request.vault_id != self.vault_id {
            return Err(interrupted());
        }
        Ok(receipt)
    }

    pub(super) fn overwrite_dependencies(
        &self,
        cancel: &AtomicBool,
    ) -> Result<HashSet<String>, ServiceError> {
        let mut protected = HashSet::new();
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            cancelled(cancel)?;
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(operation) = name
                .to_str()
                .and_then(|name| name.strip_prefix(".overwrite-journal-"))
                .filter(|id| canonical_uuid(id))
            else {
                continue;
            };
            let journal = entry.path();
            safe_io::check(&self.destination, &journal)?;
            if !fs::metadata(&journal).map_err(safe_io::io)?.is_dir() {
                return Err(interrupted());
            }
            let request: Request = read(&self.destination, &journal.join("request.json"))?;
            self.validate_request_shape(operation, &request)?;
            let request_hash = hash(&serde_json::to_vec(&request).map_err(|_| interrupted())?);
            if exists(&journal.join("published.json"))? {
                let receipt: Receipt = read(&self.destination, &journal.join("published.json"))?;
                self.validate_receipt_shape(&request, &request_hash, &receipt)?;
                continue;
            }
            if request.source == self.source && request.vault_id == self.vault_id {
                protected.insert(request.plan.restore.snapshot.snapshot_id.clone());
                protected.insert(request.plan.safety_snapshot.snapshot_id.clone());
            }
        }
        Ok(protected)
    }

    fn read_marker(
        &self,
        journal: &Path,
        name: &str,
        request_hash: &str,
    ) -> Result<bool, ServiceError> {
        let path = journal.join(name);
        if !exists(&path)? {
            return Ok(false);
        }
        let marker: Marker = read(&self.destination, &path)?;
        if marker.request_hash != request_hash {
            return Err(interrupted());
        }
        Ok(true)
    }

    fn publish_marker(
        &self,
        journal: &Path,
        name: &str,
        request_hash: &str,
    ) -> Result<(), ServiceError> {
        safe_io::publish(
            &self.destination,
            &journal.join(name),
            &marker_bytes(request_hash)?,
        )
    }

    fn stage_receipt(
        &self,
        journal: &Path,
        request: &Request,
        request_hash: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<SnapshotRestoreResult>, ServiceError> {
        let path = journal.join("staged.json");
        if !exists(&path)? {
            return Ok(None);
        }
        let receipt: StageReceipt = read(&self.destination, &path)?;
        if receipt.request_hash != request_hash
            || receipt.restore.operation_id != request.operation_id
            || receipt.restore.snapshot_id != request.plan.restore.snapshot.snapshot_id
            || receipt.restore.manifest_hash != request.plan.restore.snapshot.manifest_hash
            || receipt.restore.target != request.plan.restore.target
        {
            return Err(interrupted());
        }
        let verified =
            self.verify_restored_root(&request.plan.restore.target, &request.operation_id, cancel)?;
        if verified != receipt.restore {
            return Err(interrupted());
        }
        Ok(Some(receipt.restore))
    }

    fn verified_snapshot(
        &self,
        id: &str,
        schema: u32,
        cancel: &AtomicBool,
    ) -> Result<Snapshot, ServiceError> {
        self.not_removed(id)?;
        let snapshot = self.verify_directory(&self.directory(id)?, id, cancel)?;
        if snapshot.schema_version != schema || !self.snapshot_from_source(id)? {
            return Err(changed());
        }
        Ok(snapshot)
    }

    fn verify_storage_against_snapshot(
        &self,
        storage: &VaultStorage,
        expected_root: &Path,
        expected: &Snapshot,
        cancel: &AtomicBool,
    ) -> Result<(), ServiceError> {
        let directory = self.directory(&expected.snapshot_id)?;
        let envelope = self.envelope(&directory, &expected.snapshot_id)?;
        if Self::summary(&envelope)? != *expected {
            return Err(interrupted());
        }
        self.verify_directory(&directory, &expected.snapshot_id, cancel)?;
        let stage = self
            .destination
            .join(format!(".overwrite-verify-{}", uuid::Uuid::new_v4()));
        safe_io::check(&self.destination, &self.destination)?;
        fs::create_dir(&stage).map_err(safe_io::io)?;
        let state_file = stage.join("state.sqlite");
        let result = storage.with_snapshot_state(&state_file, |root, vault, notes| {
            cancelled(cancel)?;
            if fs::canonicalize(root).map_err(safe_io::io)?
                != fs::canonicalize(expected_root).map_err(safe_io::io)?
                || vault.vault_id != self.vault_id
                || vault.schema_version != expected.schema_version
                || !vault.writable
            {
                return Err(changed());
            }
            let mut expected_notes = envelope.manifest.notes.clone();
            let mut current_notes = notes.to_vec();
            expected_notes.sort_by(|a, b| a.note_id.cmp(&b.note_id));
            current_notes.sort_by(|a, b| a.note_id.cmp(&b.note_id));
            let mut paths = inventory(root, cancel)?;
            paths.push(".opennote/state.sqlite".into());
            paths.sort();
            let expected_paths = envelope
                .manifest
                .files
                .iter()
                .map(|file| file.relative_path.clone())
                .collect::<Vec<_>>();
            if expected_notes != current_notes || paths != expected_paths {
                return Err(changed());
            }
            for file in &envelope.manifest.files {
                let actual = if file.relative_path == ".opennote/state.sqlite" {
                    digest_file(&stage, &state_file, cancel)?
                } else {
                    digest_file(root, &root.join(&file.relative_path), cancel)?
                };
                if actual != (file.content_hash.clone(), file.byte_length) {
                    return Err(changed());
                }
            }
            Ok(())
        });
        let cleanup = (|| {
            if exists(&state_file)? {
                safe_io::check(&stage, &state_file)?;
                fs::remove_file(&state_file).map_err(safe_io::io)?;
            }
            safe_io::check(&self.destination, &stage)?;
            fs::remove_dir(&stage).map_err(safe_io::io)
        })();
        if let Err(failure) = result {
            if matches!(
                failure.code,
                ErrorCode::OperationCancelled | ErrorCode::DiskFull | ErrorCode::FileLocked
            ) {
                return Err(failure);
            }
            return Err(changed());
        }
        cleanup.map_err(|_| interrupted())
    }

    fn verify_storage_root_against_snapshot(
        &self,
        root: &Path,
        expected: &Snapshot,
        cancel: &AtomicBool,
    ) -> Result<(), ServiceError> {
        let storage = VaultStorage::open(root).map_err(|_| changed())?;
        if fs::canonicalize(storage.root_path()).map_err(safe_io::io)?
            != fs::canonicalize(root).map_err(safe_io::io)?
        {
            return Err(changed());
        }
        self.verify_storage_against_snapshot(&storage, root, expected, cancel)
    }

    pub fn plan_overwrite(
        &self,
        storage: &VaultStorage,
        snapshot_id: &str,
        operation_id: &str,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<SnapshotOverwritePlan, ServiceError> {
        self.directory(operation_id)?;
        if fs::canonicalize(storage.root_path()).map_err(safe_io::io)? != self.source
            || storage.vault().vault_id != self.vault_id
        {
            return Err(error(ErrorCode::InvalidInput, "error.vaultSessionChanged"));
        }
        if storage.vault().schema_version != 3 {
            return Err(upgrade_required());
        }
        if !storage.vault().writable {
            return Err(error(ErrorCode::InvalidInput, "error.vaultReadOnly"));
        }
        self.ensure_overwrite_name_available(operation_id)?;
        let selected = self.verified_snapshot(snapshot_id, 3, cancel)?;
        let safety_operation = uuid::Uuid::new_v4().to_string();
        let safety_snapshot = self.create(storage, &safety_operation, cancel, &mut progress)?;
        if safety_snapshot.schema_version != 3 {
            return Err(upgrade_required());
        }
        let _destination_guard = self.lock_operation("destination")?;
        self.ensure_overwrite_name_available(operation_id)?;
        let selected_now = self.verified_snapshot(snapshot_id, 3, cancel)?;
        if selected != selected_now {
            return Err(changed());
        }
        let safety_now = self.verified_snapshot(&safety_snapshot.snapshot_id, 3, cancel)?;
        if safety_now != safety_snapshot {
            return Err(interrupted());
        }
        let (stage, previous_root, _) = self.overwrite_paths(operation_id)?;
        let restore = self.plan_restore_inner(snapshot_id, &stage, cancel, Some(&stage))?;
        if restore.snapshot != selected {
            return Err(changed());
        }
        let note_count = restore.note_count;
        self.verify_storage_against_snapshot(storage, &self.source, &safety_snapshot, cancel)?;
        Ok(SnapshotOverwritePlan {
            operation_id: operation_id.into(),
            source: self.source.clone(),
            restore,
            safety_snapshot,
            previous_root,
            note_count,
        })
    }

    pub fn stage_overwrite(
        &self,
        storage: &VaultStorage,
        plan: &SnapshotOverwritePlan,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<SnapshotRestoreResult, ServiceError> {
        self.directory(&plan.operation_id)?;
        if plan.source != self.source
            || fs::canonicalize(storage.root_path()).map_err(safe_io::io)? != self.source
            || storage.vault().vault_id != self.vault_id
        {
            return Err(error(ErrorCode::InvalidInput, "error.vaultSessionChanged"));
        }
        if storage.vault().schema_version != 3 || !storage.vault().writable {
            return Err(upgrade_required());
        }
        let (journal, request, request_hash) = {
            let _destination_guard = self.lock_operation("destination")?;
            let journal = self.overwrite_journal(&plan.operation_id)?;
            if !exists(&journal.join("request.json"))? {
                self.ensure_overwrite_name_available(&plan.operation_id)?;
                let selected =
                    self.verified_snapshot(&plan.restore.snapshot.snapshot_id, 3, cancel)?;
                let safety =
                    self.verified_snapshot(&plan.safety_snapshot.snapshot_id, 3, cancel)?;
                if selected != plan.restore.snapshot
                    || safety != plan.safety_snapshot
                    || plan.source != self.source
                    || plan.note_count != plan.restore.note_count
                {
                    return Err(changed());
                }
                self.verify_storage_against_snapshot(storage, &self.source, &safety, cancel)?;
                let request = Request {
                    format_version: 1,
                    operation_id: plan.operation_id.clone(),
                    source: self.source.clone(),
                    destination: self.destination.clone(),
                    vault_id: self.vault_id.clone(),
                    plan: plan.clone(),
                };
                let request_hash = hash(&serde_json::to_vec(&request).map_err(|_| interrupted())?);
                safe_io::ensure_dir(&self.destination, &journal)?;
                safe_io::publish(
                    &self.destination,
                    &journal.join("request.json"),
                    &restore::record_bytes(&request)?,
                )?;
                (journal, request, request_hash)
            } else {
                let (journal, request, request_hash) =
                    self.overwrite_request(&plan.operation_id)?;
                if request.plan != *plan {
                    return Err(error(
                        ErrorCode::IdempotencyMismatch,
                        "error.idempotencyMismatch",
                    ));
                }
                (journal, request, request_hash)
            }
        };
        if exists(&journal.join("published.json"))? {
            if let Some(receipt) = self.stage_receipt(&journal, &request, &request_hash, cancel)? {
                return Ok(receipt);
            }
            return self.verify_restored_root(&self.source, &request.operation_id, cancel);
        }
        let selected =
            self.verified_snapshot(&request.plan.restore.snapshot.snapshot_id, 3, cancel)?;
        let safety =
            self.verified_snapshot(&request.plan.safety_snapshot.snapshot_id, 3, cancel)?;
        if selected != request.plan.restore.snapshot || safety != request.plan.safety_snapshot {
            return Err(changed());
        }
        self.verify_storage_against_snapshot(storage, &self.source, &safety, cancel)?;
        if let Some(receipt) = self.stage_receipt(&journal, &request, &request_hash, cancel)? {
            self.verify_storage_against_snapshot(storage, &self.source, &safety, cancel)?;
            return Ok(receipt);
        }
        let restore_result = self.apply_restore_inner(
            &request.plan.restore,
            &request.operation_id,
            cancel,
            &mut progress,
            true,
        )?;
        let verified =
            self.verify_restored_root(&request.plan.restore.target, &request.operation_id, cancel)?;
        if verified != restore_result {
            return Err(interrupted());
        }
        self.verify_storage_against_snapshot(storage, &self.source, &safety, cancel)?;
        safe_io::publish(
            &self.destination,
            &journal.join("staged.json"),
            &restore::record_bytes(&StageReceipt {
                request_hash: request_hash.clone(),
                restore: restore_result.clone(),
            })?,
        )?;
        Ok(restore_result)
    }

    fn overwrite_result(&self, plan: &SnapshotOverwritePlan) -> SnapshotOverwriteResult {
        SnapshotOverwriteResult {
            operation_id: plan.operation_id.clone(),
            snapshot_id: plan.restore.snapshot.snapshot_id.clone(),
            manifest_hash: plan.restore.snapshot.manifest_hash.clone(),
            target: self.source.clone(),
            previous_root: plan.previous_root.clone(),
            safety_snapshot_id: plan.safety_snapshot.snapshot_id.clone(),
            note_count: plan.note_count,
            completed_at: Utc::now().to_rfc3339(),
        }
    }

    fn publish_receipt(
        &self,
        journal: &Path,
        request_hash: &str,
        result: SnapshotOverwriteResult,
    ) -> Result<SnapshotOverwriteResult, ServiceError> {
        safe_io::publish(
            &self.destination,
            &journal.join("published.json"),
            &restore::record_bytes(&Receipt {
                request_hash: request_hash.into(),
                result: result.clone(),
            })?,
        )?;
        Ok(result)
    }

    fn rollback_previous(&self, source: &Path, previous: &Path) -> Result<(), ServiceError> {
        if exists(source)? {
            return Err(rollback_required());
        }
        fs::rename(previous, source).map_err(|_| rollback_required())
    }

    fn rollback_swap(
        &self,
        source: &Path,
        stage: &Path,
        previous: &Path,
    ) -> Result<(), ServiceError> {
        if !exists(source)? || !exists(previous)? || exists(stage)? {
            return Err(rollback_required());
        }
        fs::rename(source, stage).map_err(|_| rollback_required())?;
        fs::rename(previous, source).map_err(|_| rollback_required())?;
        Ok(())
    }

    pub fn commit_overwrite(
        &self,
        plan: &SnapshotOverwritePlan,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<SnapshotOverwriteResult, ServiceError> {
        #[cfg(not(windows))]
        return Err(error(ErrorCode::NotImplemented, "error.overwritePlatform"));
        #[cfg(windows)]
        {
            let _destination_guard = self.lock_operation("destination")?;
            let no_cancel = AtomicBool::new(false);
            let (journal, request, request_hash) = self.overwrite_request(&plan.operation_id)?;
            if request.plan != *plan {
                return Err(error(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                ));
            }
            if exists(&journal.join("published.json"))? {
                return Ok(self
                    .overwrite_receipt(&journal, &request, &request_hash)?
                    .result);
            }
            let stage = &request.plan.restore.target;
            let previous = &request.plan.previous_root;
            let source = &self.source;
            let source_exists = exists(source)?;
            let stage_exists = exists(stage)?;
            let previous_exists = exists(previous)?;
            let source_moved = self.read_marker(&journal, "source-moved", &request_hash)?;
            let swapped = self.read_marker(&journal, "swapped", &request_hash)?;
            if swapped {
                if !source_exists || !previous_exists || stage_exists {
                    return Err(interrupted());
                }
                self.verify_restored_root(source, &request.operation_id, &no_cancel)?;
                self.verify_storage_root_against_snapshot(
                    previous,
                    &request.plan.safety_snapshot,
                    &no_cancel,
                )?;
                return self.publish_receipt(
                    &journal,
                    &request_hash,
                    self.overwrite_result(&request.plan),
                );
            }
            if source_moved {
                if source_exists && previous_exists && !stage_exists {
                    self.verify_restored_root(source, &request.operation_id, &no_cancel)?;
                    self.verify_storage_root_against_snapshot(
                        previous,
                        &request.plan.safety_snapshot,
                        &no_cancel,
                    )?;
                    self.publish_marker(&journal, "swapped", &request_hash)?;
                    return self.publish_receipt(
                        &journal,
                        &request_hash,
                        self.overwrite_result(&request.plan),
                    );
                }
                if !source_exists && previous_exists && stage_exists {
                    self.verify_storage_root_against_snapshot(
                        previous,
                        &request.plan.safety_snapshot,
                        &no_cancel,
                    )?;
                    self.verify_restored_root(stage, &request.operation_id, &no_cancel)?;
                    if fs::rename(stage, source).is_err() {
                        return match self.rollback_previous(source, previous) {
                            Ok(()) => Err(interrupted()),
                            Err(failure) => Err(failure),
                        };
                    }
                    if self
                        .publish_marker(&journal, "swapped", &request_hash)
                        .is_err()
                    {
                        return match self.rollback_swap(source, stage, previous) {
                            Ok(()) => Err(interrupted()),
                            Err(failure) => Err(failure),
                        };
                    }
                    self.verify_restored_root(source, &request.operation_id, &no_cancel)?;
                    return self.publish_receipt(
                        &journal,
                        &request_hash,
                        self.overwrite_result(&request.plan),
                    );
                }
                return Err(if source_exists {
                    interrupted()
                } else {
                    rollback_required()
                });
            }
            if source_exists && previous_exists && stage_exists {
                // The source reappeared or a colliding tree was deposited after the preview.
                return Err(interrupted());
            }
            if source_exists && previous_exists && !stage_exists {
                // Without the swapped marker, the new source cannot be attributed to this operation.
                return Err(interrupted());
            }
            if !source_exists && previous_exists && stage_exists {
                self.verify_storage_root_against_snapshot(
                    previous,
                    &request.plan.safety_snapshot,
                    &no_cancel,
                )?;
                self.verify_restored_root(stage, &request.operation_id, &no_cancel)?;
                if fs::rename(stage, source).is_err() {
                    return match self.rollback_previous(source, previous) {
                        Ok(()) => Err(interrupted()),
                        Err(failure) => Err(failure),
                    };
                }
                if self
                    .publish_marker(&journal, "swapped", &request_hash)
                    .is_err()
                {
                    return match self.rollback_swap(source, stage, previous) {
                        Ok(()) => Err(interrupted()),
                        Err(failure) => Err(failure),
                    };
                }
                self.verify_restored_root(source, &request.operation_id, &no_cancel)?;
                return self.publish_receipt(
                    &journal,
                    &request_hash,
                    self.overwrite_result(&request.plan),
                );
            }
            if source_exists && !previous_exists && stage_exists {
                progress(SnapshotProgress {
                    phase: "verifying".into(),
                    completed_objects: 0,
                    total_objects: 0,
                });
                self.stage_receipt(&journal, &request, &request_hash, cancel)?
                    .ok_or_else(interrupted)?;
                self.verify_storage_root_against_snapshot(
                    source,
                    &request.plan.safety_snapshot,
                    cancel,
                )?;
                self.verify_restored_root(stage, &request.operation_id, cancel)?;
                cancelled(cancel)?;
                progress(SnapshotProgress {
                    phase: "movingPrevious".into(),
                    completed_objects: 0,
                    total_objects: 0,
                });
                fs::rename(source, previous).map_err(safe_io::io)?;
                if self
                    .publish_marker(&journal, "source-moved", &request_hash)
                    .is_err()
                {
                    return match self.rollback_previous(source, previous) {
                        Ok(()) => Err(interrupted()),
                        Err(failure) => Err(failure),
                    };
                }
                if self
                    .verify_storage_root_against_snapshot(
                        previous,
                        &request.plan.safety_snapshot,
                        &no_cancel,
                    )
                    .is_err()
                {
                    return match self.rollback_previous(source, previous) {
                        Ok(()) => Err(interrupted()),
                        Err(failure) => Err(failure),
                    };
                }
                progress(SnapshotProgress {
                    phase: "publishing".into(),
                    completed_objects: 0,
                    total_objects: 0,
                });
                if fs::rename(stage, source).is_err() {
                    return match self.rollback_previous(source, previous) {
                        Ok(()) => Err(interrupted()),
                        Err(failure) => Err(failure),
                    };
                }
                if self
                    .publish_marker(&journal, "swapped", &request_hash)
                    .is_err()
                {
                    return match self.rollback_swap(source, stage, previous) {
                        Ok(()) => Err(interrupted()),
                        Err(failure) => Err(failure),
                    };
                }
                self.verify_restored_root(source, &request.operation_id, &no_cancel)
                    .map_err(|_| interrupted())?;
                return self.publish_receipt(
                    &journal,
                    &request_hash,
                    self.overwrite_result(&request.plan),
                );
            }
            Err(if source_exists {
                interrupted()
            } else {
                rollback_required()
            })
        }
    }
}
