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
fn copied_vault_identity_cannot_replay_another_sources_create_operation() {
    let source = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let original = VaultStorage::create(source.path(), "original").unwrap();
    let copied = VaultStorage::create(copy.path(), "copy").unwrap();
    drop(copied);
    let meta_path = copy.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["vaultId"] = original.vault().vault_id.clone().into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let copied = VaultStorage::open(copy.path()).unwrap();
    let operation = op();
    let cancel = AtomicBool::new(false);
    BackupStore::open(
        source.path(),
        &original.vault().vault_id,
        destination.path(),
    )
    .unwrap()
    .create(&original, &operation, &cancel, |_| {})
    .unwrap();
    let backup =
        BackupStore::open(copy.path(), &copied.vault().vault_id, destination.path()).unwrap();
    let original_store = BackupStore::open(
        source.path(),
        &original.vault().vault_id,
        destination.path(),
    )
    .unwrap();
    assert_eq!(
        original_store
            .create(&copied, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        backup
            .create(&copied, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
}

#[test]
fn unchanged_checks_do_not_publish_and_replay_original_result_after_later_edits() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "变化检查").unwrap();
    let note = vault.create_note("a.md", "原文", &op()).unwrap();
    fs::write(source.path().join("shared.bin"), [0, 255]).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let first_op = op();
    let first = backup
        .create_if_changed(&vault, &first_op, None, &cancel, |_| {})
        .unwrap();
    let check_op = op();
    assert_eq!(
        backup
            .create_if_changed(&vault, &check_op, Some(&first.snapshot_id), &cancel, |_| {})
            .unwrap(),
        first
    );
    assert!(!destination.path().join(&check_op).exists());
    assert!(!destination
        .path()
        .join(format!(".staging-{check_op}"))
        .exists());
    assert_eq!(
        backup
            .create_if_changed(
                &vault,
                &check_op,
                Some(&first.snapshot_id),
                &AtomicBool::new(true),
                |_| {}
            )
            .unwrap(),
        first
    );
    fs::write(source.path().join("shared.bin"), [0, 255]).unwrap();
    assert_eq!(
        backup
            .create_if_changed(&vault, &op(), Some(&first.snapshot_id), &cancel, |_| {})
            .unwrap(),
        first
    );
    drop(vault);
    let mut vault = VaultStorage::open(source.path()).unwrap();
    assert_eq!(
        backup
            .create_if_changed(&vault, &op(), Some(&first.snapshot_id), &cancel, |_| {})
            .unwrap(),
        first
    );
    vault.save(&note.note_id, "后来正文", 0, &op()).unwrap();
    assert_eq!(
        backup
            .create_if_changed(&vault, &check_op, Some(&first.snapshot_id), &cancel, |_| {})
            .unwrap(),
        first
    );
    assert_eq!(
        backup
            .create(&vault, &check_op, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    let changed_op = op();
    let changed = backup
        .create_if_changed(
            &vault,
            &changed_op,
            Some(&first.snapshot_id),
            &cancel,
            |_| {},
        )
        .unwrap();
    assert_eq!(changed.snapshot_id, changed_op);
    assert_eq!(
        backup
            .create_if_changed(
                &vault,
                &changed_op,
                Some(&first.snapshot_id),
                &AtomicBool::new(true),
                |_| {}
            )
            .unwrap(),
        changed
    );
    assert_eq!(backup.list().unwrap().len(), 2);
    assert_eq!(
        backup
            .create_if_changed(
                &vault,
                &check_op,
                Some(&changed.snapshot_id),
                &cancel,
                |_| {}
            )
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    fs::write(source.path().join("shared.bin"), [0, 254]).unwrap();
    let asset_op = op();
    assert_eq!(
        backup
            .create_if_changed(
                &vault,
                &asset_op,
                Some(&changed.snapshot_id),
                &cancel,
                |_| {}
            )
            .unwrap()
            .snapshot_id,
        asset_op
    );
}

#[test]
fn cancellation_or_missing_baseline_cannot_report_an_unchanged_success() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "test").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let first = backup.create(&vault, &op(), &cancel, |_| {}).unwrap();
    assert_eq!(
        backup
            .create_if_changed(&vault, &op(), Some(&op()), &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::BackupIncomplete
    );
    let check_op = op();
    let failure = backup
        .create_if_changed(
            &vault,
            &check_op,
            Some(&first.snapshot_id),
            &cancel,
            |progress| {
                if progress.phase == "planning" {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::OperationCancelled);
    assert!(!destination.path().join(&check_op).exists());
    assert_eq!(backup.verify(&first.snapshot_id).unwrap(), first);
}
