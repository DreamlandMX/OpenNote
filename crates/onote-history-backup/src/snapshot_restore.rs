use super::*;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotRestoreResult {
    pub operation_id: String,
    pub snapshot_id: String,
    pub manifest_hash: String,
    pub target: PathBuf,
    pub note_count: u64,
    pub completed_at: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotRestoreCancellation {
    Cancelled,
    AlreadyApplied,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    operation_id: String,
    plan: SnapshotRestorePlan,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    request_hash: String,
    result: SnapshotRestoreResult,
    state_files: Vec<SnapshotFile>,
    vault_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record<T> {
    value: T,
    checksum: String,
}
fn interrupted() -> ServiceError {
    error(ErrorCode::RestoreInterrupted, "error.restoreInterrupted")
}
pub(super) fn record_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ServiceError> {
    let checksum = hash(&serde_json::to_vec(value).map_err(|_| interrupted())?);
    serde_json::to_vec(&Record { value, checksum }).map_err(|_| interrupted())
}
pub(super) fn read_record<T: Serialize + DeserializeOwned>(
    root: &Path,
    path: &Path,
) -> Result<T, ServiceError> {
    let record: Record<T> =
        serde_json::from_slice(&safe_io::read(root, path).map_err(|_| interrupted())?)
            .map_err(|_| interrupted())?;
    if hash(&serde_json::to_vec(&record.value).map_err(|_| interrupted())?) != record.checksum {
        return Err(interrupted());
    }
    Ok(record.value)
}

impl BackupStore {
    pub(super) fn verify_restored_root(
        &self,
        root: &Path,
        operation: &str,
        cancel: &AtomicBool,
    ) -> Result<SnapshotRestoreResult, ServiceError> {
        let (_, request, request_hash) = self.restore_request(operation)?;
        let canonical_root = fs::canonicalize(root).map_err(|_| interrupted())?;
        if canonical_root != request.plan.target && canonical_root != self.source {
            return Err(interrupted());
        }
        let receipt = self.receipt(
            root,
            &root.join(".opennote/restore-receipt.json"),
            &request,
            &request_hash,
        )?;
        let directory = self.directory(&request.plan.snapshot.snapshot_id)?;
        let envelope = self.envelope(&directory, &request.plan.snapshot.snapshot_id)?;
        if Self::summary(&envelope)? != request.plan.snapshot
            || request.plan.note_count != envelope.manifest.notes.len() as u64
        {
            return Err(interrupted());
        }
        self.verify_ready_restore(root, &envelope.manifest, &receipt, cancel)?;
        Ok(receipt.result)
    }

    pub(super) fn restore_dependencies(
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
                .and_then(|name| name.strip_prefix(".restore-journal-"))
                .filter(|id| canonical_uuid(id))
            else {
                continue;
            };
            let path = entry.path().join("request.json");
            safe_io::check(&self.destination, &path)?;
            if fs::metadata(&path).map_err(safe_io::io)?.len() > 1024 * 1024 {
                return Err(interrupted());
            }
            let request: Request = read_record(&self.destination, &path)?;
            if request.operation_id != operation
                || !canonical_uuid(&request.plan.snapshot.snapshot_id)
            {
                return Err(interrupted());
            }
            if request.plan.snapshot.vault_id != self.vault_id {
                continue;
            }
            // Keep even cancelled restores: retained staging may be the user's only remaining recovery material.
            if !entry.path().join("published.json").exists() {
                protected.insert(request.plan.snapshot.snapshot_id);
            } else {
                self.restore_request(operation)?;
                self.receipt(
                    &self.destination,
                    &entry.path().join("published.json"),
                    &request,
                    &hash(&serde_json::to_vec(&request).map_err(|_| interrupted())?),
                )?;
            }
        }
        Ok(protected)
    }
    fn restore_journal(&self, operation: &str) -> Result<PathBuf, ServiceError> {
        if !canonical_uuid(operation) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidOperationId"));
        }
        Ok(self
            .destination
            .join(format!(".restore-journal-{operation}")))
    }
    fn restore_request(&self, operation: &str) -> Result<(PathBuf, Request, String), ServiceError> {
        let journal = self.restore_journal(operation)?;
        let request: Request = read_record(&self.destination, &journal.join("request.json"))?;
        if request.operation_id != operation
            || request.plan.snapshot.vault_id != self.vault_id
            || !canonical_uuid(&request.plan.snapshot.snapshot_id)
        {
            return Err(interrupted());
        }
        let parent = external_directory(request.plan.target.parent().ok_or_else(interrupted)?)?;
        let name = request
            .plan
            .target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(interrupted)?;
        let overwrite_target = if request.plan.target.starts_with(&self.source)
            || request.plan.target.starts_with(&self.destination)
        {
            self.overwrite_owns_restore_target(operation, &request.plan.target)?
        } else {
            false
        };
        if !valid_relative(name)
            || parent.join(name) != request.plan.target
            || ((request.plan.target.starts_with(&self.source)
                || request.plan.target.starts_with(&self.destination))
                && !overwrite_target)
            || hash(
                &serde_json::to_vec(&(&request.plan.snapshot.manifest_hash, &request.plan.target))
                    .map_err(|_| interrupted())?,
            ) != request.plan.plan_id
        {
            return Err(interrupted());
        }
        let request_hash = hash(&serde_json::to_vec(&request).map_err(|_| interrupted())?);
        Ok((journal, request, request_hash))
    }
    fn receipt(
        &self,
        root: &Path,
        path: &Path,
        request: &Request,
        request_hash: &str,
    ) -> Result<Receipt, ServiceError> {
        let receipt: Receipt = read_record(root, path)?;
        let result = &receipt.result;
        if receipt.request_hash != request_hash
            || result.operation_id != request.operation_id
            || result.snapshot_id != request.plan.snapshot.snapshot_id
            || result.manifest_hash != request.plan.snapshot.manifest_hash
            || result.target != request.plan.target
            || result.note_count != request.plan.note_count
            || chrono::DateTime::parse_from_rfc3339(&result.completed_at).is_err()
            || !valid_hash(&receipt.vault_hash)
        {
            return Err(interrupted());
        }
        Ok(receipt)
    }
    fn finished_restore(
        &self,
        journal: &Path,
        request: &Request,
        request_hash: &str,
    ) -> Result<Option<SnapshotRestoreResult>, ServiceError> {
        if fs::symlink_metadata(journal.join("published.json")).is_ok() {
            return Ok(Some(
                self.receipt(
                    &self.destination,
                    &journal.join("published.json"),
                    request,
                    request_hash,
                )?
                .result,
            ));
        }
        let target = &request.plan.target;
        match fs::symlink_metadata(target) {
            Ok(_) => {
                let receipt = self
                    .receipt(
                        target,
                        &target.join(".opennote/restore-receipt.json"),
                        request,
                        request_hash,
                    )
                    .map_err(|_| error(ErrorCode::PathCollision, "error.restoreTargetExists"))?;
                let meta: serde_json::Value = serde_json::from_slice(&safe_io::read(
                    target,
                    &target.join(".opennote/vault.json"),
                )?)
                .map_err(|_| interrupted())?;
                if meta["vaultId"] != self.vault_id {
                    return Err(error(ErrorCode::PathCollision, "error.restoreTargetExists"));
                }
                safe_io::publish(
                    &self.destination,
                    &journal.join("published.json"),
                    &record_bytes(&receipt)?,
                )?;
                Ok(Some(receipt.result))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(safe_io::io(e)),
        }
    }
    pub fn apply_restore(
        &self,
        plan: &SnapshotRestorePlan,
        operation: &str,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotProgress),
    ) -> Result<SnapshotRestoreResult, ServiceError> {
        self.apply_restore_inner(plan, operation, cancel, progress, false)
    }
    pub(super) fn apply_restore_inner(
        &self,
        plan: &SnapshotRestorePlan,
        operation: &str,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
        allow_overwrite: bool,
    ) -> Result<SnapshotRestoreResult, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        if allow_overwrite {
            self.not_cleanup_or_upgrade_operation(operation)?;
        } else {
            self.not_cleanup_operation(operation)?;
        }
        let journal = self.restore_journal(operation)?;
        let _guard = self.lock_operation(operation)?;
        if fs::symlink_metadata(&journal).is_err() {
            cancelled(cancel)?;
            let allowed = allow_overwrite.then_some(plan.target.as_path());
            if self.plan_restore_inner(&plan.snapshot.snapshot_id, &plan.target, cancel, allowed)?
                != *plan
            {
                return Err(error(
                    ErrorCode::VersionConflict,
                    "error.restorePlanChanged",
                ));
            }
            let envelope = self.envelope(
                &self.directory(&plan.snapshot.snapshot_id)?,
                &plan.snapshot.snapshot_id,
            )?;
            self.check_copy_space(
                &plan.target,
                plan.target.parent().ok_or_else(interrupted)?,
                &envelope.manifest.files,
                true,
            )?;
            fs::create_dir(&journal).map_err(safe_io::io)?;
            safe_io::publish(
                &self.destination,
                &journal.join("request.json"),
                &record_bytes(&Request {
                    operation_id: operation.into(),
                    plan: plan.clone(),
                })?,
            )?;
        }
        let (journal, request, request_hash) = self.restore_request(operation)?;
        if request.plan != *plan {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        if fs::symlink_metadata(journal.join("published.json")).is_ok() {
            return Ok(self
                .receipt(
                    &self.destination,
                    &journal.join("published.json"),
                    &request,
                    &request_hash,
                )?
                .result);
        }
        if fs::symlink_metadata(journal.join("cancelled")).is_ok() {
            if safe_io::read(&self.destination, &journal.join("cancelled"))?
                != request_hash.as_bytes()
            {
                return Err(interrupted());
            }
            return Err(error(
                ErrorCode::OperationCancelled,
                "error.operationCancelled",
            ));
        }
        if let Some(result) = self.finished_restore(&journal, &request, &request_hash)? {
            return Ok(result);
        }
        cancelled(cancel)?;
        let directory = self.directory(&plan.snapshot.snapshot_id)?;
        let envelope = self.envelope(&directory, &plan.snapshot.snapshot_id)?;
        if Self::summary(&envelope)? != plan.snapshot
            || plan.note_count != envelope.manifest.notes.len() as u64
        {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.restorePlanChanged",
            ));
        }
        self.verify_directory(&directory, &plan.snapshot.snapshot_id, cancel)?;
        let parent = plan.target.parent().ok_or_else(interrupted)?;
        let stage = parent.join(format!(".opennote-restore-{operation}"));
        match fs::create_dir(&stage) {
            Ok(()) => safe_io::publish(
                parent,
                &stage.join(".restore-owner"),
                request_hash.as_bytes(),
            )?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if safe_io::read(parent, &stage.join(".restore-owner"))
                    .map_err(|_| interrupted())?
                    != request_hash.as_bytes()
                {
                    return Err(interrupted());
                }
            }
            Err(e) => return Err(safe_io::io(e)),
        }
        let vault_root = stage.join("vault");
        if fs::symlink_metadata(journal.join("ready.json")).is_err()
            && fs::symlink_metadata(vault_root.join(".opennote/restore-receipt.json")).is_ok()
        {
            let receipt = self.receipt(
                &vault_root,
                &vault_root.join(".opennote/restore-receipt.json"),
                &request,
                &request_hash,
            )?;
            self.verify_ready_restore(&vault_root, &envelope.manifest, &receipt, cancel)?;
            safe_io::publish(
                &self.destination,
                &journal.join("ready.json"),
                &record_bytes(&receipt)?,
            )?;
        }
        if fs::symlink_metadata(journal.join("ready.json")).is_err() {
            self.check_copy_space(&vault_root, parent, &envelope.manifest.files, true)?;
            safe_io::ensure_dir(parent, &vault_root)?;
            if fs::symlink_metadata(journal.join("copied")).is_err() {
                let meta_file = envelope
                    .manifest
                    .files
                    .iter()
                    .find(|f| f.relative_path == ".opennote/vault.json")
                    .ok_or_else(interrupted)?;
                let mut meta: serde_json::Value = serde_json::from_slice(&safe_io::read(
                    &directory,
                    &directory.join("objects").join(&meta_file.content_hash),
                )?)
                .map_err(|_| interrupted())?;
                meta["syncEnabled"] = false.into();
                meta["writable"] = false.into();
                safe_io::publish(
                    &vault_root,
                    &vault_root.join(".opennote/vault.json"),
                    &serde_json::to_vec_pretty(&meta).map_err(|_| interrupted())?,
                )?;
                for (index, file) in envelope.manifest.files.iter().enumerate() {
                    progress(SnapshotProgress {
                        phase: "restoring".into(),
                        completed_objects: index as u64,
                        total_objects: envelope.manifest.files.len() as u64,
                    });
                    cancelled(cancel)?;
                    if file.relative_path == ".opennote/vault.json" {
                        continue;
                    }
                    self.copy_file(
                        &directory,
                        &directory.join("objects").join(&file.content_hash),
                        &vault_root,
                        &vault_root.join(&file.relative_path),
                        &stage,
                        file,
                        cancel,
                    )?;
                }
                safe_io::publish(
                    &self.destination,
                    &journal.join("copied"),
                    request_hash.as_bytes(),
                )?;
            } else if safe_io::read(&self.destination, &journal.join("copied"))?
                != request_hash.as_bytes()
            {
                return Err(interrupted());
            }
            progress(SnapshotProgress {
                phase: "finalizing".into(),
                completed_objects: plan.snapshot.object_count,
                total_objects: plan.snapshot.object_count,
            });
            cancelled(cancel)?;
            let mut storage = VaultStorage::open(&vault_root)?;
            let schema_one = matches!(envelope.manifest.schema_version, 1 | 2);
            if schema_one {
                let actual = storage.list()?;
                let by_id = actual
                    .iter()
                    .map(|note| (note.note_id.as_str(), note))
                    .collect::<BTreeMap<_, _>>();
                if storage.vault().schema_version != envelope.manifest.schema_version
                    || storage.vault().writable
                    || actual.len() != envelope.manifest.notes.len()
                    || envelope.manifest.notes.iter().any(|expected| {
                        by_id.get(expected.note_id.as_str()).is_none_or(|note| {
                            note.relative_path != expected.relative_path
                                || note.local_revision != expected.local_revision
                                || note.content_hash != expected.content_hash
                        })
                    })
                {
                    return Err(interrupted());
                }
            } else {
                storage.finalize_snapshot_restore(
                    &envelope.manifest.notes,
                    &plan.snapshot.manifest_hash,
                    operation,
                )?;
                let actual = storage.list()?;
                let by_id = actual
                    .iter()
                    .map(|note| (note.note_id.as_str(), note))
                    .collect::<BTreeMap<_, _>>();
                if !storage.vault().writable
                    || actual.len() != envelope.manifest.notes.len()
                    || envelope.manifest.notes.iter().any(|expected| {
                        by_id.get(expected.note_id.as_str()).is_none_or(|note| {
                            note.relative_path != expected.relative_path
                                || note.local_revision != expected.local_revision + 1
                                || note.content_hash != expected.content_hash
                        })
                    })
                {
                    return Err(interrupted());
                }
            }
            drop(storage);
            let mut state_files = Vec::new();
            for relative in [".opennote/state.sqlite", ".opennote/state.sqlite-wal"] {
                let path = vault_root.join(relative);
                if fs::symlink_metadata(&path).is_ok() {
                    let (content_hash, byte_length) = digest_file(&vault_root, &path, cancel)?;
                    state_files.push(SnapshotFile {
                        relative_path: relative.into(),
                        content_hash,
                        byte_length,
                    });
                }
            }
            let vault_hash = digest_file(
                &vault_root,
                &vault_root.join(".opennote/vault.json"),
                cancel,
            )?
            .0;
            let receipt = Receipt {
                request_hash: request_hash.clone(),
                result: SnapshotRestoreResult {
                    operation_id: operation.into(),
                    snapshot_id: plan.snapshot.snapshot_id.clone(),
                    manifest_hash: plan.snapshot.manifest_hash.clone(),
                    target: plan.target.clone(),
                    note_count: plan.note_count,
                    completed_at: Utc::now().to_rfc3339(),
                },
                state_files,
                vault_hash,
            };
            safe_io::publish(
                &vault_root,
                &vault_root.join(".opennote/restore-receipt.json"),
                &record_bytes(&receipt)?,
            )?;
            safe_io::publish(
                &self.destination,
                &journal.join("ready.json"),
                &record_bytes(&receipt)?,
            )?;
        }
        let receipt = self.receipt(
            &self.destination,
            &journal.join("ready.json"),
            &request,
            &request_hash,
        )?;
        self.verify_ready_restore(&vault_root, &envelope.manifest, &receipt, cancel)?;
        progress(SnapshotProgress {
            phase: "publishing".into(),
            completed_objects: plan.snapshot.object_count,
            total_objects: plan.snapshot.object_count,
        });
        cancelled(cancel)?;
        if let Some(result) = self.finished_restore(&journal, &request, &request_hash)? {
            return Ok(result);
        }
        safe_io::check(parent, &vault_root)?;
        // shortcut: other platforms need an atomic no-replace directory adapter before enabling restore.
        #[cfg(not(windows))]
        return Err(error(
            ErrorCode::NotImplemented,
            "error.restorePublishPlatform",
        ));
        #[cfg(windows)]
        fs::rename(&vault_root, &plan.target).map_err(safe_io::io)?;
        safe_io::publish(
            &self.destination,
            &journal.join("published.json"),
            &record_bytes(&receipt)?,
        )?;
        Ok(receipt.result)
    }
    fn verify_ready_restore(
        &self,
        root: &Path,
        manifest: &Manifest,
        receipt: &Receipt,
        cancel: &AtomicBool,
    ) -> Result<(), ServiceError> {
        let mut paths = inventory(root, cancel)?;
        paths.sort();
        let mut expected = manifest
            .files
            .iter()
            .filter(|f| f.relative_path != ".opennote/state.sqlite")
            .map(|f| f.relative_path.clone())
            .collect::<Vec<_>>();
        expected.sort();
        if paths != expected {
            return Err(interrupted());
        }
        for file in &manifest.files {
            if !matches!(
                file.relative_path.as_str(),
                ".opennote/state.sqlite" | ".opennote/vault.json"
            ) {
                self.verify_file(root, &root.join(&file.relative_path), file, cancel)
                    .map_err(|e| {
                        if e.code == ErrorCode::OperationCancelled {
                            e
                        } else {
                            interrupted()
                        }
                    })?;
            }
        }
        if digest_file(root, &root.join(".opennote/vault.json"), cancel)?.0 != receipt.vault_hash
            || !receipt
                .state_files
                .iter()
                .any(|f| f.relative_path == ".opennote/state.sqlite")
        {
            return Err(interrupted());
        }
        let mut seen = HashSet::new();
        for file in &receipt.state_files {
            if !matches!(
                file.relative_path.as_str(),
                ".opennote/state.sqlite" | ".opennote/state.sqlite-wal"
            ) || !seen.insert(&file.relative_path)
            {
                return Err(interrupted());
            }
            self.verify_file(root, &root.join(&file.relative_path), file, cancel)
                .map_err(|e| {
                    if e.code == ErrorCode::OperationCancelled {
                        e
                    } else {
                        interrupted()
                    }
                })?;
        }
        Ok(())
    }
    pub fn cancel_restore(
        &self,
        operation: &str,
    ) -> Result<SnapshotRestoreCancellation, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        self.restore_journal(operation)?;
        let _guard = self.lock_operation(operation)?;
        let (journal, request, request_hash) = self.restore_request(operation)?;
        match self.finished_restore(&journal, &request, &request_hash) {
            Ok(Some(_)) => return Ok(SnapshotRestoreCancellation::AlreadyApplied),
            Ok(None) => (),
            Err(e) if e.code == ErrorCode::PathCollision => (),
            Err(e) => return Err(e),
        }
        safe_io::publish(
            &self.destination,
            &journal.join("cancelled"),
            request_hash.as_bytes(),
        )?;
        Ok(SnapshotRestoreCancellation::Cancelled)
    }
}
