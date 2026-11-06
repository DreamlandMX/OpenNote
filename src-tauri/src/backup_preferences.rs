use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::Component};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Preferences {
    format_version: u32,
    pub revision: u64,
    pub source: PathBuf,
    pub vault_id: String,
    pub destination: PathBuf,
    pub retained_snapshots: u32,
}
fn invalid() -> ServiceError {
    svc_err(ErrorCode::InvalidInput, "error.backupPreferencesInvalid")
}
fn io(error: std::io::Error) -> ServiceError {
    map_io_export(&error)
}
fn check(path: &Path) -> Result<(), ServiceError> {
    let mut current = path.to_path_buf();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err(invalid());
                    }
                }
                if meta.file_type().is_symlink() {
                    return Err(invalid());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io(e)),
        }
        if !current.pop() {
            break;
        }
    }
    Ok(())
}
fn directory(root: &Path, source: &Path, vault_id: &str) -> Result<PathBuf, ServiceError> {
    if !root.is_absolute()
        || !source.is_absolute()
        || !uuid::Uuid::parse_str(vault_id).is_ok_and(|id| id.to_string() == vault_id)
    {
        return Err(invalid());
    }
    let input = serde_json::to_vec(&(source, vault_id)).map_err(|_| invalid())?;
    let directory = root.join(format!("{:x}", Sha256::digest(input)));
    check(&directory)?;
    Ok(directory)
}
fn latest(directory: &Path) -> Result<u64, ServiceError> {
    check(directory)?;
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io(e)),
    };
    let mut revision = 0;
    for entry in entries {
        let name = entry.map_err(io)?.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(number) = name
            .strip_suffix(".json")
            .filter(|name| name.len() == 20 && name.bytes().all(|b| b.is_ascii_digit()))
        {
            revision = revision.max(number.parse().map_err(|_| invalid())?);
        }
    }
    Ok(revision)
}
fn read(
    directory: &Path,
    revision: u64,
    source: &Path,
    vault_id: &str,
) -> Result<Preferences, ServiceError> {
    let path = directory.join(format!("{revision:020}.json"));
    check(&path)?;
    if fs::metadata(&path).map_err(io)?.len() > 1024 * 1024 {
        return Err(invalid());
    }
    let value: Preferences =
        serde_json::from_slice(&fs::read(path).map_err(io)?).map_err(|_| invalid())?;
    if value.format_version != 1
        || value.revision != revision
        || value.source != source
        || value.vault_id != vault_id
        || value.retained_snapshots == 0
        || !value.destination.is_absolute()
        || value
            .destination
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid());
    }
    Ok(value)
}
pub(crate) fn load(
    root: &Path,
    source: &Path,
    vault_id: &str,
) -> Result<Option<Preferences>, ServiceError> {
    let directory = directory(root, source, vault_id)?;
    let revision = latest(&directory)?;
    if revision == 0 {
        return Ok(None);
    }
    read(&directory, revision, source, vault_id).map(Some)
}
pub(crate) fn choose(
    root: &Path,
    source: &Path,
    vault_id: &str,
    destination: &Path,
) -> Result<Preferences, ServiceError> {
    change(root, source, vault_id, Some(destination), None)
}

pub(crate) fn set_retention(
    root: &Path,
    source: &Path,
    vault_id: &str,
    expected_revision: u64,
    count: u32,
    expected_destination: &Path,
) -> Result<Preferences, ServiceError> {
    if count == 0 {
        return Err(invalid());
    }
    change(
        root,
        source,
        vault_id,
        None,
        Some((expected_revision, count, expected_destination)),
    )
}

fn lock_preferences(directory: &Path) -> Result<fs::File, ServiceError> {
    check(directory)?;
    let lock_path = directory.join("writer.lock");
    check(&lock_path)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(io)?;
    match lock.try_lock() {
        Ok(()) => (),
        Err(fs::TryLockError::WouldBlock) => {
            return Err(svc_err(
                ErrorCode::FileLocked,
                "error.backupPreferencesBusy",
            ))
        }
        Err(fs::TryLockError::Error(e)) => return Err(io(e)),
    }
    Ok(lock)
}

pub(crate) fn with_retention<T>(
    root: &Path,
    source: &Path,
    vault_id: &str,
    expected_revision: u64,
    count: u32,
    expected_destination: &Path,
    action: impl FnOnce() -> Result<T, ServiceError>,
) -> Result<T, ServiceError> {
    let directory = directory(root, source, vault_id)?;
    let _lock = lock_preferences(&directory)?;
    let revision = latest(&directory)?;
    if revision == 0 {
        return Err(invalid());
    }
    let value = read(&directory, revision, source, vault_id)?;
    if revision != expected_revision
        || value.retained_snapshots != count
        || value.destination != expected_destination
    {
        return Err(svc_err(
            ErrorCode::VersionConflict,
            "error.backupPreferencesInvalid",
        ));
    }
    action()
}

fn change(
    root: &Path,
    source: &Path,
    vault_id: &str,
    destination: Option<&Path>,
    retention: Option<(u64, u32, &Path)>,
) -> Result<Preferences, ServiceError> {
    if let Some(destination) = destination {
        if !destination.is_absolute()
            || destination
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(invalid());
        }
    }
    let directory = directory(root, source, vault_id)?;
    fs::create_dir_all(&directory).map_err(io)?;
    check(&directory)?;
    let _lock = lock_preferences(&directory)?;
    let revision = latest(&directory)?;
    // Explicitly selecting a destination can repair damaged preferences; old records stay intact.
    let previous = if revision > 0 {
        let value = read(&directory, revision, source, vault_id);
        if retention.is_some() {
            Some(value?)
        } else {
            value.ok()
        }
    } else {
        None
    };
    if let Some((expected, _, expected_destination)) = retention {
        if expected != revision
            || previous
                .as_ref()
                .is_none_or(|p| p.destination != expected_destination)
        {
            return Err(svc_err(
                ErrorCode::VersionConflict,
                "error.backupPreferencesInvalid",
            ));
        }
    }
    let destination = destination
        .or_else(|| previous.as_ref().map(|p| p.destination.as_path()))
        .ok_or_else(invalid)?;
    let retained_snapshots = retention
        .map(|(_, count, _)| count)
        .unwrap_or_else(|| previous.as_ref().map(|p| p.retained_snapshots).unwrap_or(7));
    if let Some(previous) = &previous {
        if previous.destination == destination && previous.retained_snapshots == retained_snapshots
        {
            return Ok(previous.clone());
        }
    }
    let value = Preferences {
        format_version: 1,
        revision: revision
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or_else(invalid)?,
        source: source.into(),
        vault_id: vault_id.into(),
        destination: destination.into(),
        retained_snapshots,
    };
    let bytes = serde_json::to_vec(&value).map_err(|_| invalid())?;
    let temporary = directory.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(io)?;
    output.write_all(&bytes).map_err(io)?;
    output.sync_all().map_err(io)?;
    drop(output);
    check(&temporary)?;
    check(&directory)?;
    fs::hard_link(
        &temporary,
        directory.join(format!("{:020}.json", value.revision)),
    )
    .map_err(io)?;
    // The published file is durable; removing our temporary hard link is optional cleanup.
    let _ = fs::remove_file(temporary);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_guard_rechecks_settings_and_holds_the_writer_lock_through_the_action() {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let saved = choose(root.path(), source.path(), &id, destination.path()).unwrap();
        let mut called = false;
        let failure = with_retention(
            root.path(),
            source.path(),
            &id,
            saved.revision + 1,
            saved.retained_snapshots,
            destination.path(),
            || {
                called = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, ErrorCode::VersionConflict);
        assert!(!called);
        with_retention(
            root.path(),
            source.path(),
            &id,
            saved.revision,
            saved.retained_snapshots,
            destination.path(),
            || {
                assert_eq!(
                    set_retention(
                        root.path(),
                        source.path(),
                        &id,
                        saved.revision,
                        2,
                        destination.path()
                    )
                    .unwrap_err()
                    .code,
                    ErrorCode::FileLocked
                );
                Ok(())
            },
        )
        .unwrap();
        set_retention(
            root.path(),
            source.path(),
            &id,
            saved.revision,
            2,
            destination.path(),
        )
        .unwrap();
    }
    #[test]
    fn retention_updates_are_revision_checked_and_never_create_a_destination() {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let initial = choose(root.path(), source.path(), &id, destination.path()).unwrap();
        assert_eq!(
            set_retention(
                root.path(),
                source.path(),
                &id,
                initial.revision,
                0,
                destination.path()
            )
            .unwrap_err()
            .code,
            ErrorCode::InvalidInput
        );
        let changed = set_retention(
            root.path(),
            source.path(),
            &id,
            initial.revision,
            3,
            destination.path(),
        )
        .unwrap();
        assert_eq!(changed.revision, initial.revision + 1);
        assert_eq!(changed.retained_snapshots, 3);
        assert_eq!(
            set_retention(
                root.path(),
                source.path(),
                &id,
                initial.revision,
                5,
                destination.path()
            )
            .unwrap_err()
            .code,
            ErrorCode::VersionConflict
        );
        assert_eq!(
            set_retention(
                root.path(),
                source.path(),
                &id,
                changed.revision,
                3,
                destination.path()
            )
            .unwrap(),
            changed
        );
        assert_eq!(
            load(root.path(), source.path(), &id).unwrap().unwrap(),
            changed
        );
        assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
    }
    #[test]
    fn selections_survive_restart_and_copied_vault_ids_are_separate() {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        assert!(load(root.path(), source.path(), &id).unwrap().is_none());
        let saved = choose(root.path(), source.path(), &id, destination.path()).unwrap();
        assert_eq!(saved.retained_snapshots, 7);
        assert_eq!(
            load(root.path(), source.path(), &id).unwrap(),
            Some(saved.clone())
        );
        assert_eq!(
            choose(root.path(), source.path(), &id, destination.path()).unwrap(),
            saved
        );
        assert!(load(root.path(), copy.path(), &id).unwrap().is_none());
    }
    #[test]
    fn corrupt_newest_record_never_silently_activates_older_settings_and_can_be_reselected() {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        choose(root.path(), source.path(), &id, destination.path()).unwrap();
        let folder = directory(root.path(), source.path(), &id).unwrap();
        let damaged = folder.join("00000000000000000002.json");
        fs::write(&damaged, b"damaged").unwrap();
        assert!(load(root.path(), source.path(), &id).is_err());
        let repaired = choose(root.path(), source.path(), &id, destination.path()).unwrap();
        assert_eq!(repaired.revision, 3);
        assert_eq!(fs::read(damaged).unwrap(), b"damaged");
        assert_eq!(
            load(root.path(), source.path(), &id).unwrap(),
            Some(repaired)
        );
    }
}
