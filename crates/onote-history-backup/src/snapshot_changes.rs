use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateRequest {
    operation_id: String,
    source: PathBuf,
    vault_id: String,
    conditional: bool,
    baseline: Option<(String, String)>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnchangedReceipt {
    request: CreateRequest,
    snapshot: Snapshot,
}

impl BackupStore {
    pub(super) fn capture_dependencies(
        &self,
        cancel: &AtomicBool,
    ) -> Result<HashSet<String>, ServiceError> {
        let mut protected = HashSet::new();
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            cancelled(cancel)?;
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|name| name.strip_prefix(".snapshot-request-"))
                .and_then(|name| name.strip_suffix(".json"))
                .filter(|id| canonical_uuid(id))
            else {
                continue;
            };
            match self.not_removed(id) {
                Ok(()) => (),
                Err(failure) if failure.code == ErrorCode::ObjectNotFound => continue,
                Err(failure) => return Err(failure),
            }
            safe_io::check(&self.destination, &entry.path())?;
            if entry.metadata().map_err(safe_io::io)?.len() > 1024 * 1024 {
                return Err(incomplete());
            }
            let request: CreateRequest =
                serde_json::from_slice(&safe_io::read(&self.destination, &entry.path())?)
                    .map_err(|_| incomplete())?;
            if request.operation_id != id || !canonical_uuid(&request.vault_id) {
                return Err(incomplete());
            }
            if request.vault_id != self.vault_id {
                continue;
            }
            // Published manifests and receipts classify completed operations; retention verifies payloads separately.
            if self.envelope(&self.directory(id)?, id).is_ok() {
                continue;
            }
            let receipt_path = self
                .destination
                .join(format!(".snapshot-unchanged-{id}.json"));
            if receipt_path.exists() {
                safe_io::check(&self.destination, &receipt_path)?;
                if fs::metadata(&receipt_path).map_err(safe_io::io)?.len() > 1024 * 1024 {
                    return Err(incomplete());
                }
                let record: UnchangedReceipt =
                    serde_json::from_slice(&safe_io::read(&self.destination, &receipt_path)?)
                        .map_err(|_| incomplete())?;
                if record.request != request
                    || request.baseline
                        != Some((record.snapshot.snapshot_id, record.snapshot.manifest_hash))
                {
                    return Err(incomplete());
                }
                continue;
            }
            if let Some((id, manifest_hash)) = request.baseline {
                if !canonical_uuid(&id) || !valid_hash(&manifest_hash) {
                    return Err(incomplete());
                }
                protected.insert(id);
            }
        }
        Ok(protected)
    }
    pub(super) fn snapshot_from_source(&self, id: &str) -> Result<bool, ServiceError> {
        let path = self
            .destination
            .join(format!(".snapshot-request-{id}.json"));
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(safe_io::io(e)),
            Ok(_) => (),
        }
        safe_io::check(&self.destination, &path)?;
        if fs::metadata(&path).map_err(safe_io::io)?.len() > 1024 * 1024 {
            return Err(incomplete());
        }
        let request: CreateRequest =
            serde_json::from_slice(&safe_io::read(&self.destination, &path)?)
                .map_err(|_| incomplete())?;
        Ok(request.operation_id == id
            && request.source == self.source
            && request.vault_id == self.vault_id)
    }
    pub fn create_if_changed(
        &self,
        storage: &VaultStorage,
        operation_id: &str,
        previous_snapshot_id: Option<&str>,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotProgress),
    ) -> Result<Snapshot, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        self.create_inner(
            storage,
            operation_id,
            true,
            previous_snapshot_id,
            cancel,
            progress,
        )
    }
    pub(super) fn create_request(
        &self,
        operation_id: &str,
        conditional: bool,
        previous: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<(CreateRequest, Option<(Envelope, Snapshot)>), ServiceError> {
        if self.directory(operation_id)?.exists() {
            let path = self
                .destination
                .join(format!(".snapshot-request-{operation_id}.json"));
            if path.exists() {
                safe_io::check(&self.destination, &path)?;
                if fs::metadata(&path).map_err(safe_io::io)?.len() > 1024 * 1024 {
                    return Err(incomplete());
                }
                let request: CreateRequest =
                    serde_json::from_slice(&safe_io::read(&self.destination, &path)?)
                        .map_err(|_| incomplete())?;
                if request.operation_id != operation_id
                    || request.source != self.source
                    || request.vault_id != self.vault_id
                    || request.conditional != conditional
                    || request.baseline.as_ref().map(|(id, _)| id.as_str()) != previous
                    || request
                        .baseline
                        .as_ref()
                        .is_some_and(|(_, digest)| !valid_hash(digest))
                {
                    return Err(error(
                        ErrorCode::IdempotencyMismatch,
                        "error.idempotencyMismatch",
                    ));
                }
                // A completed independent snapshot no longer needs its old baseline to replay.
                return Ok((request, None));
            }
        }
        let baseline = if let Some(id) = previous {
            self.not_removed(id)?;
            let snapshot = self.verify_directory(&self.directory(id)?, id, cancel)?;
            let envelope = self.envelope(&self.directory(id)?, id)?;
            if Self::summary(&envelope)? != snapshot {
                return Err(incomplete());
            }
            Some((envelope, snapshot))
        } else {
            None
        };
        let request = CreateRequest {
            operation_id: operation_id.into(),
            source: self.source.clone(),
            vault_id: self.vault_id.clone(),
            conditional,
            baseline: baseline.as_ref().map(|(_, snapshot)| {
                (snapshot.snapshot_id.clone(), snapshot.manifest_hash.clone())
            }),
        };
        let path = self
            .destination
            .join(format!(".snapshot-request-{operation_id}.json"));
        let bytes = serde_json::to_vec(&request).map_err(|_| incomplete())?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                if safe_io::read(&self.destination, &path)? != bytes {
                    return Err(error(
                        ErrorCode::IdempotencyMismatch,
                        "error.idempotencyMismatch",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Older snapshot operations were all unconditional; preserve their read-only replay.
                if self.directory(operation_id)?.exists()
                    || self
                        .destination
                        .join(format!(".staging-{operation_id}"))
                        .exists()
                {
                    if conditional {
                        return Err(error(
                            ErrorCode::IdempotencyMismatch,
                            "error.idempotencyMismatch",
                        ));
                    }
                } else {
                    safe_io::publish(&self.destination, &path, &bytes)?;
                }
            }
            Err(e) => return Err(safe_io::io(e)),
        }
        Ok((request, baseline))
    }
    pub(super) fn unchanged_result(
        &self,
        request: &CreateRequest,
    ) -> Result<Option<Snapshot>, ServiceError> {
        let path = self
            .destination
            .join(format!(".snapshot-unchanged-{}.json", request.operation_id));
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(safe_io::io(e)),
            Ok(_) => (),
        }
        let record: UnchangedReceipt =
            serde_json::from_slice(&safe_io::read(&self.destination, &path)?)
                .map_err(|_| incomplete())?;
        if record.request != *request
            || !request.conditional
            || request.baseline.as_ref()
                != Some(&(
                    record.snapshot.snapshot_id.clone(),
                    record.snapshot.manifest_hash.clone(),
                ))
        {
            return Err(incomplete());
        }
        if self.verify(&record.snapshot.snapshot_id)? != record.snapshot {
            return Err(incomplete());
        }
        Ok(Some(record.snapshot))
    }
    pub(super) fn reuse_if_unchanged(
        &self,
        stage: &Path,
        request: &CreateRequest,
        baseline: &Option<(Envelope, Snapshot)>,
        manifest: &Manifest,
        cancel: &AtomicBool,
    ) -> Result<Option<Snapshot>, ServiceError> {
        let Some((previous, snapshot)) = baseline else {
            return Ok(None);
        };
        let mut old_notes = previous.manifest.notes.clone();
        let mut new_notes = manifest.notes.clone();
        old_notes.sort_by(|a, b| a.note_id.cmp(&b.note_id));
        new_notes.sort_by(|a, b| a.note_id.cmp(&b.note_id));
        if previous.manifest.schema_version != manifest.schema_version
            || old_notes != new_notes
            || previous.manifest.files != manifest.files
        {
            return Ok(None);
        }
        cancelled(cancel)?;
        if self.verify_directory(
            &self.directory(&snapshot.snapshot_id)?,
            &snapshot.snapshot_id,
            cancel,
        )? != *snapshot
        {
            return Err(incomplete());
        }
        let record = UnchangedReceipt {
            request: request.clone(),
            snapshot: snapshot.clone(),
        };
        safe_io::publish(
            &self.destination,
            &self
                .destination
                .join(format!(".snapshot-unchanged-{}.json", request.operation_id)),
            &serde_json::to_vec(&record).map_err(|_| incomplete())?,
        )?;
        let state = manifest
            .files
            .iter()
            .find(|file| file.relative_path == ".opennote/state.sqlite")
            .ok_or_else(incomplete)?;
        self.verify_file(
            stage,
            &stage.join("state.sqlite"),
            state,
            &AtomicBool::new(false),
        )?;
        safe_io::check(&self.destination, stage)?;
        fs::remove_file(stage.join("state.sqlite")).map_err(safe_io::io)?;
        fs::remove_dir(stage).map_err(safe_io::io)?;
        Ok(Some(snapshot.clone()))
    }
}
