use onote_domain::ErrorCode;
use onote_history_backup::{BackupStore, HistoryStore};
use onote_vault_storage::{RevisionArchive, VaultStorage};
use std::{
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn legacy_readonly_vault_can_publish_and_verify_a_pre_upgrade_snapshot() {
    let (source, _destination, mut vault, backup) = fixture();
    let note = vault.create_note("legacy.md", "original", &op()).unwrap();
    drop(vault);
    let meta_path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    meta["custom"] = serde_json::json!({"original":true});
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let before_meta = fs::read(&meta_path).unwrap();
    let before_db = fs::read(source.path().join(".opennote/state.sqlite")).unwrap();
    let readonly = VaultStorage::open(source.path()).unwrap();
    let operation = op();
    let snapshot = backup
        .create(&readonly, &operation, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(snapshot.schema_version, 1);
    assert_eq!(snapshot.completion_state, "complete");
    assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
    assert_eq!(
        backup
            .create(&readonly, &operation, &AtomicBool::new(false), |_| {})
            .unwrap(),
        snapshot
    );
    assert_eq!(fs::read(meta_path).unwrap(), before_meta);
    assert_eq!(
        fs::read(source.path().join(".opennote/state.sqlite")).unwrap(),
        before_db
    );
    assert_eq!(readonly.read(&note.note_id).unwrap().source, note.source);
    assert!(!readonly.vault().writable);
}

fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    VaultStorage,
    BackupStore,
) {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "中文库").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    (source, destination, vault, backup)
}

#[test]
fn listing_cancellation_is_not_reported_as_success_and_keeps_snapshots_readable() {
    let (_source, _destination, mut vault, backup) = fixture();
    vault.create_note("a.md", "body", &op()).unwrap();
    let first = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let second = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let cancel = AtomicBool::new(false);
    let failure = backup
        .list_with_cancel(&cancel, |_| cancel.store(true, Ordering::Relaxed))
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::OperationCancelled);
    assert_eq!(backup.verify(&first.snapshot_id).unwrap(), first);
    assert_eq!(backup.verify(&second.snapshot_id).unwrap(), second);
    assert_eq!(backup.list().unwrap().len(), 2);
}

#[test]
fn snapshot_captures_notes_binary_assets_history_and_consistent_state_then_replays_original_result()
{
    let (source, destination, mut vault, backup) = fixture();
    let history = Arc::new(HistoryStore::open(source.path(), &vault.vault().vault_id).unwrap());
    vault.set_revision_archive(history.clone());
    let note = vault
        .create_note("中文.md", "---\ncustom: 保留\n---\n# 笔记", &op())
        .unwrap();
    history.force_preserve(&note).unwrap();
    fs::create_dir(source.path().join("attachments")).unwrap();
    fs::write(source.path().join("attachments/图像.bin"), [0, 255, 13, 10]).unwrap();
    fs::write(
        source.path().join(".opennote/index.sqlite"),
        b"not authoritative",
    )
    .unwrap();
    let operation = op();
    let cancel = AtomicBool::new(false);
    let result = backup.create(&vault, &operation, &cancel, |_| {}).unwrap();
    assert_eq!(result.completion_state, "complete");
    assert_eq!(result.snapshot_id, operation);
    assert!(result.object_count >= 5);
    assert_eq!(backup.verify(&operation).unwrap(), result);
    assert_eq!(backup.list().unwrap(), vec![result.clone()]);
    vault.save(&note.note_id, "later", 0, &op()).unwrap();
    assert_eq!(
        backup.create(&vault, &operation, &cancel, |_| {}).unwrap(),
        result
    );
    let manifest =
        fs::read_to_string(destination.path().join(&operation).join("manifest.json")).unwrap();
    assert!(!manifest.contains("index.sqlite"));
    assert!(!manifest.contains("writer.lock"));
    assert!(manifest.contains("attachments/图像.bin"));
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    for (path, bytes) in [
        ("attachments/图像.bin", vec![0, 255, 13, 10]),
        ("中文.md", note.source.into_bytes()),
    ] {
        let file = manifest["manifest"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["relativePath"] == path)
            .unwrap();
        let blob = destination
            .path()
            .join(&operation)
            .join("objects")
            .join(file["contentHash"].as_str().unwrap());
        assert_eq!(fs::read(blob).unwrap(), bytes);
    }
}

#[test]
fn missing_or_damaged_object_and_forged_manifest_never_verify_complete() {
    let (_source, destination, mut vault, backup) = fixture();
    vault.create_note("a.md", "body", &op()).unwrap();
    let operation = op();
    backup
        .create(&vault, &operation, &AtomicBool::new(false), |_| {})
        .unwrap();
    let directory = destination.path().join(&operation);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
    let asset = manifest["manifest"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["relativePath"] == "a.md")
        .unwrap();
    let blob = directory
        .join("objects")
        .join(asset["contentHash"].as_str().unwrap());
    let bytes = fs::read(&blob).unwrap();
    fs::remove_file(&blob).unwrap();
    assert_eq!(
        backup.verify(&operation).unwrap_err().code,
        ErrorCode::BackupIncomplete
    );
    fs::write(&blob, &bytes).unwrap();
    fs::write(&blob, b"corrupt").unwrap();
    assert_eq!(
        backup.verify(&operation).unwrap_err().code,
        ErrorCode::BackupIncomplete
    );
    fs::write(&blob, bytes).unwrap();
    fs::write(directory.join("manifest.json"), b"{}").unwrap();
    assert_eq!(
        backup.verify(&operation).unwrap_err().code,
        ErrorCode::BackupIncomplete
    );
}

#[test]
fn cancelled_snapshot_resumes_its_frozen_manifest_and_refuses_changed_uncopied_content() {
    let (source, _destination, mut vault, backup) = fixture();
    let note = vault.create_note("a.md", "old", &op()).unwrap();
    let operation = op();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        backup
            .create(&vault, &operation, &cancel, |progress| {
                if progress.phase == "copying" {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    fs::write(source.path().join("a.md"), b"changed").unwrap();
    cancel.store(false, Ordering::Relaxed);
    assert_eq!(
        backup
            .create(&vault, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::BackupIncomplete
    );
    assert!(backup.list().unwrap().is_empty());
    fs::write(source.path().join("a.md"), note.source).unwrap();
    let resumed = backup.create(&vault, &operation, &cancel, |_| {}).unwrap();
    assert_eq!(resumed.snapshot_id, operation);
    assert_eq!(backup.list().unwrap(), vec![resumed]);
}

#[test]
fn cancellation_and_failed_new_snapshot_keep_last_success_and_do_not_publish_partial_directory() {
    let (source, destination, mut vault, backup) = fixture();
    vault.create_note("a.md", "body", &op()).unwrap();
    let complete = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let interrupted = op();
    let cancel = AtomicBool::new(false);
    let failure = backup
        .create(&vault, &interrupted, &cancel, |progress| {
            if progress.phase == "copying" {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::OperationCancelled);
    assert!(!destination.path().join(&interrupted).exists());
    assert_eq!(backup.list().unwrap(), vec![complete.clone()]);
    assert_eq!(backup.verify(&complete.snapshot_id).unwrap(), complete);
    fs::write(source.path().join("a.md"), b"external change").unwrap();
    let failure = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::VersionConflict);
    assert_eq!(backup.list().unwrap().len(), 1);
}

#[test]
fn nested_destination_invalid_ids_and_unexpected_staging_preserve_existing_files() {
    let (source, destination, vault, backup) = fixture();
    let nested = source.path().join("backup");
    fs::create_dir(&nested).unwrap();
    assert_eq!(
        BackupStore::open(source.path(), &vault.vault().vault_id, &nested)
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        backup
            .create(&vault, "../../escape", &AtomicBool::new(false), |_| {})
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    let operation = op();
    let stage = destination.path().join(format!(".staging-{operation}"));
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("sentinel"), b"foreign").unwrap();
    assert_eq!(
        backup
            .create(&vault, &operation, &AtomicBool::new(false), |_| {})
            .unwrap_err()
            .code,
        ErrorCode::BackupIncomplete
    );
    assert_eq!(fs::read(stage.join("sentinel")).unwrap(), b"foreign");
}

#[test]
fn new_directory_restore_preview_verifies_all_objects_without_creating_or_overwriting_target() {
    let (_source, destination, mut vault, backup) = fixture();
    vault.create_note("a.md", "body", &op()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("恢复的知识库");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(plan.snapshot, snapshot);
    assert_eq!(plan.note_count, 1);
    assert_eq!(
        plan.target,
        fs::canonicalize(parent.path())
            .unwrap()
            .join("恢复的知识库")
    );
    assert_eq!(
        plan,
        backup
            .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
            .unwrap()
    );
    assert!(!target.exists());
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel"), b"keep").unwrap();
    assert_eq!(
        backup
            .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ErrorCode::PathCollision
    );
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"keep");
    let manifest = destination
        .path()
        .join(&snapshot.snapshot_id)
        .join("manifest.json");
    fs::write(manifest, b"{}").unwrap();
    assert_eq!(
        backup
            .plan_restore(
                &snapshot.snapshot_id,
                &parent.path().join("another"),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
        ErrorCode::BackupIncomplete
    );
    assert!(!parent.path().join("another").exists());
}
