use crate::{error, hash};
use onote_domain::{ErrorCode, ServiceError};
use std::{fs, io::Write, path::Path};

pub(crate) fn io(error_value: std::io::Error) -> ServiceError {
    #[cfg(windows)]
    if matches!(error_value.raw_os_error(), Some(32 | 33)) {
        let mut failure = error(ErrorCode::FileLocked, "error.historyStorage");
        failure.retryable = true;
        return failure;
    }
    error(
        match error_value.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::HistoryMissing,
            std::io::ErrorKind::StorageFull => ErrorCode::DiskFull,
            _ => ErrorCode::IoDenied,
        },
        "error.historyStorage",
    )
}

pub(crate) fn write_snapshot_bytes(file: &mut fs::File, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(remaining) = SNAPSHOT_WRITE_BUDGET.with(|value| value.get()) {
        let length = bytes.len().min(remaining);
        file.write_all(&bytes[..length])?;
        SNAPSHOT_WRITE_BUDGET.with(|value| value.set(Some(remaining - length)));
        if length < bytes.len() {
            return Err(std::io::ErrorKind::StorageFull.into());
        }
        return Ok(());
    }
    file.write_all(bytes)
}

#[cfg(test)]
thread_local! { static SNAPSHOT_WRITE_BUDGET: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }

#[cfg(test)]
mod snapshot_write_tests {
    use super::*;
    use crate::BackupStore;
    use onote_vault_storage::VaultStorage;
    use std::sync::atomic::AtomicBool;

    struct Budget(Option<usize>);
    impl Budget {
        fn new(bytes: usize) -> Self {
            Self(SNAPSHOT_WRITE_BUDGET.with(|value| value.replace(Some(bytes))))
        }
    }
    impl Drop for Budget {
        fn drop(&mut self) {
            SNAPSHOT_WRITE_BUDGET.with(|value| value.set(self.0));
        }
    }
    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[test]
    fn partial_snapshot_and_restore_writes_report_disk_full_and_preserve_prior_success() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(source.path(), "write fixture").unwrap();
        let note = storage.create_note("a.md", "body", &op()).unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        let first = op();
        let complete = backup.create(&storage, &first, &cancel, |_| {}).unwrap();
        let second = op();
        {
            let _budget = Budget::new(17);
            assert_eq!(
                backup
                    .create(&storage, &second, &cancel, |_| {})
                    .unwrap_err()
                    .code,
                ErrorCode::DiskFull
            );
        }
        assert!(!destination.path().join(&second).exists());
        let objects = destination.path().join(format!(".staging-{second}"));
        assert!(fs::read_dir(&objects).unwrap().any(|entry| entry
            .unwrap()
            .metadata()
            .unwrap()
            .len()
            == 17));
        assert_eq!(backup.verify(&first).unwrap(), complete);
        let target = parent.path().join("restored");
        let operation = op();
        let plan = backup.plan_restore(&first, &target, &cancel).unwrap();
        {
            let _budget = Budget::new(5);
            assert_eq!(
                backup
                    .apply_restore(&plan, &operation, &cancel, |_| {})
                    .unwrap_err()
                    .code,
                ErrorCode::DiskFull
            );
        }
        assert!(!target.exists());
        assert_eq!(backup.verify(&first).unwrap(), complete);
        assert_eq!(
            fs::read(source.path().join("a.md")).unwrap(),
            note.source.as_bytes()
        );
        let result = backup
            .apply_restore(&plan, &operation, &cancel, |_| {})
            .unwrap();
        assert_eq!(result.note_count, 1);
        assert_eq!(
            fs::read(target.join("a.md")).unwrap(),
            note.source.as_bytes()
        );
    }
}

fn reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

pub(crate) fn check(root: &Path, target: &Path) -> Result<(), ServiceError> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| error(ErrorCode::InvalidInput, "error.pathEscape"))?;
    let mut current = root.to_path_buf();
    let root_metadata = fs::symlink_metadata(root).map_err(io)?;
    if reparse(&root_metadata) {
        return Err(error(ErrorCode::IoDenied, "error.historyReparse"));
    }
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            return Err(error(ErrorCode::InvalidInput, "error.pathEscape"));
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(io)?;
        if reparse(&metadata) {
            return Err(error(ErrorCode::IoDenied, "error.historyReparse"));
        }
        if current != target && !metadata.is_dir() {
            return Err(error(ErrorCode::IoDenied, "error.historyParent"));
        }
    }
    Ok(())
}

pub(crate) fn ensure_dir(root: &Path, target: &Path) -> Result<(), ServiceError> {
    check(root, root)?;
    if !fs::symlink_metadata(root).map_err(io)?.is_dir() {
        return Err(error(ErrorCode::IoDenied, "error.historyParent"));
    }
    let relative = target
        .strip_prefix(root)
        .map_err(|_| error(ErrorCode::InvalidInput, "error.pathEscape"))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            return Err(error(ErrorCode::InvalidInput, "error.pathEscape"));
        };
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(io(e)),
        }
        check(root, &current)?;
        if !fs::symlink_metadata(&current).map_err(io)?.is_dir() {
            return Err(error(ErrorCode::IoDenied, "error.historyParent"));
        }
    }
    Ok(())
}

pub(crate) fn read(root: &Path, path: &Path) -> Result<Vec<u8>, ServiceError> {
    check(root, path)?;
    if !fs::symlink_metadata(path).map_err(io)?.is_file() {
        return Err(error(ErrorCode::HistoryMissing, "error.historyInvalidFile"));
    }
    fs::read(path).map_err(io)
}

pub(crate) fn publish(root: &Path, target: &Path, bytes: &[u8]) -> Result<(), ServiceError> {
    let parent = target
        .parent()
        .ok_or_else(|| error(ErrorCode::InvalidInput, "error.pathEscape"))?;
    ensure_dir(root, parent)?;
    let temporary = parent.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    drop(file);
    check(root, &temporary)?;
    check(root, parent)?;
    match fs::hard_link(&temporary, target) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(io(e)),
    }
    if read(root, target)? != bytes {
        return Err(error(ErrorCode::HistoryMissing, "error.historyCollision"));
    }
    if read(root, &temporary).is_ok_and(|actual| hash(&actual) == hash(bytes)) {
        let _ = fs::remove_file(&temporary);
    }
    Ok(())
}

pub(crate) fn replace(root: &Path, target: &Path, bytes: &[u8]) -> Result<(), ServiceError> {
    let parent = target
        .parent()
        .ok_or_else(|| error(ErrorCode::InvalidInput, "error.pathEscape"))?;
    ensure_dir(root, parent)?;
    let temporary = parent.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    drop(file);
    check(root, &temporary)?;
    check(root, parent)?;
    if target.exists() {
        check(root, target)?;
        if !fs::symlink_metadata(target).map_err(io)?.is_file() {
            return Err(error(ErrorCode::HistoryMissing, "error.historyInvalidFile"));
        }
    }
    fs::rename(&temporary, target).map_err(io)?;
    if read(root, target)? != bytes {
        return Err(error(ErrorCode::HistoryMissing, "error.historyCollision"));
    }
    Ok(())
}
