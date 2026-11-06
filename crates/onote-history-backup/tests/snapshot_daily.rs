use onote_domain::ErrorCode;
use onote_history_backup::{BackupStore, DailyBackupResult};
use onote_vault_storage::VaultStorage;
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
};
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn cancelling_before_capture_also_persists_the_automatic_deferral() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "early cancel fixture").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let cancel = AtomicBool::new(true);
    assert_eq!(
        backup
            .run_daily(&vault, &op(), now, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    cancel.store(false, Ordering::Relaxed);
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 1, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::NotDue { .. }
    ));
    assert!(backup.list().unwrap().is_empty());
}

#[test]
fn cancelling_during_baseline_inventory_is_not_silently_restarted_next_minute() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "inventory cancel fixture").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let now = chrono::Utc::now().timestamp() as u64 + 86400;
    let cancel = AtomicBool::new(false);
    let original = backup.create(&vault, &op(), &cancel, |_| {}).unwrap();
    assert_eq!(
        backup
            .run_daily(&vault, &op(), now, &cancel, |progress| {
                if progress.phase == "listing" {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    cancel.store(false, Ordering::Relaxed);
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 1, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::NotDue { .. }
    ));
    assert_eq!(backup.verify(&original.snapshot_id).unwrap(), original);
}

#[test]
fn daily_checks_persist_across_reopen_skip_unchanged_data_and_run_after_clock_rollback() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "daily fixture").unwrap();
    let note = vault.create_note("a.md", "body", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let cancel = AtomicBool::new(false);
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::Created { .. }
    ));
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 86399, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::NotDue { .. }
    ));
    drop(backup);
    drop(vault);
    let mut vault = VaultStorage::open(source.path()).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    // Snapshot creation may cross into the next whole second; avoid the exact daily boundary.
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 86405, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::Unchanged { .. }
    ));
    assert_eq!(backup.list().unwrap().len(), 1);
    vault.save(&note.note_id, "changed", 0, &op()).unwrap();
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 86405, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::NotDue { .. }
    ));
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::Created { .. }
    ));
    assert_eq!(backup.list().unwrap().len(), 2);
}

#[test]
fn a_new_manual_snapshot_replaces_failed_daily_work_without_removing_its_staging() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "manual recovery fixture").unwrap();
    fs::write(source.path().join("asset.bin"), b"original").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let cancel = AtomicBool::new(false);
    let operation = op();
    assert!(backup
        .run_daily(&vault, &operation, now, &cancel, |progress| {
            if progress.phase == "copying" && progress.completed_objects == 0 {
                fs::write(source.path().join("asset.bin"), b"later content").unwrap();
            }
        })
        .is_err());
    let manual = backup.create(&vault, &op(), &cancel, |_| {}).unwrap();
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 300, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::NotDue { .. }
    ));
    assert!(destination
        .path()
        .join(format!(".staging-{operation}"))
        .exists());
    assert_eq!(backup.list().unwrap(), vec![manual]);
}

#[test]
fn failed_daily_copy_retries_original_operation_after_backoff_and_keeps_staging() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "retry fixture").unwrap();
    fs::write(source.path().join("asset.bin"), b"original").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let cancel = AtomicBool::new(false);
    let operation = op();
    assert_eq!(
        backup
            .run_daily(&vault, &operation, now, &cancel, |progress| {
                if progress.phase == "copying" && progress.completed_objects == 0 {
                    fs::write(source.path().join("asset.bin"), b"changed").unwrap();
                }
            })
            .unwrap_err()
            .code,
        ErrorCode::BackupIncomplete
    );
    assert!(destination
        .path()
        .join(format!(".staging-{operation}"))
        .exists());
    assert!(backup.list().unwrap().is_empty());
    fs::write(source.path().join("asset.bin"), b"original").unwrap();
    drop(backup);
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 299, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::NotDue { .. }
    ));
    let DailyBackupResult::Created { snapshot, .. } = backup
        .run_daily(&vault, &op(), now + 300, &cancel, |_| {})
        .unwrap()
    else {
        panic!("expected resumed creation");
    };
    assert_eq!(snapshot.snapshot_id, operation);
    assert_eq!(backup.list().unwrap().len(), 1);
}

#[test]
fn cancelling_daily_work_retains_it_and_defers_automatic_work_until_next_day() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "cancel fixture").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let cancel = AtomicBool::new(false);
    let operation = op();
    assert_eq!(
        backup
            .run_daily(&vault, &operation, now, &cancel, |progress| {
                if progress.phase == "copying" {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    assert!(destination
        .path()
        .join(format!(".staging-{operation}"))
        .exists());
    cancel.store(false, Ordering::Relaxed);
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 1, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::NotDue { .. }
    ));
    assert!(matches!(
        backup
            .run_daily(&vault, &op(), now + 86400, &cancel, |_| {})
            .unwrap(),
        DailyBackupResult::Created { .. }
    ));
}
