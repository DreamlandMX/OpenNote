use onote_domain::ErrorCode;
use onote_history_backup::{BackupStore, SnapshotRestoreCancellation};
use onote_vault_storage::VaultStorage;
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
};
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn restores_new_directory_with_same_identity_new_revision_and_paused_sync_then_replays() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let target_parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let mut vault = VaultStorage::create(source.path(), "原始库").unwrap();
    let note = vault
        .create_note("中文.md", "---\ncustom: 保留\n---\n原正文", &op())
        .unwrap();
    fs::write(source.path().join("shared.pdf"), [0, 255, 1]).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let target = target_parent.path().join("恢复库");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    let operation = op();
    let result = backup
        .apply_restore(&plan, &operation, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(result.note_count, 1);
    assert_eq!(result.target, plan.target);
    let mut restored = VaultStorage::open(&target).unwrap();
    let document = restored.read(&note.note_id).unwrap();
    assert_eq!(document.source, note.source);
    assert_eq!(document.local_revision, 1);
    assert_eq!(fs::read(target.join("shared.pdf")).unwrap(), [0, 255, 1]);
    let meta: serde_json::Value =
        serde_json::from_slice(&fs::read(target.join(".opennote/vault.json")).unwrap()).unwrap();
    assert_eq!(meta["syncEnabled"], false);
    assert_eq!(meta["writable"], true);
    restored.save(&note.note_id, "后来编辑", 1, &op()).unwrap();
    drop(restored);
    assert_eq!(
        backup
            .apply_restore(&plan, &operation, &AtomicBool::new(false), |_| {})
            .unwrap(),
        result
    );
    assert!(fs::read_to_string(target.join("中文.md"))
        .unwrap()
        .contains("后来编辑"));
    assert_eq!(
        backup.cancel_restore(&operation).unwrap(),
        SnapshotRestoreCancellation::AlreadyApplied
    );
    assert_eq!(
        fs::read(source.path().join("中文.md")).unwrap(),
        note.source.as_bytes()
    );
}

#[test]
fn interrupted_restore_resumes_or_cancels_without_publishing_partial_content_or_overwriting_collision(
) {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "test").unwrap();
    let note = vault.create_note("a.md", "old", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let target = parent.path().join("restored");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    let operation = op();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        backup
            .apply_restore(&plan, &operation, &cancel, |progress| {
                if progress.phase == "finalizing" {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    assert!(!target.exists());
    cancel.store(false, Ordering::Relaxed);
    let result = backup
        .apply_restore(&plan, &operation, &cancel, |_| {})
        .unwrap();
    assert_eq!(result.note_count, 1);
    assert_eq!(
        VaultStorage::open(&target)
            .unwrap()
            .read(&note.note_id)
            .unwrap()
            .local_revision,
        1
    );
    let next = parent.path().join("cancelled");
    let plan2 = backup
        .plan_restore(&snapshot.snapshot_id, &next, &AtomicBool::new(false))
        .unwrap();
    let next_op = op();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        backup
            .apply_restore(&plan2, &next_op, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    cancel.store(false, Ordering::Relaxed);
    assert_eq!(
        backup
            .apply_restore(&plan2, &next_op, &cancel, |progress| {
                if progress.phase == "restoring" {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    assert_eq!(
        backup.cancel_restore(&next_op).unwrap(),
        SnapshotRestoreCancellation::Cancelled
    );
    cancel.store(false, Ordering::Relaxed);
    assert_eq!(
        backup
            .apply_restore(&plan2, &next_op, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    assert!(!next.exists());
    let collision = parent.path().join("occupied");
    let plan3 = backup
        .plan_restore(&snapshot.snapshot_id, &collision, &cancel)
        .unwrap();
    fs::create_dir(&collision).unwrap();
    fs::write(collision.join("sentinel"), b"keep").unwrap();
    assert_eq!(
        backup
            .apply_restore(&plan3, &op(), &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::PathCollision
    );
    assert_eq!(fs::read(collision.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn resumes_after_ready_receipt_and_after_directory_publication_without_reapplying_revisions() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "test").unwrap();
    let note = vault.create_note("a.md", "old", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let target = parent.path().join("ready");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    let operation = op();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        backup
            .apply_restore(&plan, &operation, &cancel, |p| {
                if p.phase == "publishing" {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    let journal = destination
        .path()
        .join(format!(".restore-journal-{operation}"));
    fs::remove_file(journal.join("ready.json")).unwrap();
    cancel.store(false, Ordering::Relaxed);
    let result = backup
        .apply_restore(&plan, &operation, &cancel, |_| {})
        .unwrap();
    assert_eq!(
        VaultStorage::open(&target)
            .unwrap()
            .read(&note.note_id)
            .unwrap()
            .local_revision,
        1
    );
    fs::remove_file(journal.join("published.json")).unwrap();
    assert_eq!(
        backup
            .apply_restore(&plan, &operation, &cancel, |_| {})
            .unwrap(),
        result
    );
    assert_eq!(
        VaultStorage::open(&target)
            .unwrap()
            .read(&note.note_id)
            .unwrap()
            .local_revision,
        1
    );
}

#[test]
fn changed_staged_content_is_preserved_and_cancellation_survives_a_foreign_target() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "test").unwrap();
    vault.create_note("a.md", "old", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let target = parent.path().join("ready");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    let operation = op();
    let cancel = AtomicBool::new(false);
    backup
        .apply_restore(&plan, &operation, &cancel, |p| {
            if p.phase == "publishing" {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap_err();
    cancel.store(false, Ordering::Relaxed);
    let staged = parent
        .path()
        .join(format!(".opennote-restore-{operation}/vault/a.md"));
    fs::write(&staged, b"external staged work").unwrap();
    assert_eq!(
        backup
            .apply_restore(&plan, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::RestoreInterrupted
    );
    assert_eq!(fs::read(&staged).unwrap(), b"external staged work");
    assert!(!target.exists());
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel"), b"foreign").unwrap();
    assert_eq!(
        backup.cancel_restore(&operation).unwrap(),
        SnapshotRestoreCancellation::Cancelled
    );
    assert_eq!(
        backup
            .apply_restore(&plan, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::OperationCancelled
    );
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"foreign");
    assert_eq!(fs::read(staged).unwrap(), b"external staged work");
}

#[test]
fn raw_readonly_notes_bom_crlf_and_unknown_metadata_survive_new_directory_restore() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "旧库").unwrap();
    drop(vault);
    let bytes = b"\xef\xbb\xbf---\r\nunknown: keep\r\n---\r\nbody\r\n";
    fs::write(source.path().join("legacy.md"), bytes).unwrap();
    fs::write(source.path().join("invalid.md"), [255, 254, 0, 13, 10]).unwrap();
    let meta_path = source.path().join(".opennote/vault.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    metadata["custom"] = "keep".into();
    metadata["syncEnabled"] = true.into();
    fs::write(meta_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let vault = VaultStorage::open(source.path()).unwrap();
    let baseline = vault.list().unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let target = parent.path().join("restored");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    backup
        .apply_restore(&plan, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(fs::read(target.join("legacy.md")).unwrap(), bytes);
    assert_eq!(
        fs::read(target.join("invalid.md")).unwrap(),
        [255, 254, 0, 13, 10]
    );
    let restored = VaultStorage::open(&target).unwrap();
    for note in restored.list().unwrap() {
        let prior = baseline.iter().find(|n| n.note_id == note.note_id).unwrap();
        assert_eq!(note.local_revision, prior.local_revision + 1);
        assert_eq!(note.content_hash, prior.content_hash);
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(target.join(".opennote/vault.json")).unwrap()).unwrap();
    assert_eq!(metadata["custom"], "keep");
    assert_eq!(metadata["syncEnabled"], false);
}

#[test]
fn later_staged_revision_cannot_be_silently_published_as_the_original_restore() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "test").unwrap();
    let note = vault.create_note("a.md", "old", &op()).unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let target = parent.path().join("restored");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    let operation = op();
    let cancel = AtomicBool::new(false);
    backup
        .apply_restore(&plan, &operation, &cancel, |p| {
            if p.phase == "publishing" {
                cancel.store(true, Ordering::Relaxed)
            }
        })
        .unwrap_err();
    cancel.store(false, Ordering::Relaxed);
    let staged = parent
        .path()
        .join(format!(".opennote-restore-{operation}/vault"));
    fs::remove_file(
        destination
            .path()
            .join(format!(".restore-journal-{operation}/ready.json")),
    )
    .unwrap();
    fs::remove_file(staged.join(".opennote/restore-receipt.json")).unwrap();
    let mut changed = VaultStorage::open(&staged).unwrap();
    changed.save(&note.note_id, &note.source, 1, &op()).unwrap();
    drop(changed);
    assert_eq!(
        backup
            .apply_restore(&plan, &operation, &cancel, |_| {})
            .unwrap_err()
            .code,
        ErrorCode::RestoreInterrupted
    );
    assert!(!target.exists());
    assert_eq!(
        VaultStorage::open(&staged)
            .unwrap()
            .read(&note.note_id)
            .unwrap()
            .local_revision,
        2
    );
}

#[test]
fn schema_one_snapshot_restores_read_only_with_database_state_intact() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "legacy restore").unwrap();
    let note = vault.create_note("legacy.md", "original", &op()).unwrap();
    drop(vault);
    let meta_path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    meta["custom"] = "keep".into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let database = source.path().join(".opennote/state.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE old_queue(payload TEXT);\
             INSERT INTO old_queue VALUES ('keep legacy work');",
        )
        .unwrap();
    drop(connection);

    let vault = VaultStorage::open(source.path()).unwrap();
    assert_eq!(vault.vault().schema_version, 1);
    assert!(!vault.vault().writable);
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let target = parent.path().join("legacy-restored");
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    backup
        .apply_restore(&plan, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();

    let restored = VaultStorage::open(&target).unwrap();
    assert_eq!(restored.vault().schema_version, 1);
    assert!(!restored.vault().writable);
    let document = restored.read(&note.note_id).unwrap();
    assert_eq!(document.source, note.source);
    assert_eq!(document.local_revision, note.local_revision);
    drop(restored);
    let connection = rusqlite::Connection::open(target.join(".opennote/state.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT payload FROM old_queue", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "keep legacy work"
    );
    drop(connection);
    let restored_meta: serde_json::Value =
        serde_json::from_slice(&fs::read(target.join(".opennote/vault.json")).unwrap()).unwrap();
    assert_eq!(restored_meta["schemaVersion"], 1);
    assert_eq!(restored_meta["writable"], false);
    assert_eq!(restored_meta["syncEnabled"], false);
    assert_eq!(restored_meta["custom"], "keep");
}

#[test]
fn restore_preview_reports_missing_and_unsafe_local_references_without_flagging_remote_urls() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "references").unwrap();
    vault
        .create_note(
            "notes/references.md",
            "![ok](../attachments/OK.png)\n\
             ![missing](../attachments/missing.png)\n\
             [remote](https://example.com/file.png)\n\
             `![code](../ignored.png)`\n\
             ![escape](../../outside.png)\n\
             [ref]: ../assets/ref.pdf",
            &op(),
        )
        .unwrap();
    fs::create_dir_all(source.path().join("attachments")).unwrap();
    fs::write(source.path().join("attachments/ok.png"), b"present").unwrap();
    let backup =
        BackupStore::open(source.path(), &vault.vault().vault_id, destination.path()).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let plan = backup
        .plan_restore(
            &snapshot.snapshot_id,
            &parent.path().join("restored"),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(!plan.reference_report.complete);
    assert_eq!(
        plan.reference_report
            .issues
            .iter()
            .map(|issue| (issue.reference.as_str(), issue.reason))
            .collect::<Vec<_>>(),
        vec![
            (
                "../../outside.png",
                onote_history_backup::SnapshotReferenceIssueReason::Unsafe
            ),
            (
                "../assets/ref.pdf",
                onote_history_backup::SnapshotReferenceIssueReason::Missing
            ),
            (
                "../attachments/missing.png",
                onote_history_backup::SnapshotReferenceIssueReason::Missing
            ),
        ]
    );
}
