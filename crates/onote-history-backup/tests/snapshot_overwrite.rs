use onote_domain::{ErrorCode, Snapshot};
use onote_history_backup::{
    BackupStore, SnapshotOverwritePlan, SnapshotPreservationReason, SnapshotReferenceIssueReason,
};
use onote_vault_storage::VaultStorage;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn create_vault(root: &Path, name: &str) -> VaultStorage {
    if !root.exists() {
        fs::create_dir(root).unwrap();
    }
    VaultStorage::create(root, name).unwrap()
}

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&source_path, &target_path);
        } else {
            fs::copy(source_path, target_path).unwrap();
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestHash<'a> {
    format_version: u32,
    operation_id: &'a str,
    source: &'a Path,
    destination: &'a Path,
    vault_id: &'a str,
    plan: &'a SnapshotOverwritePlan,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    request_hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record<T> {
    value: T,
    checksum: String,
}

fn publish_marker(destination: &Path, plan: &SnapshotOverwritePlan, name: &str) {
    let destination = fs::canonicalize(destination).unwrap();
    let request = RequestHash {
        format_version: 1,
        operation_id: &plan.operation_id,
        source: &plan.source,
        destination: &destination,
        vault_id: &plan.restore.snapshot.vault_id,
        plan,
    };
    let request_hash = hash(&serde_json::to_vec(&request).unwrap());
    let value = Marker { request_hash };
    let checksum = hash(&serde_json::to_vec(&value).unwrap());
    fs::write(
        destination
            .join(format!(".overwrite-journal-{}", plan.operation_id))
            .join(name),
        serde_json::to_vec(&Record { value, checksum }).unwrap(),
    )
    .unwrap();
}

fn selected_snapshot(vault: &mut VaultStorage, backup: &BackupStore) -> Snapshot {
    backup
        .create(vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap()
}

fn assert_note_contains(root: &Path, note_id: &str, text: &str) {
    let storage = VaultStorage::open(root).unwrap();
    assert!(storage.read(note_id).unwrap().source.contains(text));
}

#[test]
fn overwrite_restores_selected_snapshot_advances_revisions_and_retains_previous_root() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "overwrite");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();

    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    let safety = plan.safety_snapshot.clone();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);

    let result = backup
        .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(
        fs::canonicalize(&result.target).unwrap(),
        fs::canonicalize(source.path()).unwrap()
    );
    assert_eq!(result.note_count, 1);
    assert_eq!(result.snapshot_id, selected.snapshot_id);
    assert_eq!(result.safety_snapshot_id, safety.snapshot_id);
    assert_eq!(
        fs::canonicalize(&result.previous_root).unwrap(),
        fs::canonicalize(&plan.previous_root).unwrap()
    );

    let restored = VaultStorage::open(source.path()).unwrap();
    let document = restored.read(&note.note_id).unwrap();
    assert!(document.source.contains("selected"));
    assert_eq!(document.local_revision, 1);
    assert_eq!(restored.vault().schema_version, 3);
    assert!(restored.vault().writable);
    assert!(!restored.vault().sync_enabled);
    drop(restored);

    let previous = VaultStorage::open(&result.previous_root).unwrap();
    let document = previous.read(&note.note_id).unwrap();
    assert!(document.source.contains("current"));
    assert_eq!(document.local_revision, 1);
    assert_eq!(backup.verify(&selected.snapshot_id).unwrap(), selected);
    assert_eq!(backup.verify(&safety.snapshot_id).unwrap(), safety);
}

#[test]
fn schema_one_source_is_rejected_before_any_overwrite_state_is_created() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "legacy");
    storage.create_note("legacy.md", "legacy", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    drop(storage);

    let metadata_path = source.path().join(".opennote/vault.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["schemaVersion"] = 1.into();
    fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let before = fs::read(&metadata_path).unwrap();
    let storage = VaultStorage::open(source.path()).unwrap();
    let operation = op();
    assert_eq!(
        backup
            .plan_overwrite(
                &storage,
                &selected.snapshot_id,
                &operation,
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedSchema
    );
    assert_eq!(fs::read(&metadata_path).unwrap(), before);
    assert!(!source
        .path()
        .join(format!(".opennote-overwrite-new-{operation}"))
        .exists());
    assert!(!source
        .path()
        .join(format!(".opennote-overwrite-old-{operation}"))
        .exists());
    assert_eq!(backup.list().unwrap(), vec![selected]);
}

#[test]
fn source_changes_after_preview_are_rejected_without_publishing_a_stage() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "changed");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    let current = storage.read(&note.note_id).unwrap();
    storage
        .save(
            &note.note_id,
            "changed again",
            current.local_revision,
            &op(),
        )
        .unwrap();
    assert_eq!(
        backup
            .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {},)
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    assert!(storage
        .read(&note.note_id)
        .unwrap()
        .source
        .contains("changed again"));
    assert!(!plan.restore.target.exists());
    assert!(!plan.previous_root.exists());
}

#[test]
fn exact_stage_previous_and_journal_collisions_preserve_unrelated_data() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "collision");
    storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    let journal = destination
        .path()
        .join(format!(".overwrite-journal-{operation}"));
    for (path, value) in [
        (&plan.restore.target, b"stage".as_slice()),
        (&plan.previous_root, b"previous".as_slice()),
        (&journal, b"journal".as_slice()),
    ] {
        fs::create_dir(path).unwrap();
        fs::write(path.join("sentinel"), value).unwrap();
    }
    assert_eq!(
        backup
            .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {},)
            .unwrap_err()
            .code,
        ErrorCode::PathCollision
    );
    assert_eq!(
        fs::read(plan.restore.target.join("sentinel")).unwrap(),
        b"stage"
    );
    assert_eq!(
        fs::read(plan.previous_root.join("sentinel")).unwrap(),
        b"previous"
    );
    assert_eq!(fs::read(journal.join("sentinel")).unwrap(), b"journal");
}

#[test]
fn resumes_when_staging_is_complete_and_the_source_was_already_moved() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "resume");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);
    fs::rename(source.path(), &plan.previous_root).unwrap();

    let result = backup
        .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_note_contains(source.path(), &note.note_id, "selected");
    assert_note_contains(&result.previous_root, &note.note_id, "current");
}

#[test]
fn resumes_after_source_moved_marker_with_stage_and_previous_root_intact() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "marker resume");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);
    fs::rename(source.path(), &plan.previous_root).unwrap();
    publish_marker(destination.path(), &plan, "source-moved");

    let result = backup
        .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_note_contains(source.path(), &note.note_id, "selected");
    assert_note_contains(&result.previous_root, &note.note_id, "current");
}

#[test]
fn unmarked_new_source_and_previous_root_fail_without_overwriting_either() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "unmarked swap");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);
    fs::rename(source.path(), &plan.previous_root).unwrap();
    fs::rename(&plan.restore.target, source.path()).unwrap();

    assert_eq!(
        backup
            .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
            .unwrap_err()
            .code,
        ErrorCode::IoDenied
    );
    assert_note_contains(source.path(), &note.note_id, "selected");
    assert_note_contains(&plan.previous_root, &note.note_id, "current");
    assert!(!destination
        .path()
        .join(format!(".overwrite-journal-{operation}/published.json"))
        .exists());
}

#[test]
fn unmarked_source_previous_and_stage_are_all_preserved() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "ambiguous");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);
    copy_tree(source.path(), &plan.previous_root);

    assert_eq!(
        backup
            .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
            .unwrap_err()
            .code,
        ErrorCode::IoDenied
    );
    assert_note_contains(source.path(), &note.note_id, "current");
    assert_note_contains(&plan.previous_root, &note.note_id, "current");
    assert_note_contains(&plan.restore.target, &note.note_id, "selected");
}

#[test]
fn published_receipt_replay_returns_the_same_result_without_touching_source() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "replay");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);
    let result = backup
        .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    let bytes = fs::read(source.path().join("note.md")).unwrap();

    assert_eq!(
        backup
            .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
            .unwrap(),
        result
    );
    assert_eq!(fs::read(source.path().join("note.md")).unwrap(), bytes);
    assert_eq!(
        VaultStorage::open(source.path())
            .unwrap()
            .read(&note.note_id)
            .unwrap()
            .local_revision,
        1
    );
}

#[test]
fn wrong_source_or_operation_cannot_reuse_a_pinned_overwrite_plan() {
    let source = tempfile::tempdir().unwrap();
    let other_source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "scope");
    storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();

    let other = create_vault(other_source.path(), "other");
    let wrong_backup = BackupStore::open(
        other_source.path(),
        &storage.vault().vault_id,
        destination.path(),
    )
    .unwrap();
    assert_eq!(
        wrong_backup
            .stage_overwrite(&other, &plan, &AtomicBool::new(false), |_| {},)
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );

    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);
    let mut wrong_plan = plan.clone();
    wrong_plan.operation_id = op();
    assert!(backup
        .commit_overwrite(&wrong_plan, &AtomicBool::new(false), |_| {})
        .is_err());
    assert!(fs::read_to_string(source.path().join("note.md"))
        .unwrap()
        .contains("selected"));
    backup
        .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
        .unwrap();
}

#[test]
fn retention_and_cleanup_protect_pending_selected_and_safety_snapshots() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "retention");
    storage.create_note("note.md", "selected", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    let operation = op();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &operation,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    let safety = plan.safety_snapshot.clone();
    for _ in 0..3 {
        selected_snapshot(&mut storage, &backup);
    }
    let cancel = AtomicBool::new(false);
    let retention = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    for protected in [&selected, &safety] {
        assert!(retention.preserved.iter().any(|item| {
            item.snapshot_id == protected.snapshot_id
                && item.reason == SnapshotPreservationReason::ProtectedDependency
        }));
        assert!(!retention
            .candidates
            .iter()
            .any(|item| item.snapshot_id == protected.snapshot_id));
    }
    let result = backup
        .apply_cleanup(&retention, &op(), &cancel, |_| {})
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(backup.verify(&selected.snapshot_id).unwrap(), selected);
    assert_eq!(backup.verify(&safety.snapshot_id).unwrap(), safety);
}

#[test]
fn overwrite_preview_surfaces_missing_and_unsafe_reference_issues() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = create_vault(source.path(), "references");
    storage
        .create_note(
            "notes.md",
            "[remote](https://example.com/file)\n\
             ![missing](missing.png)\n\
             ![escape](../../outside.png)",
            &op(),
        )
        .unwrap();
    let backup =
        BackupStore::open(source.path(), &storage.vault().vault_id, destination.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &op(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    assert!(!plan.restore.reference_report.complete);
    assert_eq!(
        plan.restore
            .reference_report
            .issues
            .iter()
            .map(|issue| (issue.reference.as_str(), issue.reason))
            .collect::<Vec<_>>(),
        vec![
            ("../../outside.png", SnapshotReferenceIssueReason::Unsafe),
            ("missing.png", SnapshotReferenceIssueReason::Missing),
        ]
    );
}

#[test]
fn destination_can_be_the_source_parent_without_colliding_with_overwrite_work() {
    let container = tempfile::tempdir().unwrap();
    let source = container.path().join("vault");
    let mut storage = create_vault(&source, "parent destination");
    let note = storage.create_note("note.md", "selected", &op()).unwrap();
    let backup = BackupStore::open(&source, &storage.vault().vault_id, container.path()).unwrap();
    let selected = selected_snapshot(&mut storage, &backup);
    storage
        .save(&note.note_id, "current", note.local_revision, &op())
        .unwrap();
    let plan = backup
        .plan_overwrite(
            &storage,
            &selected.snapshot_id,
            &op(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    backup
        .stage_overwrite(&storage, &plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    drop(storage);
    let result = backup
        .commit_overwrite(&plan, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(
        fs::canonicalize(&result.target).unwrap(),
        fs::canonicalize(&source).unwrap()
    );
    assert_note_contains(&source, &note.note_id, "selected");
    assert!(PathBuf::from(&result.previous_root).exists());
}
