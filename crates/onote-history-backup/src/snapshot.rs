use crate::{canonical_uuid, error, hash, safe_io};
use chrono::Utc;
use onote_domain::{ErrorCode, NoteSummary, ServiceError, Snapshot, SnapshotProgress};
use onote_vault_storage::VaultStorage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[path = "snapshot_restore.rs"]
mod restore;
pub use restore::{SnapshotRestoreCancellation, SnapshotRestoreResult};
#[path = "snapshot_references.rs"]
mod references;
pub use references::{
    SnapshotReferenceIssue, SnapshotReferenceIssueReason, SnapshotReferenceReport,
};

#[path = "snapshot_changes.rs"]
mod changes;
#[path = "snapshot_daily.rs"]
mod daily;
#[path = "snapshot_space.rs"]
mod space;
pub use daily::DailyBackupResult;
#[path = "snapshot_retention.rs"]
mod retention;
pub use retention::{PreservedSnapshot, SnapshotPreservationReason, SnapshotRetentionPlan};
#[path = "snapshot_cleanup.rs"]
mod cleanup;
pub use cleanup::{PendingSnapshotCleanup, SnapshotCleanupResult};
#[path = "snapshot_publication.rs"]
mod publication;
#[path = "snapshot_upgrade.rs"]
mod upgrade;
pub use upgrade::{PendingSnapshotUpgrade, SnapshotUpgradeAbandonment, SnapshotUpgradePlan};
#[path = "snapshot_overwrite.rs"]
mod overwrite;
pub use overwrite::{SnapshotOverwritePlan, SnapshotOverwriteResult};

#[derive(Debug)]
pub struct BackupStore {
    source: PathBuf,
    destination: PathBuf,
    vault_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotRestorePlan {
    pub plan_id: String,
    pub snapshot: Snapshot,
    pub target: PathBuf,
    pub note_count: u64,
    #[serde(default)]
    pub reference_report: SnapshotReferenceReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotFile {
    relative_path: String,
    content_hash: String,
    byte_length: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    snapshot_id: String,
    vault_id: String,
    schema_version: u32,
    created_at: String,
    notes: Vec<NoteSummary>,
    files: Vec<SnapshotFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    manifest: Manifest,
    checksum: String,
}

fn incomplete() -> ServiceError {
    error(ErrorCode::BackupIncomplete, "error.backupIncomplete")
}
fn snapshot_time() -> String {
    #[cfg(test)]
    if let Some(value) = publication::TEST_TIME.with(|value| value.borrow().clone()) {
        return value;
    }
    Utc::now().to_rfc3339()
}
fn source_error(failure: ServiceError) -> ServiceError {
    if failure.code == ErrorCode::HistoryMissing {
        incomplete()
    } else {
        failure
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<(), ServiceError> {
    if cancel.load(Ordering::Relaxed) {
        Err(error(
            ErrorCode::OperationCancelled,
            "error.operationCancelled",
        ))
    } else {
        Ok(())
    }
}
fn external_directory(path: &Path) -> Result<PathBuf, ServiceError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(error(ErrorCode::InvalidInput, "error.backupDestination"));
    }
    let mut ancestor = path.to_path_buf();
    loop {
        safe_io::check(&ancestor, &ancestor)?;
        if !ancestor.pop() {
            break;
        }
    }
    let canonical = fs::canonicalize(path).map_err(safe_io::io)?;
    if !canonical.is_dir() {
        return Err(error(ErrorCode::InvalidInput, "error.backupDestination"));
    }
    Ok(canonical)
}
fn valid_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':', '\0'])
        && path.split('/').all(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with([' ', '.'])
                && !part
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '"' | '<' | '>' | '|' | '*' | '?'))
                && !matches!(
                    stem.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                )
                && !stem
                    .strip_prefix("COM")
                    .or_else(|| stem.strip_prefix("LPT"))
                    .is_some_and(|suffix| {
                        matches!(
                            suffix,
                            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                        )
                    })
        })
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn digest_file(
    root: &Path,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(String, u64), ServiceError> {
    safe_io::check(root, path).map_err(source_error)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut input = options
        .open(path)
        .map_err(safe_io::io)
        .map_err(source_error)?;
    if !input.metadata().map_err(safe_io::io)?.is_file() {
        return Err(incomplete());
    }
    let mut hasher = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        cancelled(cancel)?;
        let size = input.read(&mut buffer).map_err(safe_io::io)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
        count = count.checked_add(size as u64).ok_or_else(incomplete)?;
    }
    Ok((hex::encode(hasher.finalize()), count))
}

fn walk(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<String>,
    depth: usize,
    cancel: &AtomicBool,
) -> Result<(), ServiceError> {
    if depth > 64 {
        return Err(incomplete());
    }
    safe_io::check(root, directory)?;
    for entry in fs::read_dir(directory).map_err(safe_io::io)? {
        cancelled(cancel)?;
        let path = entry.map_err(safe_io::io)?.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| incomplete())?
            .to_str()
            .ok_or_else(incomplete)?
            .replace('\\', "/");
        if !valid_relative(&relative) {
            return Err(incomplete());
        }
        if relative.eq_ignore_ascii_case(".opennote") {
            continue;
        }
        safe_io::check(root, &path)?;
        let metadata = fs::symlink_metadata(&path).map_err(safe_io::io)?;
        if metadata.is_dir() {
            walk(root, &path, paths, depth + 1, cancel)?;
        } else if metadata.is_file() {
            paths.push(relative);
        } else {
            return Err(incomplete());
        }
    }
    Ok(())
}

fn inventory(root: &Path, cancel: &AtomicBool) -> Result<Vec<String>, ServiceError> {
    let mut paths = Vec::new();
    walk(root, root, &mut paths, 0, cancel)?;
    paths.push(".opennote/vault.json".into());
    for folder in ["history", "trash", "upgrade-v1-v2"] {
        let directory = root.join(".opennote").join(folder);
        match fs::symlink_metadata(&directory) {
            Ok(_) => walk(root, &directory, &mut paths, 0, cancel)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(safe_io::io(e)),
        }
    }
    paths.sort();
    Ok(paths)
}

impl BackupStore {
    pub fn open(
        source: impl AsRef<Path>,
        vault_id: &str,
        destination: impl AsRef<Path>,
    ) -> Result<Self, ServiceError> {
        if !canonical_uuid(vault_id) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidVaultId"));
        }
        let source = external_directory(source.as_ref())?;
        let destination = external_directory(destination.as_ref())?;
        if destination.starts_with(&source) {
            return Err(error(ErrorCode::InvalidInput, "error.backupRecursive"));
        }
        Ok(Self {
            source,
            destination,
            vault_id: vault_id.into(),
        })
    }

    fn directory(&self, id: &str) -> Result<PathBuf, ServiceError> {
        if !canonical_uuid(id) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidSnapshotId"));
        }
        Ok(self.destination.join(id))
    }

    pub fn create(
        &self,
        storage: &VaultStorage,
        operation_id: &str,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotProgress),
    ) -> Result<Snapshot, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        self.create_inner(storage, operation_id, false, None, cancel, progress)
    }

    fn create_inner(
        &self,
        storage: &VaultStorage,
        operation_id: &str,
        conditional: bool,
        previous_snapshot_id: Option<&str>,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<Snapshot, ServiceError> {
        self.not_removed(operation_id)?;
        self.not_cleanup_operation(operation_id)?;
        let complete = self.directory(operation_id)?;
        if storage.vault().vault_id != self.vault_id
            || fs::canonicalize(storage.root_path()).map_err(safe_io::io)? != self.source
        {
            return Err(error(ErrorCode::InvalidInput, "error.vaultSessionChanged"));
        }
        let _operation_guard = self.lock_operation(operation_id)?;
        let completed = fs::symlink_metadata(&complete).is_ok()
            || fs::symlink_metadata(
                self.destination
                    .join(format!(".snapshot-unchanged-{operation_id}.json")),
            )
            .is_ok();
        let replay = AtomicBool::new(false);
        let (request, baseline) = self.create_request(
            operation_id,
            conditional,
            previous_snapshot_id,
            if completed { &replay } else { cancel },
        )?;
        if let Some(snapshot) = self.unchanged_result(&request)? {
            return Ok(snapshot);
        }
        if fs::symlink_metadata(&complete).is_ok() {
            return self.verify(operation_id);
        }
        cancelled(cancel)?;
        let stage = self.destination.join(format!(".staging-{operation_id}"));
        match fs::symlink_metadata(&stage) {
            Ok(_) => return self.resume_stage(&stage, operation_id, cancel, &mut progress),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(safe_io::io(e)),
        }
        progress(SnapshotProgress {
            phase: "checkingSpace".into(),
            completed_objects: 0,
            total_objects: 0,
        });
        self.check_new_snapshot_space(cancel)?;
        cancelled(cancel)?;
        match fs::create_dir(&stage) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return self.resume_stage(&stage, operation_id, cancel, &mut progress)
            }
            Err(e) => return Err(safe_io::io(e)),
        }
        safe_io::check(&self.destination, &stage)?;
        let state_file = stage.join("state.sqlite");
        let reused = storage.with_snapshot_state(&state_file, |root, vault, notes| {
            if root != self.source || vault.vault_id != self.vault_id {
                return Err(error(ErrorCode::InvalidInput, "error.vaultSessionChanged"));
            }
            progress(SnapshotProgress {
                phase: "planning".into(),
                completed_objects: 0,
                total_objects: 0,
            });
            let paths = inventory(root, cancel)?;
            let mut files = Vec::with_capacity(paths.len() + 1);
            for relative in paths {
                let (content_hash, byte_length) = digest_file(root, &root.join(&relative), cancel)?;
                files.push(SnapshotFile {
                    relative_path: relative,
                    content_hash,
                    byte_length,
                });
            }
            let (content_hash, byte_length) = digest_file(&stage, &state_file, cancel)?;
            files.push(SnapshotFile {
                relative_path: ".opennote/state.sqlite".into(),
                content_hash,
                byte_length,
            });
            files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
            let manifest = Manifest {
                format_version: 1,
                snapshot_id: operation_id.into(),
                vault_id: self.vault_id.clone(),
                schema_version: vault.schema_version,
                created_at: snapshot_time(),
                notes: notes.to_vec(),
                files,
            };
            self.validate_manifest(&manifest, operation_id)?;
            if let Some(snapshot) =
                self.reuse_if_unchanged(&stage, &request, &baseline, &manifest, cancel)?
            {
                return Ok(Some(snapshot));
            }
            let checksum = hash(&serde_json::to_vec(&manifest).map_err(|_| incomplete())?);
            safe_io::publish(
                &self.destination,
                &stage.join("manifest.json"),
                &serde_json::to_vec(&Envelope { manifest, checksum }).map_err(|_| incomplete())?,
            )?;
            self.copy_stage(&stage, operation_id, cancel, &mut progress)?;
            Ok(None)
        })?;
        if let Some(snapshot) = reused {
            return Ok(snapshot);
        }
        self.publish_stage(&stage, operation_id, cancel)
    }

    fn lock_operation(&self, id: &str) -> Result<fs::File, ServiceError> {
        let path = self.destination.join(format!(".snapshot-lock-{id}"));
        safe_io::check(&self.destination, &self.destination)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => safe_io::check(&self.destination, &path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(safe_io::io(e)),
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(safe_io::io)?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                let mut failure = error(ErrorCode::FileLocked, "error.backupBusy");
                failure.retryable = true;
                Err(failure)
            }
            Err(fs::TryLockError::Error(e)) => Err(safe_io::io(e)),
        }
    }

    fn envelope(&self, directory: &Path, id: &str) -> Result<Envelope, ServiceError> {
        let bytes = safe_io::read(&self.destination, &directory.join("manifest.json"))
            .map_err(|_| incomplete())?;
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| incomplete())?;
        self.validate_manifest(&envelope.manifest, id)?;
        if hash(&serde_json::to_vec(&envelope.manifest).map_err(|_| incomplete())?)
            != envelope.checksum
        {
            return Err(incomplete());
        }
        Ok(envelope)
    }

    fn validate_manifest(&self, manifest: &Manifest, id: &str) -> Result<(), ServiceError> {
        if manifest.format_version != 1
            || manifest.snapshot_id != id
            || !canonical_uuid(id)
            || manifest.vault_id != self.vault_id
            || manifest.schema_version == 0
            || chrono::DateTime::parse_from_rfc3339(&manifest.created_at).is_err()
        {
            return Err(incomplete());
        }
        let mut paths = BTreeMap::new();
        for file in &manifest.files {
            if !valid_relative(&file.relative_path)
                || !valid_hash(&file.content_hash)
                || paths
                    .insert(file.relative_path.clone(), file.content_hash.as_str())
                    .is_some()
            {
                return Err(incomplete());
            }
            if file
                .relative_path
                .split('/')
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case(".opennote"))
                && !matches!(
                    file.relative_path.as_str(),
                    ".opennote/state.sqlite" | ".opennote/vault.json"
                )
                && !file.relative_path.starts_with(".opennote/history/")
                && !file.relative_path.starts_with(".opennote/trash/")
                && !file.relative_path.starts_with(".opennote/upgrade-v1-v2/")
            {
                return Err(incomplete());
            }
        }
        if !paths.contains_key(".opennote/state.sqlite")
            || !paths.contains_key(".opennote/vault.json")
        {
            return Err(incomplete());
        }
        let mut ordered = paths.keys().collect::<Vec<_>>();
        ordered.sort_by(|a, b| onote_vault_storage::compare_windows_paths(a, b));
        if ordered
            .windows(2)
            .any(|pair| onote_vault_storage::compare_windows_paths(pair[0], pair[1]).is_eq())
        {
            return Err(incomplete());
        }
        for path in &ordered {
            let mut parent = path.as_str();
            while let Some((prefix, _)) = parent.rsplit_once('/') {
                if ordered
                    .binary_search_by(|candidate| {
                        onote_vault_storage::compare_windows_paths(candidate, prefix)
                    })
                    .is_ok()
                {
                    return Err(incomplete());
                }
                parent = prefix;
            }
        }
        let mut ids = HashSet::new();
        let mut note_paths = HashSet::new();
        for note in &manifest.notes {
            if !canonical_uuid(&note.note_id)
                || note.vault_id != self.vault_id
                || note.local_revision > 9_007_199_254_740_991
                || !ids.insert(note.note_id.as_str())
                || !note_paths.insert(note.relative_path.as_str())
                || paths.get(&note.relative_path).copied() != Some(note.content_hash.as_str())
            {
                return Err(incomplete());
            }
        }
        Ok(())
    }

    fn copy_stage(
        &self,
        stage: &Path,
        id: &str,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(SnapshotProgress),
    ) -> Result<(), ServiceError> {
        let envelope = self.envelope(stage, id)?;
        self.check_copy_space(stage, stage, &envelope.manifest.files, false)?;
        safe_io::ensure_dir(&self.destination, &stage.join("objects"))?;
        let total = envelope.manifest.files.len() as u64;
        for (index, file) in envelope.manifest.files.iter().enumerate() {
            progress(SnapshotProgress {
                phase: "copying".into(),
                completed_objects: index as u64,
                total_objects: total,
            });
            cancelled(cancel)?;
            let target = stage.join("objects").join(&file.content_hash);
            if fs::symlink_metadata(&target).is_ok() {
                self.verify_file(stage, &target, file, cancel)?;
                continue;
            }
            let (root, source) = if file.relative_path == ".opennote/state.sqlite" {
                (stage, stage.join("state.sqlite"))
            } else {
                (self.source.as_path(), self.source.join(&file.relative_path))
            };
            self.copy_file(root, &source, stage, &target, stage, file, cancel)?;
        }
        progress(SnapshotProgress {
            phase: "verifying".into(),
            completed_objects: total,
            total_objects: total,
        });
        self.verify_directory(stage, id, cancel)?;
        Ok(())
    }

    fn copy_file(
        &self,
        root: &Path,
        source: &Path,
        target_root: &Path,
        target: &Path,
        work_root: &Path,
        file: &SnapshotFile,
        cancel: &AtomicBool,
    ) -> Result<(), ServiceError> {
        safe_io::ensure_dir(target_root, target.parent().ok_or_else(incomplete)?)?;
        if fs::symlink_metadata(target).is_ok() {
            return self.verify_file(target_root, target, file, cancel);
        }
        safe_io::check(root, source).map_err(source_error)?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let mut input = options
            .open(source)
            .map_err(safe_io::io)
            .map_err(source_error)?;
        if !input.metadata().map_err(safe_io::io)?.is_file() {
            return Err(incomplete());
        }
        safe_io::check(work_root, work_root)?;
        let temporary = work_root.join(format!(".pending-{}", uuid::Uuid::new_v4()));
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(safe_io::io)?;
        let mut buffer = [0u8; 65536];
        loop {
            cancelled(cancel)?;
            let size = input.read(&mut buffer).map_err(safe_io::io)?;
            if size == 0 {
                break;
            }
            safe_io::write_snapshot_bytes(&mut output, &buffer[..size]).map_err(safe_io::io)?;
        }
        output.sync_all().map_err(safe_io::io)?;
        drop(output);
        drop(input);
        self.verify_file(work_root, &temporary, file, cancel)?;
        safe_io::check(target_root, target.parent().ok_or_else(incomplete)?)?;
        match fs::hard_link(&temporary, target) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(safe_io::io(e)),
        }
        self.verify_file(target_root, target, file, cancel)?;
        fs::remove_file(&temporary).map_err(safe_io::io)?;
        Ok(())
    }

    fn verify_file(
        &self,
        root: &Path,
        path: &Path,
        file: &SnapshotFile,
        cancel: &AtomicBool,
    ) -> Result<(), ServiceError> {
        let actual = digest_file(root, path, cancel).map_err(|failure| {
            if failure.code == ErrorCode::OperationCancelled {
                failure
            } else {
                incomplete()
            }
        })?;
        if actual != (file.content_hash.clone(), file.byte_length) {
            return Err(incomplete());
        }
        Ok(())
    }

    fn summary(envelope: &Envelope) -> Result<Snapshot, ServiceError> {
        let manifest = &envelope.manifest;
        let total_bytes = manifest
            .files
            .iter()
            .try_fold(0u64, |total, file| total.checked_add(file.byte_length))
            .ok_or_else(incomplete)?;
        Ok(Snapshot {
            snapshot_id: manifest.snapshot_id.clone(),
            vault_id: manifest.vault_id.clone(),
            schema_version: manifest.schema_version,
            manifest_hash: envelope.checksum.clone(),
            created_at: manifest.created_at.clone(),
            object_count: manifest.files.len() as u64,
            total_bytes,
            completion_state: "complete".into(),
        })
    }
    fn verify_directory(
        &self,
        directory: &Path,
        id: &str,
        cancel: &AtomicBool,
    ) -> Result<Snapshot, ServiceError> {
        let envelope = self.envelope(directory, id)?;
        for file in &envelope.manifest.files {
            self.verify_file(
                directory,
                &directory.join("objects").join(&file.content_hash),
                file,
                cancel,
            )?;
        }
        let metadata = envelope
            .manifest
            .files
            .iter()
            .find(|file| file.relative_path == ".opennote/vault.json")
            .ok_or_else(incomplete)?;
        let vault: serde_json::Value = serde_json::from_slice(
            &safe_io::read(
                directory,
                &directory.join("objects").join(&metadata.content_hash),
            )
            .map_err(|_| incomplete())?,
        )
        .map_err(|_| incomplete())?;
        if vault.get("vaultId").and_then(|value| value.as_str()) != Some(self.vault_id.as_str())
            || vault.get("schemaVersion").and_then(|value| value.as_u64())
                != Some(envelope.manifest.schema_version.into())
        {
            return Err(incomplete());
        }
        Self::summary(&envelope)
    }

    fn resume_stage(
        &self,
        stage: &Path,
        id: &str,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(SnapshotProgress),
    ) -> Result<Snapshot, ServiceError> {
        self.copy_stage(stage, id, cancel, progress)?;
        self.publish_stage(stage, id, cancel)
    }
    fn publish_stage(
        &self,
        stage: &Path,
        id: &str,
        cancel: &AtomicBool,
    ) -> Result<Snapshot, ServiceError> {
        let snapshot = self.verify_directory(stage, id, cancel)?;
        cancelled(cancel)?;
        let complete = self.directory(id)?;
        if fs::symlink_metadata(&complete).is_ok() {
            return self.verify(id);
        }
        self.reserve_publication(&snapshot)?;
        cancelled(cancel)?;
        safe_io::check(&self.destination, stage)?;
        fs::rename(stage, &complete).map_err(safe_io::io)?;
        Ok(snapshot)
    }
    pub fn verify(&self, id: &str) -> Result<Snapshot, ServiceError> {
        self.not_removed(id)?;
        self.verify_directory(&self.directory(id)?, id, &AtomicBool::new(false))
    }
    pub fn plan_restore(
        &self,
        id: &str,
        target: &Path,
        cancel: &AtomicBool,
    ) -> Result<SnapshotRestorePlan, ServiceError> {
        self.plan_restore_inner(id, target, cancel, None)
    }
    pub(super) fn plan_restore_inner(
        &self,
        id: &str,
        target: &Path,
        cancel: &AtomicBool,
        allowed_reserved_target: Option<&Path>,
    ) -> Result<SnapshotRestorePlan, ServiceError> {
        self.not_removed(id)?;
        if !target.is_absolute()
            || target
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(error(ErrorCode::InvalidInput, "error.restoreTarget"));
        }
        let parent = external_directory(
            target
                .parent()
                .ok_or_else(|| error(ErrorCode::InvalidInput, "error.restoreTarget"))?,
        )?;
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| error(ErrorCode::InvalidInput, "error.restoreTarget"))?;
        if !valid_relative(name) || name.encode_utf16().count() > 120 {
            return Err(error(ErrorCode::InvalidInput, "error.restoreTarget"));
        }
        let target = parent.join(name);
        if (target.starts_with(&self.source) || target.starts_with(&self.destination))
            && allowed_reserved_target != Some(target.as_path())
        {
            return Err(error(ErrorCode::InvalidInput, "error.restoreRecursive"));
        }
        match fs::symlink_metadata(&target) {
            Ok(_) => return Err(error(ErrorCode::PathCollision, "error.restoreTargetExists")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(safe_io::io(e)),
        }
        let directory = self.directory(id)?;
        let snapshot = self.verify_directory(&directory, id, cancel)?;
        let envelope = self.envelope(&directory, id)?;
        let reference_report = self.reference_report(&directory, &envelope.manifest, cancel)?;
        let plan_id = hash(
            &serde_json::to_vec(&(&snapshot.manifest_hash, &target)).map_err(|_| incomplete())?,
        );
        Ok(SnapshotRestorePlan {
            plan_id,
            snapshot,
            target,
            note_count: envelope.manifest.notes.len() as u64,
            reference_report,
        })
    }
    pub fn list(&self) -> Result<Vec<Snapshot>, ServiceError> {
        self.list_with_cancel(&AtomicBool::new(false), |_| {})
    }
    pub fn list_with_cancel(
        &self,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<Vec<Snapshot>, ServiceError> {
        cancelled(cancel)?;
        let mut result = Vec::new();
        safe_io::check(&self.destination, &self.destination)?;
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            cancelled(cancel)?;
            let name = entry.map_err(safe_io::io)?.file_name();
            if let Some(id) = name.to_str().filter(|id| canonical_uuid(id)) {
                if self.not_removed(id).is_err() {
                    continue;
                }
                match self.verify_directory(&self.directory(id)?, id, cancel) {
                    Ok(snapshot) => result.push(snapshot),
                    Err(error) if error.code == ErrorCode::OperationCancelled => return Err(error),
                    Err(_) => (),
                }
                progress(SnapshotProgress {
                    phase: "listing".into(),
                    completed_objects: result.len() as u64,
                    total_objects: 0,
                });
            }
        }
        result.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.snapshot_id.cmp(&b.snapshot_id))
        });
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_manifest_validation_rejects_unsafe_paths_independently_of_checksum() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "test").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        backup
            .create(&storage, &id, &AtomicBool::new(false), |_| {})
            .unwrap();
        let mut envelope = backup
            .envelope(&backup.directory(&id).unwrap(), &id)
            .unwrap();
        for path in [
            "../outside",
            "AUX.png",
            "COM¹.txt",
            ".OPENNOTE/credential",
            "folder/a:b",
            "foo\\bar",
            "a.",
            "/outside",
        ] {
            envelope.manifest.files.push(SnapshotFile {
                relative_path: path.into(),
                content_hash: "a".repeat(64),
                byte_length: 0,
            });
            assert_eq!(
                backup
                    .validate_manifest(&envelope.manifest, &id)
                    .unwrap_err()
                    .code,
                ErrorCode::BackupIncomplete,
                "{path}"
            );
            envelope.manifest.files.pop();
        }
        for paths in [["Ä.md", "ä.md"], ["folder", "folder/file.md"]] {
            for path in paths {
                envelope.manifest.files.push(SnapshotFile {
                    relative_path: path.into(),
                    content_hash: "a".repeat(64),
                    byte_length: 0,
                });
            }
            assert_eq!(
                backup
                    .validate_manifest(&envelope.manifest, &id)
                    .unwrap_err()
                    .code,
                ErrorCode::BackupIncomplete
            );
            envelope
                .manifest
                .files
                .truncate(envelope.manifest.files.len() - 2);
        }
    }

    #[test]
    fn concurrent_same_operation_is_locked_without_modifying_existing_lock_bytes() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "test").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = destination.path().join(format!(".snapshot-lock-{id}"));
        fs::write(&path, b"sentinel").unwrap();
        let lock = backup.lock_operation(&id).unwrap();
        let failure = backup
            .create(&storage, &id, &AtomicBool::new(false), |_| {})
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::FileLocked);
        assert!(failure.retryable);
        drop(lock);
        assert_eq!(fs::read(path).unwrap(), b"sentinel");
        backup
            .create(&storage, &id, &AtomicBool::new(false), |_| {})
            .unwrap();
    }
}
