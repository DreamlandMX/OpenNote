use super::*;

// Estimates include allocation/manifest overhead, but cannot reserve space against other writers.
fn allocation(bytes: u64) -> Result<u64, ServiceError> {
    bytes
        .checked_add(4095)
        .and_then(|n| (n / 4096 * 4096).checked_add(4096))
        .ok_or_else(incomplete)
}
fn required(sizes: impl IntoIterator<Item = u64>) -> Result<u64, ServiceError> {
    sizes
        .into_iter()
        .try_fold(8 * 1024 * 1024u64, |sum, bytes| {
            sum.checked_add(allocation(bytes)?).ok_or_else(incomplete)
        })
}
fn available(path: &Path) -> Result<u64, ServiceError> {
    #[cfg(test)]
    if let Some(value) = TEST_AVAILABLE.with(|value| value.get()) {
        return Ok(value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let path = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut bytes = 0;
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut bytes,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(safe_io::io(std::io::Error::last_os_error()));
        }
        Ok(bytes)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(error(
            ErrorCode::NotImplemented,
            "error.backupSpacePlatform",
        ))
    }
}
pub(super) fn check(path: &Path, sizes: impl IntoIterator<Item = u64>) -> Result<(), ServiceError> {
    let available_bytes = available(path)?;
    let required_bytes = required(sizes)?;
    if available_bytes < required_bytes {
        let mut failure = error(ErrorCode::DiskFull, "error.backupSpace");
        failure
            .params
            .insert("availableBytes".into(), available_bytes.to_string());
        failure
            .params
            .insert("requiredBytes".into(), required_bytes.to_string());
        failure.retryable = true;
        return Err(failure);
    }
    Ok(())
}

impl BackupStore {
    pub fn destination_space(&self) -> Result<(u64, String), ServiceError> {
        Ok((available(&self.destination)?, Utc::now().to_rfc3339()))
    }
    pub(super) fn check_new_snapshot_space(&self, cancel: &AtomicBool) -> Result<(), ServiceError> {
        let mut sizes = Vec::new();
        for relative in inventory(&self.source, cancel)? {
            cancelled(cancel)?;
            let path = self.source.join(relative);
            safe_io::check(&self.source, &path)?;
            sizes.push(fs::metadata(path).map_err(safe_io::io)?.len());
        }
        // The exported SQLite file and its object copy both occupy destination space.
        for name in ["state.sqlite", "state.sqlite-wal"] {
            let path = self.source.join(".opennote").join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    safe_io::check(&self.source, &path)?;
                    if !metadata.is_file() {
                        return Err(incomplete());
                    }
                    sizes.extend([metadata.len(), metadata.len()]);
                }
                Err(e) if name.ends_with("-wal") && e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(safe_io::io(e)),
            }
        }
        check(&self.destination, sizes)
    }
    pub(super) fn check_copy_space(
        &self,
        target: &Path,
        work: &Path,
        files: &[SnapshotFile],
        restore: bool,
    ) -> Result<(), ServiceError> {
        let mut sizes = Vec::new();
        let mut seen = HashSet::new();
        for file in files {
            let path = if restore {
                target.join(&file.relative_path)
            } else {
                target.join("objects").join(&file.content_hash)
            };
            if !restore && !seen.insert(file.content_hash.as_str()) {
                continue;
            }
            match fs::symlink_metadata(&path) {
                Ok(_) => safe_io::check(work, &path)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => sizes.push(file.byte_length),
                Err(e) => return Err(safe_io::io(e)),
            }
            if restore && file.relative_path == ".opennote/state.sqlite" {
                // Revision transactions may need a rollback journal and database growth.
                sizes.extend([file.byte_length, file.byte_length]);
            }
        }
        check(work, sizes)
    }
}

#[cfg(test)]
thread_local! { static TEST_AVAILABLE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
struct TestSpace(Option<u64>);
#[cfg(test)]
impl TestSpace {
    fn new(bytes: u64) -> Self {
        Self(TEST_AVAILABLE.with(|value| value.replace(Some(bytes))))
    }
}
#[cfg(test)]
impl Drop for TestSpace {
    fn drop(&mut self) {
        TEST_AVAILABLE.with(|value| value.set(self.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_preview_low_space_keeps_source_and_verified_backup_unchanged() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "upgrade space").unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let original = fs::read(&path).unwrap();
        let storage = VaultStorage::open(source.path()).unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        let snapshot = backup
            .create(&storage, &uuid::Uuid::new_v4().to_string(), &cancel, |_| {})
            .unwrap();
        let plan = backup
            .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
            .unwrap();
        let _space = TestSpace::new(0);
        assert_eq!(
            backup
                .apply_upgrade(
                    &storage,
                    &plan,
                    &uuid::Uuid::new_v4().to_string(),
                    &cancel,
                    |_| {}
                )
                .unwrap_err()
                .code,
            ErrorCode::DiskFull
        );
        assert!(!source.path().join(".opennote/upgrade-v1-v2").exists());
        assert_eq!(
            backup
                .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
                .unwrap_err()
                .code,
            ErrorCode::DiskFull
        );
        assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
        assert_eq!(fs::read(path).unwrap(), original);
        assert!(fs::read_dir(destination.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".upgrade-preview-")));
    }

    #[test]
    fn space_estimate_accounts_for_small_files_and_rejects_overflow() {
        assert!(required([1; 10000]).unwrap() > 80_000_000);
        assert_eq!(
            required([u64::MAX]).unwrap_err().code,
            ErrorCode::BackupIncomplete
        );
        let _space = TestSpace::new(0);
        assert_eq!(
            check(Path::new("unused"), [1]).unwrap_err().code,
            ErrorCode::DiskFull
        );
    }

    #[test]
    fn low_space_refuses_new_snapshot_before_staging_and_does_not_block_old_result_replay() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(source.path(), "space fixture").unwrap();
        storage
            .create_note("a.md", "body", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = AtomicBool::new(false);
        let complete = backup.create(&storage, &id, &cancel, |_| {}).unwrap();
        let operation = uuid::Uuid::new_v4().to_string();
        let _space = TestSpace::new(0);
        assert_eq!(
            backup
                .create(&storage, &operation, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::DiskFull
        );
        assert!(!destination
            .path()
            .join(format!(".staging-{operation}"))
            .exists());
        assert!(!destination.path().join(&operation).exists());
        assert_eq!(backup.verify(&id).unwrap(), complete);
        assert_eq!(
            backup.create(&storage, &id, &cancel, |_| {}).unwrap(),
            complete
        );
    }

    #[test]
    fn low_restore_space_keeps_preview_target_absent_and_replays_a_completed_restore() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "restore space fixture").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = AtomicBool::new(false);
        backup.create(&storage, &id, &cancel, |_| {}).unwrap();
        let finished_plan = backup
            .plan_restore(&id, &parent.path().join("finished"), &cancel)
            .unwrap();
        let finished_op = uuid::Uuid::new_v4().to_string();
        let result = backup
            .apply_restore(&finished_plan, &finished_op, &cancel, |_| {})
            .unwrap();
        let plan = backup
            .plan_restore(&id, &parent.path().join("new-target"), &cancel)
            .unwrap();
        let operation = uuid::Uuid::new_v4().to_string();
        let _space = TestSpace::new(0);
        assert_eq!(
            backup
                .apply_restore(&plan, &operation, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::DiskFull
        );
        assert!(!plan.target.exists());
        assert!(!destination
            .path()
            .join(format!(".restore-journal-{operation}"))
            .exists());
        assert!(!parent
            .path()
            .join(format!(".opennote-restore-{operation}"))
            .exists());
        assert_eq!(
            backup
                .apply_restore(&finished_plan, &finished_op, &cancel, |_| {})
                .unwrap(),
            result
        );
    }
}
