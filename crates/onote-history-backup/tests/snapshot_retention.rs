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
fn preview_keeps_latest_successes_and_never_changes_snapshot_files() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "retention").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..4 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let ordered = backup.list().unwrap();
    let before = fs::read_dir(destination.path()).unwrap().count();
    let plan = backup.plan_retention(2, &cancel, |_| {}).unwrap();
    assert_eq!(plan.retained, ordered[..2]);
    assert_eq!(plan.candidates, ordered[2..]);
    assert!(plan.preserved.is_empty());
    assert_eq!(
        plan.candidate_bytes,
        ordered[2..].iter().map(|s| s.total_bytes).sum::<u64>()
    );
    assert_eq!(plan, backup.plan_retention(2, &cancel, |_| {}).unwrap());
    assert_ne!(
        plan.plan_id,
        backup.plan_retention(3, &cancel, |_| {}).unwrap().plan_id
    );
    assert_eq!(backup.list().unwrap(), ordered);
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), before);
    backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    assert_ne!(
        plan.plan_id,
        backup.plan_retention(2, &cancel, |_| {}).unwrap().plan_id
    );
}

#[test]
fn another_source_cannot_select_snapshots_and_mid_inventory_cancel_is_honest() {
    let source = tempfile::tempdir().unwrap();
    let copied = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "original").unwrap();
    let copy = VaultStorage::create(copied.path(), "copy").unwrap();
    drop(copy);
    let meta_path = copied.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["vaultId"] = storage.vault().vault_id.clone().into();
    fs::write(meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..2 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let other =
        BackupStore::open(copied.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let plan = other.plan_retention(1, &cancel, |_| {}).unwrap();
    assert!(plan.retained.is_empty() && plan.candidates.is_empty());
    assert_eq!(plan.preserved.len(), 2);
    let original = backup.list().unwrap();
    let failure = backup
        .plan_retention(1, &cancel, |_| cancel.store(true, Ordering::Relaxed))
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::OperationCancelled);
    assert_eq!(backup.list().unwrap(), original);
}

#[test]
fn unscoped_damaged_and_staging_data_are_preserved_instead_of_cleanup_candidates() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "retention").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let cancel = AtomicBool::new(false);
    let legacy = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    fs::remove_file(
        destination
            .path()
            .join(format!(".snapshot-request-{}.json", legacy.snapshot_id)),
    )
    .unwrap();
    let damaged = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    fs::write(
        destination
            .path()
            .join(&damaged.snapshot_id)
            .join("manifest.json"),
        b"{}",
    )
    .unwrap();
    let staging = destination.path().join(format!(".staging-{}", op()));
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("keep.bin"), b"keep").unwrap();
    let good = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    assert_eq!(plan.retained, vec![good]);
    assert!(plan.candidates.is_empty());
    let preserved: Vec<_> = plan
        .preserved
        .iter()
        .map(|p| p.snapshot_id.as_str())
        .collect();
    assert!(preserved.contains(&legacy.snapshot_id.as_str()));
    assert!(preserved.contains(&damaged.snapshot_id.as_str()));
    assert_eq!(fs::read(staging.join("keep.bin")).unwrap(), b"keep");
}

#[test]
fn preview_rejects_zero_and_cancellation_without_writing() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let storage = VaultStorage::create(source.path(), "retention").unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    assert_eq!(
        backup
            .plan_retention(0, &AtomicBool::new(false), |_| {})
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        backup
            .plan_retention(7, &AtomicBool::new(true), |_| {})
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
}
