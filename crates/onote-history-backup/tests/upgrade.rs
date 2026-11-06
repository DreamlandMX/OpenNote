use onote_history_backup::BackupStore;
use onote_vault_storage::VaultStorage;
use std::{fs, sync::atomic::AtomicBool};

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn schema_two_upgrade_preserves_legacy_receipts_and_exact_note_bytes() {
    use onote_vault_storage::Failpoint;
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let restored = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(source.path(), "schema two").unwrap();
    let note = storage
        .create_note(
            "legacy.md",
            "\u{feff}---\r\ncustom: unchanged\r\n---\r\nbody\r\n",
            &op(),
        )
        .unwrap();
    drop(storage);
    let path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let storage = VaultStorage::open(source.path()).unwrap();
    let legacy_op = op();
    let legacy_manifest = "a".repeat(64);
    let original_receipt = storage
        .apply_schema_upgrade_to(
            &destination.path().join("old.sqlite"),
            &legacy_op,
            &legacy_manifest,
            2,
            |_, _, _, _| Ok(()),
        )
        .unwrap();
    assert_eq!(original_receipt.to_schema, 2);
    drop(storage);
    let storage = VaultStorage::open(source.path()).unwrap();
    assert_eq!(storage.vault().schema_version, 2);
    assert!(!storage.vault().writable);
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let snapshot = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let restore_plan = backup
        .plan_restore(
            &snapshot.snapshot_id,
            &restored.path().join("schema-two"),
            &cancel,
        )
        .unwrap();
    backup
        .apply_restore(&restore_plan, &op(), &cancel, |_| {})
        .unwrap();
    let readonly = VaultStorage::open(restored.path().join("schema-two")).unwrap();
    assert_eq!(readonly.vault().schema_version, 2);
    assert!(!readonly.vault().writable);
    let plan = backup
        .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
        .unwrap();
    assert_eq!((plan.from_schema, plan.to_schema), (2, 3));
    let operation = op();
    storage.push_failpoint(Failpoint::UpgradeMetadata);
    assert!(backup
        .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
        .is_err());
    drop(storage);
    let storage = VaultStorage::open(source.path()).unwrap();
    assert!(!storage.vault().writable);
    let result = backup
        .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
        .unwrap();
    assert_eq!((result.from_schema, result.to_schema), (2, 3));
    drop(storage);
    let storage = VaultStorage::open(source.path()).unwrap();
    assert!(storage.vault().writable);
    assert_eq!(storage.vault().schema_version, 3);
    assert_eq!(storage.read(&note.note_id).unwrap().source, note.source);
    assert_eq!(
        storage
            .completed_schema_upgrade(&legacy_op, &legacy_manifest)
            .unwrap(),
        Some(original_receipt)
    );
    assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
}

#[test]
fn cancelling_upgrade_verification_keeps_source_unchanged_and_original_request_retryable() {
    use std::sync::atomic::Ordering;
    for phase in ["verifying", "finalizing"] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "cancel upgrade").unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let metadata = fs::read(&path).unwrap();
        let database = fs::read(source.path().join(".opennote/state.sqlite")).unwrap();
        let storage = VaultStorage::open(source.path()).unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        let snapshot = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        let plan = backup
            .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
            .unwrap();
        let operation = op();
        assert_eq!(
            backup
                .apply_upgrade(&storage, &plan, &operation, &cancel, |progress| {
                    if progress.phase == phase {
                        cancel.store(true, Ordering::Relaxed);
                    }
                })
                .unwrap_err()
                .code,
            onote_domain::ErrorCode::OperationCancelled
        );
        assert_eq!(fs::read(path).unwrap(), metadata);
        assert_eq!(
            fs::read(source.path().join(".opennote/state.sqlite")).unwrap(),
            database
        );
        assert!(!source.path().join(".opennote/upgrade-v1-v2").exists());
        assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(
            backup.pending_upgrades(&cancel).unwrap()[0].operation_id,
            operation
        );
        backup
            .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
            .unwrap();
        assert!(fs::read_dir(destination.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".upgrade-apply-")));
    }
}

#[test]
fn abandoning_an_unstarted_upgrade_releases_only_its_snapshot_dependency() {
    use onote_vault_storage::Failpoint;

    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "abandon upgrade").unwrap();
    drop(storage);
    let path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let metadata = fs::read(&path).unwrap();
    let database = fs::read(source.path().join(".opennote/state.sqlite")).unwrap();
    let mut storage = VaultStorage::open(source.path()).unwrap();
    storage.set_revision_archive(std::sync::Arc::new(
        onote_history_backup::HistoryStore::open(source.path(), &storage.vault().vault_id).unwrap(),
    ));
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let protected = backup
        .create(&storage, &op(), &cancel, |_| {})
        .unwrap_or_else(|failure| panic!("{failure:?}"));
    let plan = backup
        .plan_upgrade(&storage, &protected.snapshot_id, &cancel)
        .unwrap();
    let operation = op();
    assert_eq!(
        backup
            .apply_upgrade(&storage, &plan, &operation, &cancel, |progress| {
                if progress.phase == "verifying" {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            })
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::OperationCancelled
    );
    cancel.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        backup.pending_upgrades(&cancel).unwrap()[0].operation_id,
        operation
    );
    assert!(!destination
        .path()
        .join(format!(".upgrade-journal-{operation}/abandoned.json"))
        .exists());
    let clone = tempfile::tempdir().unwrap();
    fs::create_dir(clone.path().join(".opennote")).unwrap();
    for name in ["vault.json", "state.sqlite"] {
        fs::copy(
            source.path().join(".opennote").join(name),
            clone.path().join(".opennote").join(name),
        )
        .unwrap();
    }
    let cloned_storage = VaultStorage::open(clone.path()).unwrap();
    let cloned_backup =
        BackupStore::open(clone.path(), &storage.vault().vault_id, destination.path()).unwrap();
    backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    assert_eq!(
        cloned_backup
            .abandon_upgrade(&cloned_storage, &operation, &cancel)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::IdempotencyMismatch
    );
    assert_eq!(backup.pending_upgrades(&cancel).unwrap().len(), 1);
    assert!(backup
        .plan_retention(1, &cancel, |_| {})
        .unwrap()
        .preserved
        .iter()
        .any(|snapshot| snapshot.snapshot_id == protected.snapshot_id
            && snapshot.reason
                == onote_history_backup::SnapshotPreservationReason::ProtectedDependency));
    assert!(!destination
        .path()
        .join(format!(".upgrade-journal-{operation}/abandoned.json"))
        .exists());

    meta["schemaVersion"] = 2.into();
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    assert_eq!(
        backup
            .abandon_upgrade(&storage, &operation, &cancel)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::VersionConflict
    );
    assert_eq!(backup.pending_upgrades(&cancel).unwrap().len(), 1);
    fs::write(&path, &metadata).unwrap();
    let abandoned = backup
        .abandon_upgrade(&storage, &operation, &cancel)
        .unwrap();
    assert_eq!(abandoned.operation_id, operation);
    assert_eq!(abandoned.snapshot_id, protected.snapshot_id);
    assert_eq!(
        cloned_backup
            .abandon_upgrade(&cloned_storage, &operation, &cancel)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        backup
            .abandon_upgrade(&storage, &operation, &cancel)
            .unwrap(),
        abandoned
    );
    drop(storage);
    let reopened_storage = VaultStorage::open(source.path()).unwrap();
    let reopened_backup = BackupStore::open(
        source.path(),
        &reopened_storage.vault().vault_id,
        destination.path(),
    )
    .unwrap();
    assert_eq!(
        reopened_backup
            .abandon_upgrade(&reopened_storage, &operation, &cancel)
            .unwrap(),
        abandoned
    );
    drop(reopened_storage);
    let mut storage = VaultStorage::open(source.path()).unwrap();
    storage.set_revision_archive(std::sync::Arc::new(
        onote_history_backup::HistoryStore::open(source.path(), &storage.vault().vault_id).unwrap(),
    ));
    assert!(backup.pending_upgrades(&cancel).unwrap().is_empty());
    assert_eq!(
        backup
            .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::IdempotencyMismatch
    );
    let retention = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    assert!(!retention
        .preserved
        .iter()
        .any(|snapshot| snapshot.snapshot_id == protected.snapshot_id
            && snapshot.reason
                == onote_history_backup::SnapshotPreservationReason::ProtectedDependency));
    assert_eq!(backup.verify(&protected.snapshot_id).unwrap(), protected);
    assert_eq!(fs::read(&path).unwrap(), metadata);
    assert_eq!(
        fs::read(source.path().join(".opennote/state.sqlite")).unwrap(),
        database
    );
    assert!(!source.path().join(".opennote/upgrade-v1-v2").exists());

    let started_source = tempfile::tempdir().unwrap();
    let started_destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(started_source.path(), "started upgrade").unwrap();
    drop(storage);
    let path = started_source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let storage = VaultStorage::open(started_source.path()).unwrap();
    let backup = BackupStore::open(
        started_source.path(),
        &storage.vault().vault_id,
        started_destination.path(),
    )
    .unwrap();
    let protected = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let plan = backup
        .plan_upgrade(&storage, &protected.snapshot_id, &cancel)
        .unwrap();
    let operation = op();
    storage.push_failpoint(Failpoint::UpgradePrepared);
    assert!(backup
        .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
        .is_err());
    assert!(started_source
        .path()
        .join(".opennote/upgrade-v1-v2")
        .exists());
    assert_eq!(
        backup
            .abandon_upgrade(&storage, &operation, &cancel)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::VersionConflict
    );
    assert!(!started_destination
        .path()
        .join(format!(".upgrade-journal-{operation}/abandoned.json"))
        .exists());
    drop(storage);
    let reopened = VaultStorage::open(started_source.path()).unwrap();
    assert!(!reopened.vault().writable);
    assert_eq!(
        backup
            .abandon_upgrade(&reopened, &operation, &cancel)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::VersionConflict
    );
    assert_eq!(backup.pending_upgrades(&cancel).unwrap().len(), 1);
    assert_eq!(backup.verify(&protected.snapshot_id).unwrap(), protected);
}

#[test]
fn verified_upgrade_execution_resumes_and_protects_its_backup_from_cleanup() {
    use onote_vault_storage::Failpoint;
    for stop in [
        Failpoint::UpgradePrepared,
        Failpoint::UpgradeDatabase,
        Failpoint::UpgradeMetadata,
    ] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(source.path(), "verified migration").unwrap();
        let note = storage.create_note("a.md", "original", &op()).unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let storage = VaultStorage::open(source.path()).unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        let snapshot = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        let plan = backup
            .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
            .unwrap();
        let operation = op();
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        let stale_cleanup = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        assert!(backup
            .apply_upgrade(&storage, &plan, &snapshot.snapshot_id, &cancel, |_| {})
            .is_err());
        storage.push_failpoint(stop);
        assert!(backup
            .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
            .is_err());
        assert_eq!(
            backup.pending_upgrades(&cancel).unwrap()[0].operation_id,
            operation
        );
        let retention = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        assert!(retention
            .preserved
            .iter()
            .any(|s| s.snapshot_id == snapshot.snapshot_id
                && s.reason
                    == onote_history_backup::SnapshotPreservationReason::ProtectedDependency));
        assert!(backup
            .apply_cleanup(&stale_cleanup, &op(), &cancel, |_| {})
            .is_err());
        assert!(backup
            .create(&storage, &operation, &cancel, |_| {})
            .is_err());
        drop(storage);
        let storage = VaultStorage::open(source.path()).unwrap();
        assert!(!storage.vault().writable);
        fs::write(
            source.path().join("a.md"),
            b"external change while migration paused",
        )
        .unwrap();
        assert!(backup
            .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
            .is_err());
        assert_eq!(
            fs::read(source.path().join("a.md")).unwrap(),
            b"external change while migration paused"
        );
        fs::write(source.path().join("a.md"), note.source.as_bytes()).unwrap();
        let result = backup
            .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
            .unwrap();
        assert_eq!(result.to_schema, 3);
        assert_eq!(result.note_count, 1);
        assert!(backup.pending_upgrades(&cancel).unwrap().is_empty());
        fs::remove_file(
            destination
                .path()
                .join(format!(".upgrade-journal-{operation}/complete.json")),
        )
        .unwrap();
        assert_eq!(
            backup
                .apply_upgrade(&storage, &plan, &operation, &AtomicBool::new(true), |_| {})
                .unwrap(),
            result
        );
        assert_eq!(
            backup
                .apply_upgrade(&storage, &plan, &operation, &cancel, |_| {})
                .unwrap(),
            result
        );
        let reopened = VaultStorage::open(source.path()).unwrap();
        assert!(reopened.vault().writable);
        assert_eq!(reopened.read(&note.note_id).unwrap().source, note.source);
        assert_eq!(
            reopened.read(&note.note_id).unwrap().local_revision,
            note.local_revision
        );
        assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
    }
}

#[test]
fn completed_upgrade_journal_is_backed_up_and_remains_valid_after_new_directory_restore() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(source.path(), "upgrade journal").unwrap();
    let note = storage.create_note("a.md", "original", &op()).unwrap();
    drop(storage);
    let path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let storage = VaultStorage::open(source.path()).unwrap();
    storage
        .apply_schema_upgrade(
            &destination.path().join("export.sqlite"),
            &op(),
            &"a".repeat(64),
            |_, _, _, _| Ok(()),
        )
        .unwrap();
    drop(storage);
    let storage = VaultStorage::open(source.path()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let snapshot = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let restore = backup
        .plan_restore(
            &snapshot.snapshot_id,
            &parent.path().join("restored"),
            &cancel,
        )
        .unwrap();
    backup
        .apply_restore(&restore, &op(), &cancel, |_| {})
        .unwrap();
    for name in ["request.json", "complete"] {
        let relative = format!(".opennote/upgrade-v1-v2/{name}");
        assert_eq!(
            fs::read(source.path().join(&relative)).unwrap(),
            fs::read(restore.target.join(&relative)).unwrap()
        );
    }
    let restored = VaultStorage::open(&restore.target).unwrap();
    assert!(restored.vault().writable);
    assert_eq!(restored.read(&note.note_id).unwrap().source, note.source);
    assert_eq!(
        restored.read(&note.note_id).unwrap().local_revision,
        note.local_revision + 1
    );
}

#[test]
fn upgrade_preview_requires_verified_snapshot_of_the_exact_current_source() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(source.path(), "upgrade").unwrap();
    let note = storage.create_note("a.md", "original", &op()).unwrap();
    drop(storage);
    let path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    fs::write(source.path().join("asset.bin"), [0, 255, 13, 10]).unwrap();
    let storage = VaultStorage::open(source.path()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let snapshot = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let before_meta = fs::read(&path).unwrap();
    let before_db = fs::read(source.path().join(".opennote/state.sqlite")).unwrap();
    let plan = backup
        .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
        .unwrap();
    assert_eq!(plan.snapshot, snapshot);
    assert_eq!(plan.note_count, 1);
    assert_eq!(plan.from_schema, 1);
    assert_eq!(plan.to_schema, 3);
    assert_eq!(
        backup
            .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
            .unwrap()
            .plan_id,
        plan.plan_id
    );
    assert_eq!(fs::read(&path).unwrap(), before_meta);
    assert_eq!(
        fs::read(source.path().join(".opennote/state.sqlite")).unwrap(),
        before_db
    );
    assert_eq!(storage.read(&note.note_id).unwrap().source, note.source);
    assert!(!storage.vault().writable);
    assert_eq!(
        backup
            .plan_upgrade(&storage, &snapshot.snapshot_id, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::OperationCancelled
    );
    assert!(backup.plan_upgrade(&storage, &op(), &cancel).is_err());
    let copy = tempfile::tempdir().unwrap();
    fs::create_dir(copy.path().join(".opennote")).unwrap();
    for relative in [
        ".opennote/vault.json",
        ".opennote/state.sqlite",
        "a.md",
        "asset.bin",
    ] {
        fs::copy(source.path().join(relative), copy.path().join(relative)).unwrap();
    }
    let copied = VaultStorage::open(copy.path()).unwrap();
    let copied_backup =
        BackupStore::open(copy.path(), &storage.vault().vault_id, destination.path()).unwrap();
    assert!(copied_backup
        .plan_upgrade(&copied, &snapshot.snapshot_id, &cancel)
        .is_err());
    fs::write(&path, b"{\"schemaVersion\":1}").unwrap();
    assert!(backup
        .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
        .is_err());
    fs::write(&path, &before_meta).unwrap();
    fs::write(source.path().join("a.md"), b"external note change").unwrap();
    assert!(backup
        .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
        .is_err());
    fs::write(source.path().join("a.md"), note.source.as_bytes()).unwrap();
    fs::write(source.path().join("asset.bin"), [1, 2, 3]).unwrap();
    assert!(backup
        .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
        .is_err());
    fs::write(source.path().join("asset.bin"), [0, 255, 13, 10]).unwrap();
    fs::write(source.path().join("new.bin"), [7]).unwrap();
    assert!(backup
        .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
        .is_err());
    fs::remove_file(source.path().join("new.bin")).unwrap();
    let database =
        rusqlite::Connection::open(source.path().join(".opennote/state.sqlite")).unwrap();
    database.execute_batch("CREATE TABLE new_pending_work(payload TEXT); INSERT INTO new_pending_work VALUES ('must back up first');").unwrap();
    assert!(backup
        .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
        .is_err());
    fs::write(
        destination
            .path()
            .join(&snapshot.snapshot_id)
            .join("objects")
            .join(&note.content_hash),
        b"corrupt backup object",
    )
    .unwrap();
    assert_eq!(
        backup
            .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::BackupIncomplete
    );
    assert!(fs::read_dir(destination.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".upgrade-preview-")));
}

#[test]
fn upgrade_preview_does_not_treat_current_format_as_a_legacy_migration() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "current format").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let snapshot = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    assert_eq!(
        backup
            .plan_upgrade(&storage, &snapshot.snapshot_id, &cancel)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::UnsupportedSchema
    );
    assert!(storage.vault().writable);
}
