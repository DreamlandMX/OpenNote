use onote_domain::ErrorCode;
use onote_history_backup::BackupStore;
use onote_vault_storage::VaultStorage;
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
};
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn confirmed_cleanup_is_scoped_idempotent_and_cannot_recreate_removed_operations() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(source.path(), "cleanup").unwrap();
    let note = storage.create_note("a.md", "original", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..3 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    let operation = op();
    let result = backup
        .apply_cleanup(&plan, &operation, &cancel, |_| {})
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.removed.len(), 2);
    assert!(result.remaining.is_empty() && result.quarantined.is_empty());
    assert_eq!(backup.list().unwrap(), plan.retained);
    assert_eq!(
        fs::read(source.path().join("a.md")).unwrap(),
        note.source.as_bytes()
    );
    assert_eq!(
        backup
            .apply_cleanup(&plan, &operation, &AtomicBool::new(true), |_| {})
            .unwrap(),
        result
    );
    fs::remove_file(
        destination
            .path()
            .join(format!(".cleanup-journal-{operation}"))
            .join("result.json"),
    )
    .unwrap();
    assert_eq!(
        backup
            .resume_cleanup(&operation, &AtomicBool::new(true), |_| {})
            .unwrap(),
        result
    );
    for removed in &result.removed {
        assert_eq!(
            backup
                .create(&storage, &removed.snapshot_id, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::ObjectNotFound
        );
        assert_eq!(
            backup.verify(&removed.snapshot_id).unwrap_err().code,
            ErrorCode::ObjectNotFound
        );
    }
    assert_eq!(
        backup
            .apply_cleanup(
                &backup.plan_retention(2, &cancel, |_| {}).unwrap(),
                &operation,
                &cancel,
                |_| {}
            )
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        backup
            .create(&storage, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    let before = fs::read_dir(destination.path()).unwrap().count();
    assert_eq!(
        backup
            .run_daily(&storage, &operation, 1, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), before);
}

#[test]
fn completed_conditional_capture_replays_after_its_old_baseline_is_cleaned() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "conditional replay").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let first = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    fs::write(source.path().join("new.bin"), b"changed").unwrap();
    let operation = op();
    let second = backup
        .create_if_changed(
            &storage,
            &operation,
            Some(&first.snapshot_id),
            &cancel,
            |_| {},
        )
        .unwrap();
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    backup.apply_cleanup(&plan, &op(), &cancel, |_| {}).unwrap();
    assert_eq!(
        backup
            .create_if_changed(
                &storage,
                &operation,
                Some(&first.snapshot_id),
                &cancel,
                |_| {}
            )
            .unwrap(),
        second
    );
}

#[test]
fn incomplete_capture_and_restore_keep_their_baseline_outside_expired_candidates() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "dependencies").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let first = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let pending = op();
    let _ = backup.create_if_changed(&storage, &pending, Some(&first.snapshot_id), &cancel, |p| {
        if p.phase == "planning" {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    cancel.store(false, Ordering::Relaxed);
    let second = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let restore = backup
        .plan_restore(
            &second.snapshot_id,
            &target.path().join("restored"),
            &cancel,
        )
        .unwrap();
    let _ = backup.apply_restore(&restore, &op(), &cancel, |p| {
        if p.phase == "restoring" {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    cancel.store(false, Ordering::Relaxed);
    let last = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    assert_eq!(plan.retained, vec![last]);
    assert!(plan.candidates.is_empty());
    for snapshot in [first, second] {
        assert!(plan
            .preserved
            .iter()
            .any(|p| p.snapshot_id == snapshot.snapshot_id));
        assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
    }
}

#[test]
fn changed_preview_and_cancel_before_commit_preserve_every_success() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "stale").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..2 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let old = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let before = backup.list().unwrap();
    let operation = op();
    assert_eq!(
        backup
            .apply_cleanup(&old, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    assert!(!destination
        .path()
        .join(format!(".cleanup-journal-{operation}"))
        .exists());
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    assert_eq!(
        backup
            .apply_cleanup(&plan, &op(), &AtomicBool::new(true), |_| {})
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    assert_eq!(backup.list().unwrap(), before);
}

#[test]
fn crash_during_deletion_resumes_original_journal_after_restart() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "crash").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..3 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    let operation = op();
    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = backup.apply_cleanup(&plan, &operation, &cancel, |p| {
            if p.phase == "cleaning" && p.completed_objects > 0 {
                panic!("simulated process interruption");
            }
        });
    }));
    assert!(stopped.is_err());
    drop(backup);
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    assert_eq!(
        backup.verify(&plan.retained[0].snapshot_id).unwrap(),
        plan.retained[0]
    );
    let pending = backup.pending_cleanups().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, operation);
    assert_eq!(pending[0].plan, plan);
    let new_plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    assert_eq!(
        backup
            .apply_cleanup(&new_plan, &op(), &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::FileLocked
    );
    let result = backup.resume_cleanup(&operation, &cancel, |_| {}).unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.removed, plan.candidates);
    assert_eq!(backup.list().unwrap(), plan.retained);
    assert!(backup.pending_cleanups().unwrap().is_empty());
}

#[test]
fn cancellation_reports_partial_work_and_retains_quarantine_without_claiming_rollback() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "cancel").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..3 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    let operation = op();
    let result = backup
        .apply_cleanup(&plan, &operation, &cancel, |p| {
            if p.phase == "cleaning" {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    assert_eq!(result.status, "cancelled");
    assert!(result.removed.is_empty());
    assert_eq!(result.quarantined.len(), 1);
    assert_eq!(result.remaining.len(), 1);
    assert_eq!(
        backup.verify(&plan.retained[0].snapshot_id).unwrap(),
        plan.retained[0]
    );
    assert_eq!(
        backup
            .apply_cleanup(&plan, &operation, &cancel, |_| {})
            .unwrap(),
        result
    );
    let moved = destination
        .path()
        .join(format!(".cleanup-journal-{operation}"))
        .join(&result.quarantined[0].snapshot_id);
    assert!(moved.is_dir());
}

#[test]
fn cleanup_excludes_unknown_files_and_serializes_with_new_captures() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "unknown data").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let first = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    fs::write(
        destination
            .path()
            .join(&first.snapshot_id)
            .join("unrelated.bin"),
        b"do not remove",
    )
    .unwrap();
    for _ in 0..3 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    assert!(plan
        .preserved
        .iter()
        .any(|p| p.snapshot_id == first.snapshot_id));
    assert_eq!(plan.candidates.len(), 2);
    let mut attempted = false;
    let target = tempfile::tempdir().unwrap();
    let restore = backup
        .plan_restore(
            &plan.retained[0].snapshot_id,
            &target.path().join("restore"),
            &cancel,
        )
        .unwrap();
    let result = backup
        .apply_cleanup(&plan, &op(), &cancel, |p| {
            if p.phase == "cleaning" && !attempted {
                attempted = true;
                assert_eq!(
                    backup
                        .create(&storage, &op(), &cancel, |_| {})
                        .unwrap_err()
                        .code,
                    ErrorCode::FileLocked
                );
                assert_eq!(
                    backup
                        .create_if_changed(&storage, &op(), None, &cancel, |_| {})
                        .unwrap_err()
                        .code,
                    ErrorCode::FileLocked
                );
                assert_eq!(
                    backup
                        .run_daily(&storage, &op(), 1, &cancel, |_| {})
                        .unwrap_err()
                        .code,
                    ErrorCode::FileLocked
                );
                assert_eq!(
                    backup
                        .apply_restore(&restore, &op(), &cancel, |_| {})
                        .unwrap_err()
                        .code,
                    ErrorCode::FileLocked
                );
            }
        })
        .unwrap();
    assert!(attempted);
    assert_eq!(result.status, "completed");
    assert_eq!(
        fs::read(
            destination
                .path()
                .join(&first.snapshot_id)
                .join("unrelated.bin")
        )
        .unwrap(),
        b"do not remove"
    );
    assert_eq!(backup.verify(&first.snapshot_id).unwrap(), first);
}
