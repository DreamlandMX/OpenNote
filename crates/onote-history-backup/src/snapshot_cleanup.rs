use super::*;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotCleanupResult {
    pub operation_id: String,
    pub status: String,
    pub removed: Vec<Snapshot>,
    pub remaining: Vec<Snapshot>,
    pub quarantined: Vec<Snapshot>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSnapshotCleanup {
    pub operation_id: String,
    pub plan: SnapshotRetentionPlan,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    format_version: u32,
    operation_id: String,
    source: PathBuf,
    destination: PathBuf,
    vault_id: String,
    plan: SnapshotRetentionPlan,
    files: Vec<Vec<SnapshotFile>>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    request_hash: String,
    result: SnapshotCleanupResult,
}
fn exists(root: &Path, path: &Path) -> Result<bool, ServiceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            safe_io::check(root, path)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(safe_io::io(e)),
    }
}
fn read<T: Serialize + DeserializeOwned>(root: &Path, path: &Path) -> Result<T, ServiceError> {
    safe_io::check(root, path)?;
    if fs::metadata(path).map_err(safe_io::io)?.len() > 64 * 1024 * 1024 {
        return Err(incomplete());
    }
    restore::read_record(root, path).map_err(|_| incomplete())
}
fn bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ServiceError> {
    restore::record_bytes(value).map_err(|_| incomplete())
}
impl BackupStore {
    pub(super) fn not_cleanup_operation(&self, operation: &str) -> Result<(), ServiceError> {
        self.not_overwrite_operation(operation)?;
        self.not_cleanup_or_upgrade_operation(operation)
    }
    pub(super) fn not_cleanup_or_upgrade_operation(
        &self,
        operation: &str,
    ) -> Result<(), ServiceError> {
        self.not_upgrade_operation(operation)?;
        self.directory(operation)?;
        if exists(
            &self.destination,
            &self
                .destination
                .join(format!(".cleanup-journal-{operation}")),
        )? {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        Ok(())
    }
    pub fn pending_cleanups(&self) -> Result<Vec<PendingSnapshotCleanup>, ServiceError> {
        let mut pending = Vec::new();
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(operation) = name
                .to_str()
                .and_then(|name| name.strip_prefix(".cleanup-journal-"))
                .filter(|id| canonical_uuid(id))
            else {
                continue;
            };
            if !exists(&self.destination, &entry.path().join("request.json"))? {
                continue;
            }
            let request: Request = read(&self.destination, &entry.path().join("request.json"))?;
            if request.source != self.source || request.vault_id != self.vault_id {
                continue;
            }
            let (_, digest) = self.cleanup_request(&entry.path(), operation, &request.plan)?;
            if exists(&self.destination, &entry.path().join("result.json"))? {
                let receipt: Receipt = read(&self.destination, &entry.path().join("result.json"))?;
                if receipt.request_hash != digest
                    || receipt.result.operation_id != operation
                    || !matches!(receipt.result.status.as_str(), "completed" | "cancelled")
                {
                    return Err(incomplete());
                }
            } else {
                pending.push(PendingSnapshotCleanup {
                    operation_id: operation.into(),
                    plan: request.plan,
                });
            }
        }
        pending.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        Ok(pending)
    }
    pub fn resume_cleanup(
        &self,
        operation: &str,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotProgress),
    ) -> Result<SnapshotCleanupResult, ServiceError> {
        self.directory(operation)?;
        let journal = self
            .destination
            .join(format!(".cleanup-journal-{operation}"));
        let request: Request = read(&self.destination, &journal.join("request.json"))?;
        self.cleanup_request(&journal, operation, &request.plan)?;
        self.apply_cleanup(&request.plan, operation, cancel, progress)
    }
    pub(super) fn not_removed(&self, id: &str) -> Result<(), ServiceError> {
        self.directory(id)?;
        if exists(
            &self.destination,
            &self
                .destination
                .join(format!(".snapshot-removed-{id}.json")),
        )? {
            return Err(error(ErrorCode::ObjectNotFound, "error.backupIncomplete"));
        }
        Ok(())
    }
    pub(super) fn cleanup_files(
        &self,
        snapshot: &Snapshot,
        cancel: &AtomicBool,
    ) -> Result<Vec<SnapshotFile>, ServiceError> {
        let directory = self.directory(&snapshot.snapshot_id)?;
        let envelope = self.envelope(&directory, &snapshot.snapshot_id)?;
        if Self::summary(&envelope)? != *snapshot {
            return Err(incomplete());
        }
        let mut files = BTreeMap::new();
        for file in &envelope.manifest.files {
            files.insert(
                format!("objects/{}", file.content_hash),
                SnapshotFile {
                    relative_path: format!("objects/{}", file.content_hash),
                    content_hash: file.content_hash.clone(),
                    byte_length: file.byte_length,
                },
            );
        }
        // Refuse unknown files, including unrelated data placed in an otherwise valid snapshot.
        for entry in fs::read_dir(&directory).map_err(safe_io::io)? {
            cancelled(cancel)?;
            let entry = entry.map_err(safe_io::io)?;
            safe_io::check(&directory, &entry.path())?;
            let name = entry.file_name();
            match name.to_str() {
                Some("objects") if entry.metadata().map_err(safe_io::io)?.is_dir() => {
                    for object in fs::read_dir(entry.path()).map_err(safe_io::io)? {
                        cancelled(cancel)?;
                        let object = object.map_err(safe_io::io)?;
                        safe_io::check(&directory, &object.path())?;
                        let name = object.file_name();
                        if !object.metadata().map_err(safe_io::io)?.is_file()
                            || name
                                .to_str()
                                .is_none_or(|name| !files.contains_key(&format!("objects/{name}")))
                        {
                            return Err(incomplete());
                        }
                    }
                }
                Some("manifest.json") | Some("state.sqlite")
                    if entry.metadata().map_err(safe_io::io)?.is_file() =>
                {
                    let name = name.to_str().ok_or_else(incomplete)?;
                    let (content_hash, byte_length) =
                        digest_file(&directory, &entry.path(), cancel)?;
                    if name == "state.sqlite"
                        && !envelope.manifest.files.iter().any(|file| {
                            file.relative_path == ".opennote/state.sqlite"
                                && file.content_hash == content_hash
                                && file.byte_length == byte_length
                        })
                    {
                        return Err(incomplete());
                    }
                    files.insert(
                        name.into(),
                        SnapshotFile {
                            relative_path: name.into(),
                            content_hash,
                            byte_length,
                        },
                    );
                }
                _ => return Err(incomplete()),
            }
        }
        // Delete the manifest last; the immutable request carries the full recipe across interruption.
        let manifest = files.remove("manifest.json").ok_or_else(incomplete)?;
        let mut result: Vec<_> = files.into_values().collect();
        result.push(manifest);
        Ok(result)
    }
    fn cleanup_request(
        &self,
        journal: &Path,
        operation: &str,
        plan: &SnapshotRetentionPlan,
    ) -> Result<(Request, String), ServiceError> {
        let request: Request = read(&self.destination, &journal.join("request.json"))?;
        if request.source != self.source
            || request.destination != self.destination
            || request.vault_id != self.vault_id
            || request.operation_id != operation
            || request.plan != *plan
        {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        let mut unsigned = plan.clone();
        unsigned.plan_id.clear();
        let expected = hash(
            &serde_json::to_vec(&(&self.source, &self.destination, &self.vault_id, &unsigned))
                .map_err(|_| incomplete())?,
        );
        let mut ids = HashSet::new();
        if request.format_version != 1
            || expected != plan.plan_id
            || plan.retained_snapshots == 0
            || (plan.retained.is_empty() && !plan.candidates.is_empty())
            || request.files.len() != plan.candidates.len()
        {
            return Err(incomplete());
        }
        for snapshot in plan.retained.iter().chain(&plan.candidates) {
            if !canonical_uuid(&snapshot.snapshot_id)
                || snapshot.vault_id != self.vault_id
                || !valid_hash(&snapshot.manifest_hash)
                || !ids.insert(&snapshot.snapshot_id)
            {
                return Err(incomplete());
            }
        }
        let order: BTreeMap<_, _> = self
            .publication_history()?
            .into_iter()
            .enumerate()
            .map(|(index, snapshot)| (snapshot.snapshot_id, index))
            .collect();
        if plan
            .retained
            .windows(2)
            .any(|pair| retention::snapshot_order(&pair[0], &pair[1], &order).is_gt())
            || plan.retained.last().is_some_and(|last| {
                plan.candidates
                    .iter()
                    .any(|candidate| retention::snapshot_order(last, candidate, &order).is_gt())
            })
        {
            return Err(incomplete());
        }
        for files in &request.files {
            let mut paths = HashSet::new();
            for file in files {
                if !valid_hash(&file.content_hash)
                    || !paths.insert(file.relative_path.as_str())
                    || !(matches!(
                        file.relative_path.as_str(),
                        "manifest.json" | "state.sqlite"
                    ) || file
                        .relative_path
                        .strip_prefix("objects/")
                        .is_some_and(|name| valid_hash(name) && name == file.content_hash))
                {
                    return Err(incomplete());
                }
            }
            if !paths.contains("manifest.json") {
                return Err(incomplete());
            }
        }
        let digest = hash(&serde_json::to_vec(&request).map_err(|_| incomplete())?);
        Ok((request, digest))
    }
    fn other_cleanup_pending(&self, operation: &str) -> Result<(), ServiceError> {
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|name| name.strip_prefix(".cleanup-journal-"))
                .filter(|id| canonical_uuid(id) && *id != operation)
            else {
                continue;
            };
            if !exists(&self.destination, &entry.path().join("request.json"))? {
                continue;
            }
            let request: Request = read(&self.destination, &entry.path().join("request.json"))?;
            if request.operation_id != id {
                return Err(incomplete());
            }
            if !exists(&self.destination, &entry.path().join("result.json"))? {
                return Err(error(ErrorCode::FileLocked, "error.backupBusy"));
            }
            let receipt: Receipt = read(&self.destination, &entry.path().join("result.json"))?;
            if receipt.request_hash
                != hash(&serde_json::to_vec(&request).map_err(|_| incomplete())?)
            {
                return Err(incomplete());
            }
        }
        Ok(())
    }
    pub fn apply_cleanup(
        &self,
        plan: &SnapshotRetentionPlan,
        operation: &str,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<SnapshotCleanupResult, ServiceError> {
        if !canonical_uuid(operation) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidOperationId"));
        }
        let _destination_guard = self.lock_operation("destination")?;
        let journal = self
            .destination
            .join(format!(".cleanup-journal-{operation}"));
        if !exists(&self.destination, &journal.join("request.json"))? {
            cancelled(cancel)?;
            self.other_cleanup_pending(operation)?;
            if self.plan_retention(plan.retained_snapshots, cancel, &mut progress)? != *plan {
                return Err(error(
                    ErrorCode::VersionConflict,
                    "error.restorePlanChanged",
                ));
            }
            for path in [
                self.directory(operation)?,
                self.destination
                    .join(format!(".snapshot-request-{operation}.json")),
                self.destination.join(format!(".staging-{operation}")),
                self.destination
                    .join(format!(".restore-journal-{operation}")),
                self.destination
                    .join(format!(".upgrade-journal-{operation}")),
                self.destination
                    .join(format!(".overwrite-journal-{operation}")),
                self.destination
                    .join(format!(".snapshot-removed-{operation}.json")),
            ] {
                if exists(&self.destination, &path)? {
                    return Err(error(
                        ErrorCode::IdempotencyMismatch,
                        "error.idempotencyMismatch",
                    ));
                }
            }
            let files = plan
                .candidates
                .iter()
                .map(|snapshot| self.cleanup_files(snapshot, cancel))
                .collect::<Result<Vec<_>, _>>()?;
            cancelled(cancel)?;
            let request = Request {
                format_version: 1,
                operation_id: operation.into(),
                source: self.source.clone(),
                destination: self.destination.clone(),
                vault_id: self.vault_id.clone(),
                plan: plan.clone(),
                files,
            };
            safe_io::publish(
                &self.destination,
                &journal.join("request.json"),
                &bytes(&request)?,
            )?;
        }
        let (request, request_hash) = self.cleanup_request(&journal, operation, plan)?;
        if exists(&self.destination, &journal.join("result.json"))? {
            let receipt: Receipt = read(&self.destination, &journal.join("result.json"))?;
            let result = receipt.result;
            let mut all = result.removed.clone();
            all.extend(result.remaining.clone());
            all.extend(result.quarantined.clone());
            all.sort_by(|a, b| a.snapshot_id.cmp(&b.snapshot_id));
            let mut expected = plan.candidates.clone();
            expected.sort_by(|a, b| a.snapshot_id.cmp(&b.snapshot_id));
            if receipt.request_hash != request_hash
                || result.operation_id != operation
                || !matches!(result.status.as_str(), "completed" | "cancelled")
                || all != expected
                || (result.status == "completed"
                    && (!result.remaining.is_empty() || !result.quarantined.is_empty()))
            {
                return Err(incomplete());
            }
            return Ok(result);
        }
        if !cancel.load(Ordering::Relaxed) {
            let validation = (|| {
                for retained in &plan.retained {
                    self.not_removed(&retained.snapshot_id)?;
                    if self.verify_directory(
                        &self.directory(&retained.snapshot_id)?,
                        &retained.snapshot_id,
                        cancel,
                    )? != *retained
                    {
                        return Err(incomplete());
                    }
                }
                let mut protected = self.capture_dependencies(cancel)?;
                protected.extend(self.daily_dependencies()?);
                protected.extend(self.restore_dependencies(cancel)?);
                protected.extend(self.upgrade_dependencies(cancel)?);
                protected.extend(self.overwrite_dependencies(cancel)?);
                if plan
                    .candidates
                    .iter()
                    .any(|snapshot| protected.contains(&snapshot.snapshot_id))
                {
                    return Err(error(
                        ErrorCode::VersionConflict,
                        "error.restorePlanChanged",
                    ));
                }
                Ok(())
            })();
            if let Err(failure) = validation {
                if !cancel.load(Ordering::Relaxed) {
                    return Err(failure);
                }
            }
        }
        let total = request
            .files
            .iter()
            .try_fold(0u64, |sum, files| sum.checked_add(files.len() as u64))
            .ok_or_else(incomplete)?;
        let mut completed = 0;
        for (snapshot, files) in plan.candidates.iter().zip(&request.files) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let original = self.directory(&snapshot.snapshot_id)?;
            let moved = journal.join(&snapshot.snapshot_id);
            let marker = self
                .destination
                .join(format!(".snapshot-removed-{}.json", snapshot.snapshot_id));
            let owned = if exists(&self.destination, &marker)? {
                if safe_io::read(&self.destination, &marker)? != request_hash.as_bytes() {
                    return Err(incomplete());
                }
                true
            } else {
                false
            };
            if !owned && !self.snapshot_from_source(&snapshot.snapshot_id)? {
                return Err(incomplete());
            }
            if exists(&self.destination, &original)? {
                if exists(&self.destination, &moved)? {
                    return Err(incomplete());
                }
                let verification = self
                    .verify_directory(&original, &snapshot.snapshot_id, cancel)
                    .and_then(|verified| {
                        if verified != *snapshot {
                            return Err(incomplete());
                        }
                        self.cleanup_files(snapshot, cancel)
                    });
                match verification {
                    Err(failure) if failure.code == ErrorCode::OperationCancelled => break,
                    Err(failure) => return Err(failure),
                    Ok(actual) if actual != *files => return Err(incomplete()),
                    Ok(_) => (),
                }
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                safe_io::publish(&self.destination, &marker, request_hash.as_bytes())?;
                safe_io::check(&self.destination, &original)?;
                safe_io::check(&self.destination, &journal)?;
                fs::rename(&original, &moved).map_err(safe_io::io)?;
            } else if !owned {
                return Err(incomplete());
            }
            if exists(&self.destination, &moved)? {
                for file in files {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let path = moved.join(&file.relative_path);
                    if exists(&self.destination, &path)? {
                        match self.verify_file(&moved, &path, file, cancel) {
                            Err(failure) if failure.code == ErrorCode::OperationCancelled => break,
                            Err(failure) => return Err(failure),
                            Ok(()) => (),
                        }
                        safe_io::check(&self.destination, &path)?;
                        fs::remove_file(path).map_err(safe_io::io)?;
                    }
                    completed += 1;
                    progress(SnapshotProgress {
                        phase: "cleaning".into(),
                        completed_objects: completed,
                        total_objects: total,
                    });
                }
                if files
                    .iter()
                    .all(|file| !moved.join(&file.relative_path).exists())
                {
                    let objects = moved.join("objects");
                    if exists(&self.destination, &objects)? {
                        fs::remove_dir(objects).map_err(safe_io::io)?;
                    }
                    safe_io::check(&self.destination, &moved)?;
                    fs::remove_dir(&moved).map_err(safe_io::io)?;
                }
            } else {
                completed += files.len() as u64;
            }
        }
        let mut result = SnapshotCleanupResult {
            operation_id: operation.into(),
            status: "completed".into(),
            removed: Vec::new(),
            remaining: Vec::new(),
            quarantined: Vec::new(),
        };
        for snapshot in &plan.candidates {
            if exists(&self.destination, &journal.join(&snapshot.snapshot_id))? {
                result.quarantined.push(snapshot.clone());
            } else if exists(&self.destination, &self.directory(&snapshot.snapshot_id)?)? {
                result.remaining.push(snapshot.clone());
            } else {
                let marker = self
                    .destination
                    .join(format!(".snapshot-removed-{}.json", snapshot.snapshot_id));
                if safe_io::read(&self.destination, &marker)? != request_hash.as_bytes() {
                    return Err(incomplete());
                }
                result.removed.push(snapshot.clone());
            }
        }
        if !result.remaining.is_empty() || !result.quarantined.is_empty() {
            result.status = "cancelled".into();
        }
        safe_io::publish(
            &self.destination,
            &journal.join("result.json"),
            &bytes(&Receipt {
                request_hash,
                result: result.clone(),
            })?,
        )?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    #[test]
    fn tampered_recipe_is_rejected_even_with_a_recomputed_record_checksum() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "tamper").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        for _ in 0..2 {
            backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        }
        let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        let operation = op();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = backup.apply_cleanup(&plan, &operation, &cancel, |p| {
                if p.phase == "cleaning" {
                    panic!("stop after delete");
                }
            });
        }));
        assert!(interrupted.is_err());
        let path = destination
            .path()
            .join(format!(".cleanup-journal-{operation}"))
            .join("request.json");
        let mut request: Request = read(destination.path(), &path).unwrap();
        request.files[0][0].relative_path = "../../escape.bin".into();
        let outside = destination.path().join("escape.bin");
        fs::write(&outside, b"keep").unwrap();
        fs::write(&path, bytes(&request).unwrap()).unwrap();
        assert_eq!(
            backup
                .resume_cleanup(&operation, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::BackupIncomplete
        );
        assert_eq!(fs::read(outside).unwrap(), b"keep");
        assert_eq!(
            backup.verify(&plan.retained[0].snapshot_id).unwrap(),
            plan.retained[0]
        );
    }
}
