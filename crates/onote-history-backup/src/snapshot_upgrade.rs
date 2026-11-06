use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotUpgradePlan {
    pub plan_id: String,
    pub snapshot: Snapshot,
    pub from_schema: u32,
    pub to_schema: u32,
    pub note_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingSnapshotUpgrade {
    pub operation_id: String,
    pub plan: SnapshotUpgradePlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotUpgradeAbandonment {
    pub operation_id: String,
    pub snapshot_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation_id: String,
    source: PathBuf,
    plan: SnapshotUpgradePlan,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AbandonmentReceipt {
    request_hash: String,
    result: SnapshotUpgradeAbandonment,
}

fn interrupted() -> ServiceError {
    error(ErrorCode::IoDenied, "error.upgradeInterrupted")
}
fn read<T: Serialize + serde::de::DeserializeOwned>(
    root: &Path,
    path: &Path,
) -> Result<T, ServiceError> {
    safe_io::check(root, path)?;
    if fs::metadata(path).map_err(safe_io::io)?.len() > 64 * 1024 * 1024 {
        return Err(interrupted());
    }
    restore::read_record(root, path).map_err(|_| interrupted())
}

fn check_source(
    backup: &BackupStore,
    root: &Path,
    vault: &onote_domain::Vault,
    notes: &[NoteSummary],
    expected: &Manifest,
    stage: &Path,
    state_file: &Path,
    resuming: bool,
    cancel: &AtomicBool,
) -> Result<(), ServiceError> {
    cancelled(cancel)?;
    if root != backup.source
        || vault.vault_id != backup.vault_id
        || vault.schema_version != expected.schema_version
    {
        return Err(interrupted());
    }
    let mut old_notes = expected.notes.clone();
    let mut current_notes = notes.to_vec();
    old_notes.sort_by(|a, b| a.note_id.cmp(&b.note_id));
    current_notes.sort_by(|a, b| a.note_id.cmp(&b.note_id));
    let mut paths = inventory(root, cancel)?;
    if resuming {
        paths.retain(|p| {
            expected.files.iter().any(|f| &f.relative_path == p)
                || (!p.starts_with(".opennote/upgrade-v1-v2/")
                    && !p.starts_with(".opennote/upgrade-v2-v3/"))
        });
    }
    paths.push(".opennote/state.sqlite".into());
    paths.sort();
    if old_notes != current_notes
        || paths
            != expected
                .files
                .iter()
                .map(|f| f.relative_path.clone())
                .collect::<Vec<_>>()
    {
        return Err(error(ErrorCode::VersionConflict, "error.snapshotChanged"));
    }
    for file in &expected.files {
        if resuming
            && matches!(
                file.relative_path.as_str(),
                ".opennote/state.sqlite" | ".opennote/vault.json"
            )
        {
            continue;
        }
        let (base, path) = if file.relative_path == ".opennote/state.sqlite" {
            (stage, state_file.to_path_buf())
        } else {
            (root, root.join(&file.relative_path))
        };
        if digest_file(base, &path, cancel)? != (file.content_hash.clone(), file.byte_length) {
            return Err(error(ErrorCode::VersionConflict, "error.snapshotChanged"));
        }
    }
    cancelled(cancel)
}

fn clean_stage(destination: &Path, stage: &Path) -> Result<(), ServiceError> {
    safe_io::check(destination, stage)?;
    let state = stage.join("state.sqlite");
    match fs::symlink_metadata(&state) {
        Ok(_) => {
            safe_io::check(stage, &state)?;
            fs::remove_file(state).map_err(safe_io::io)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(safe_io::io(e)),
    }
    fs::remove_dir(stage).map_err(safe_io::io)
}

fn source_space(source: &Path, exported_bytes: u64) -> Result<(), ServiceError> {
    let mut sizes = vec![exported_bytes];
    for name in ["state.sqlite", "state.sqlite-wal"] {
        let path = source.join(".opennote").join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                safe_io::check(source, &path)?;
                if !metadata.is_file() {
                    return Err(interrupted());
                }
                sizes.push(metadata.len());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && name.ends_with("-wal") => (),
            Err(e) => return Err(safe_io::io(e)),
        }
    }
    space::check(source, sizes)
}

impl BackupStore {
    fn upgrade_id(&self, source: &Path, snapshot: &Snapshot) -> Result<String, ServiceError> {
        Ok(hash(
            &serde_json::to_vec(&(source, &self.destination, snapshot))
                .map_err(|_| interrupted())?,
        ))
    }
    pub(super) fn not_upgrade_operation(&self, operation: &str) -> Result<(), ServiceError> {
        self.directory(operation)?;
        match fs::symlink_metadata(
            self.destination
                .join(format!(".upgrade-journal-{operation}")),
        ) {
            Ok(_) => Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(safe_io::io(e)),
        }
    }
    fn upgrade_request(&self, operation: &str) -> Result<(PathBuf, Request), ServiceError> {
        self.directory(operation)?;
        let folder = self
            .destination
            .join(format!(".upgrade-journal-{operation}"));
        let request: Request = read(&self.destination, &folder.join("request.json"))?;
        if request.operation_id != operation
            || !canonical_uuid(&request.plan.snapshot.vault_id)
            || !canonical_uuid(&request.plan.snapshot.snapshot_id)
            || !matches!(request.plan.from_schema, 1 | 2)
            || !matches!(
                (request.plan.from_schema, request.plan.to_schema),
                (1, 2) | (1, 3) | (2, 3)
            )
            || request.plan.snapshot.schema_version != request.plan.from_schema
            || !valid_hash(&request.plan.snapshot.manifest_hash)
            || !request.source.is_absolute()
            || request.plan.plan_id != self.upgrade_id(&request.source, &request.plan.snapshot)?
        {
            return Err(interrupted());
        }
        Ok((folder, request))
    }
    fn upgrade_receipt(
        &self,
        folder: &Path,
        request: &Request,
    ) -> Result<Option<onote_vault_storage::SchemaUpgradeResult>, ServiceError> {
        let path = folder.join("complete.json");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(safe_io::io(e)),
            _ => (),
        }
        let result: onote_vault_storage::SchemaUpgradeResult = read(&self.destination, &path)?;
        if result.operation_id != request.operation_id
            || result.manifest_hash != request.plan.snapshot.manifest_hash
            || result.from_schema != request.plan.from_schema
            || result.to_schema != request.plan.to_schema
            || result.note_count != request.plan.note_count
        {
            return Err(interrupted());
        }
        Ok(Some(result))
    }
    fn upgrade_abandonment(
        &self,
        folder: &Path,
        request: &Request,
    ) -> Result<Option<SnapshotUpgradeAbandonment>, ServiceError> {
        let path = folder.join("abandoned.json");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(safe_io::io(e)),
            Ok(_) => (),
        }
        let receipt: AbandonmentReceipt = read(&self.destination, &path)?;
        let request_hash = hash(&serde_json::to_vec(request).map_err(|_| interrupted())?);
        if receipt.request_hash != request_hash
            || receipt.result.operation_id != request.operation_id
            || receipt.result.snapshot_id != request.plan.snapshot.snapshot_id
        {
            return Err(interrupted());
        }
        Ok(Some(receipt.result))
    }
    pub fn pending_upgrades(
        &self,
        cancel: &AtomicBool,
    ) -> Result<Vec<PendingSnapshotUpgrade>, ServiceError> {
        let mut pending = Vec::new();
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            cancelled(cancel)?;
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(operation) = name
                .to_str()
                .and_then(|s| s.strip_prefix(".upgrade-journal-"))
                .filter(|s| canonical_uuid(s))
            else {
                continue;
            };
            let (folder, request) = self.upgrade_request(operation)?;
            if request.source == self.source
                && request.plan.snapshot.vault_id == self.vault_id
                && self.upgrade_receipt(&folder, &request)?.is_none()
                && self.upgrade_abandonment(&folder, &request)?.is_none()
            {
                pending.push(PendingSnapshotUpgrade {
                    operation_id: operation.into(),
                    plan: request.plan,
                });
            }
        }
        pending.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        Ok(pending)
    }
    pub(super) fn upgrade_dependencies(
        &self,
        cancel: &AtomicBool,
    ) -> Result<HashSet<String>, ServiceError> {
        Ok(self
            .pending_upgrades(cancel)?
            .into_iter()
            .map(|p| p.plan.snapshot.snapshot_id)
            .collect())
    }
    pub fn apply_upgrade(
        &self,
        storage: &VaultStorage,
        plan: &SnapshotUpgradePlan,
        operation: &str,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<onote_vault_storage::SchemaUpgradeResult, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        self.not_overwrite_operation(operation)?;
        self.directory(operation)?;
        if fs::canonicalize(storage.root_path()).map_err(safe_io::io)? != self.source
            || storage.vault().vault_id != self.vault_id
        {
            return Err(interrupted());
        }
        let folder = self
            .destination
            .join(format!(".upgrade-journal-{operation}"));
        let request_path = folder.join("request.json");
        let new_request = match fs::symlink_metadata(&request_path) {
            Ok(_) => false,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => return Err(safe_io::io(e)),
        };
        if new_request {
            cancelled(cancel)?;
            for path in [
                self.directory(operation)?,
                self.destination
                    .join(format!(".snapshot-request-{operation}.json")),
                self.destination
                    .join(format!(".restore-journal-{operation}")),
                self.destination
                    .join(format!(".cleanup-journal-{operation}")),
                self.destination
                    .join(format!(".snapshot-removed-{operation}.json")),
                self.destination
                    .join(format!(".snapshot-unchanged-{operation}.json")),
                self.destination.join(format!(".staging-{operation}")),
            ] {
                match fs::symlink_metadata(&path) {
                    Ok(_) => {
                        return Err(error(
                            ErrorCode::IdempotencyMismatch,
                            "error.idempotencyMismatch",
                        ))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(safe_io::io(e)),
                }
            }
            self.not_removed(&plan.snapshot.snapshot_id)?;
            let snapshot = self.verify_directory(
                &self.directory(&plan.snapshot.snapshot_id)?,
                &plan.snapshot.snapshot_id,
                cancel,
            )?;
            let envelope = self.envelope(
                &self.directory(&snapshot.snapshot_id)?,
                &snapshot.snapshot_id,
            )?;
            if snapshot != plan.snapshot
                || plan.plan_id != self.upgrade_id(&self.source, &snapshot)?
                || plan.from_schema != snapshot.schema_version
                || plan.to_schema != 3
                || !matches!(snapshot.schema_version, 1 | 2)
                || plan.note_count != envelope.manifest.notes.len() as u64
                || !self.snapshot_from_source(&snapshot.snapshot_id)?
                || envelope
                    .manifest
                    .files
                    .iter()
                    .any(|f| f.relative_path.starts_with(".opennote/upgrade-v2-v3/"))
            {
                return Err(interrupted());
            }
            safe_io::ensure_dir(&self.destination, &folder)?;
            safe_io::publish(
                &self.destination,
                &request_path,
                &restore::record_bytes(&Request {
                    operation_id: operation.into(),
                    source: self.source.clone(),
                    plan: plan.clone(),
                })?,
            )?;
        }
        let (folder, request) = self.upgrade_request(operation)?;
        if request.source != self.source || request.plan != *plan {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        if let Some(result) = self.upgrade_receipt(&folder, &request)? {
            return Ok(result);
        }
        if self.upgrade_abandonment(&folder, &request)?.is_some() {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        if let Some(result) =
            storage.completed_schema_upgrade(operation, &plan.snapshot.manifest_hash)?
        {
            safe_io::publish(
                &self.destination,
                &folder.join("complete.json"),
                &restore::record_bytes(&result)?,
            )?;
            return Ok(result);
        }
        cancelled(cancel)?;
        self.not_removed(&plan.snapshot.snapshot_id)?;
        if self.verify_directory(
            &self.directory(&plan.snapshot.snapshot_id)?,
            &plan.snapshot.snapshot_id,
            cancel,
        )? != plan.snapshot
        {
            return Err(interrupted());
        }
        let expected = self
            .envelope(
                &self.directory(&plan.snapshot.snapshot_id)?,
                &plan.snapshot.snapshot_id,
            )?
            .manifest;
        if expected
            .files
            .iter()
            .any(|f| f.relative_path.starts_with(".opennote/upgrade-v2-v3/"))
        {
            return Err(interrupted());
        }
        let database = expected
            .files
            .iter()
            .find(|f| f.relative_path == ".opennote/state.sqlite")
            .ok_or_else(interrupted)?;
        source_space(&self.source, database.byte_length)?;
        space::check(&self.destination, [database.byte_length])?;
        let stage = self
            .destination
            .join(format!(".upgrade-apply-{}", uuid::Uuid::new_v4()));
        safe_io::check(&self.destination, &self.destination)?;
        fs::create_dir(&stage).map_err(safe_io::io)?;
        let state_file = stage.join("state.sqlite");
        progress(SnapshotProgress {
            phase: "verifying".into(),
            completed_objects: 0,
            total_objects: 0,
        });
        let result = storage.apply_schema_upgrade_to(
            &state_file,
            operation,
            &plan.snapshot.manifest_hash,
            plan.to_schema,
            |root, vault, notes, resuming| {
                check_source(
                    self,
                    root,
                    vault,
                    notes,
                    &expected,
                    &stage,
                    &state_file,
                    resuming,
                    cancel,
                )?;
                source_space(root, database.byte_length)?;
                progress(SnapshotProgress {
                    phase: "finalizing".into(),
                    completed_objects: 0,
                    total_objects: 0,
                });
                cancelled(cancel)
            },
        );
        clean_stage(&self.destination, &stage)?;
        let result = result?;
        safe_io::publish(
            &self.destination,
            &folder.join("complete.json"),
            &restore::record_bytes(&result)?,
        )?;
        Ok(result)
    }

    /// Abandoning only releases this unfinished task's snapshot dependency.
    /// It never deletes the snapshot or changes the source vault.
    pub fn abandon_upgrade(
        &self,
        storage: &VaultStorage,
        operation: &str,
        cancel: &AtomicBool,
    ) -> Result<SnapshotUpgradeAbandonment, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        self.not_overwrite_operation(operation)?;
        self.directory(operation)?;
        if fs::canonicalize(storage.root_path()).map_err(safe_io::io)? != self.source
            || storage.vault().vault_id != self.vault_id
        {
            return Err(interrupted());
        }
        let (folder, request) = self.upgrade_request(operation)?;
        if request.source != self.source || request.plan.snapshot.vault_id != self.vault_id {
            return Err(error(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
            ));
        }
        if self.upgrade_receipt(&folder, &request)?.is_some() {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.upgradeInterrupted",
            ));
        }
        if let Some(result) = self.upgrade_abandonment(&folder, &request)? {
            return Ok(result);
        }
        cancelled(cancel)?;
        if !storage.schema_upgrade_not_started(
            &request.operation_id,
            &request.plan.snapshot.manifest_hash,
        )? {
            return Err(error(
                ErrorCode::VersionConflict,
                "error.upgradeInterrupted",
            ));
        }
        let result = SnapshotUpgradeAbandonment {
            operation_id: request.operation_id.clone(),
            snapshot_id: request.plan.snapshot.snapshot_id.clone(),
        };
        cancelled(cancel)?;
        safe_io::publish(
            &self.destination,
            &folder.join("abandoned.json"),
            &restore::record_bytes(&AbandonmentReceipt {
                request_hash: hash(&serde_json::to_vec(&request).map_err(|_| interrupted())?),
                result: result.clone(),
            })?,
        )?;
        Ok(result)
    }

    /// Inspection only: a plan never changes the source format or authorizes writes.
    pub fn plan_upgrade(
        &self,
        storage: &VaultStorage,
        snapshot_id: &str,
        cancel: &AtomicBool,
    ) -> Result<SnapshotUpgradePlan, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        cancelled(cancel)?;
        self.not_removed(snapshot_id)?;
        let snapshot = self.verify_directory(&self.directory(snapshot_id)?, snapshot_id, cancel)?;
        if !matches!(snapshot.schema_version, 1 | 2) || !self.snapshot_from_source(snapshot_id)? {
            return Err(error(ErrorCode::UnsupportedSchema, "error.schemaReadOnly"));
        }
        let expected = self
            .envelope(&self.directory(snapshot_id)?, snapshot_id)?
            .manifest;
        if expected
            .files
            .iter()
            .any(|f| f.relative_path.starts_with(".opennote/upgrade-v2-v3/"))
        {
            return Err(interrupted());
        }
        let database = expected
            .files
            .iter()
            .find(|f| f.relative_path == ".opennote/state.sqlite")
            .ok_or_else(incomplete)?;
        space::check(&self.destination, [database.byte_length])?;
        let stage = self
            .destination
            .join(format!(".upgrade-preview-{}", uuid::Uuid::new_v4()));
        safe_io::check(&self.destination, &self.destination)?;
        fs::create_dir(&stage).map_err(safe_io::io)?;
        let state_file = stage.join("state.sqlite");
        let result = storage.with_snapshot_state(&state_file, |root, vault, notes| {
            cancelled(cancel)?;
            if root != self.source
                || vault.vault_id != self.vault_id
                || vault.schema_version != expected.schema_version
            {
                return Err(error(ErrorCode::VersionConflict, "error.snapshotChanged"));
            }
            check_source(
                self,
                root,
                vault,
                notes,
                &expected,
                &stage,
                &state_file,
                false,
                cancel,
            )?;
            Ok(SnapshotUpgradePlan {
                plan_id: hash(
                    &serde_json::to_vec(&(&self.source, &self.destination, &snapshot))
                        .map_err(|_| incomplete())?,
                ),
                snapshot: snapshot.clone(),
                from_schema: snapshot.schema_version,
                to_schema: 3,
                note_count: notes.len() as u64,
            })
        });
        clean_stage(&self.destination, &stage)?;
        let plan = result?;
        cancelled(cancel)?;
        Ok(plan)
    }
}
