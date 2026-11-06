use onote_domain::ErrorCode;
use onote_history_backup::HistoryStore;
use onote_vault_storage::{Failpoint, TrashArchive, VaultStorage};
use std::{fs, sync::Arc};
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn setup() -> (tempfile::TempDir, VaultStorage, Arc<HistoryStore>) {
    let dir = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(dir.path(), "trash").unwrap();
    let history = Arc::new(HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap());
    let mut vault = vault;
    vault.set_revision_archive(history.clone());
    vault.set_trash_archive(history.clone());
    (dir, vault, history)
}
fn reopen(dir: &tempfile::TempDir, history: &Arc<HistoryStore>) -> VaultStorage {
    let mut vault = VaultStorage::open(dir.path()).unwrap();
    vault.set_revision_archive(history.clone());
    vault.set_trash_archive(history.clone());
    vault
}
#[test]
fn delete_retains_exact_bytes_and_shared_assets_restore_keeps_identity_and_advances_revision() {
    let (dir, mut vault, history) = setup();
    let note = vault
        .create_note("中文.md", "![PDF](shared.pdf)\n原始正文", &op())
        .unwrap();
    let other = vault
        .create_note("other.md", "[shared](shared.pdf)", &op())
        .unwrap();
    fs::write(dir.path().join("shared.pdf"), b"synthetic shared PDF").unwrap();
    let deletion = op();
    let entry = vault
        .delete_note(&note.note_id, 0, &note.content_hash, &deletion)
        .unwrap();
    assert_eq!(entry.deleted_revision, 1);
    assert!(!dir.path().join("中文.md").exists());
    assert_eq!(vault.list().unwrap().len(), 1);
    assert_eq!(vault.read(&other.note_id).unwrap(), other);
    assert_eq!(
        history.read_deleted(&entry.object_id).unwrap().document,
        note
    );
    assert_eq!(
        vault
            .delete_note(&note.note_id, 0, &note.content_hash, &deletion)
            .unwrap(),
        entry
    );
    drop(vault);
    let mut vault = reopen(&dir, &history);
    assert_eq!(vault.list_trash().unwrap(), vec![entry.clone()]);
    assert_eq!(vault.list().unwrap().len(), 1);
    let restore_op = op();
    let restored = vault
        .restore_trash(&entry.object_id, "恢复/中文.md", 1, &restore_op)
        .unwrap();
    assert_eq!(restored.note_id, note.note_id);
    assert_eq!(restored.local_revision, 2);
    assert_eq!(vault.read(&note.note_id).unwrap().source, note.source);
    assert!(vault.list_trash().unwrap().is_empty());
    assert_eq!(
        vault
            .restore_trash(&entry.object_id, "恢复/中文.md", 1, &restore_op)
            .unwrap(),
        restored
    );
    assert_eq!(
        fs::read(dir.path().join("shared.pdf")).unwrap(),
        b"synthetic shared PDF"
    );
}
#[test]
fn restore_collision_and_changed_requests_preserve_both_versions() {
    let (dir, mut vault, _) = setup();
    let note = vault.create_note("note.md", "old", &op()).unwrap();
    let delete = op();
    let entry = vault
        .delete_note(&note.note_id, 0, &note.content_hash, &delete)
        .unwrap();
    fs::write(dir.path().join("note.md"), b"new unrelated note").unwrap();
    assert_eq!(
        vault
            .restore_trash(&entry.object_id, "note.md", 1, &op())
            .unwrap_err()
            .code,
        ErrorCode::PathCollision
    );
    assert_eq!(
        fs::read(dir.path().join("note.md")).unwrap(),
        b"new unrelated note"
    );
    assert_eq!(
        vault
            .delete_note(&note.note_id, 1, &note.content_hash, &delete)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        vault
            .create_note("unrelated.md", "text", &delete)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        vault
            .save(&note.note_id, "text", 0, &delete)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        vault
            .restore_trash(&entry.object_id, "alternate.md", 0, &op())
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    vault
        .restore_trash(&entry.object_id, "alternate.md", 1, &op())
        .unwrap();
}
#[test]
fn external_edits_and_stale_instances_cannot_delete_a_newer_version() {
    let (dir, mut vault, history) = setup();
    let note = vault.create_note("note.md", "old", &op()).unwrap();
    let mut stale = reopen(&dir, &history);
    vault.save(&note.note_id, "new", 0, &op()).unwrap();
    assert_eq!(
        stale
            .delete_note(&note.note_id, 0, &note.content_hash, &op())
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    assert!(stale.list_trash().unwrap().is_empty());
    let current = vault.read(&note.note_id).unwrap();
    fs::write(
        dir.path().join("note.md"),
        current.source.replace("new", "external"),
    )
    .unwrap();
    assert_eq!(
        vault
            .delete_note(&note.note_id, 1, &current.content_hash, &op())
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    assert!(fs::read_to_string(dir.path().join("note.md"))
        .unwrap()
        .ends_with("external"));
}
#[test]
fn delete_crash_boundaries_preserve_source_and_resume_without_duplicate_trash() {
    for point in [
        Failpoint::AfterPrepared,
        Failpoint::AfterReplace,
        Failpoint::BeforeDbCommit,
    ] {
        let (dir, mut vault, history) = setup();
        let note = vault.create_note("note.md", "original", &op()).unwrap();
        let operation = op();
        vault.push_failpoint(point);
        vault
            .delete_note(&note.note_id, 0, &note.content_hash, &operation)
            .unwrap_err();
        assert_eq!(history.read_deleted(&operation).unwrap().document, note);
        drop(vault);
        let mut vault = reopen(&dir, &history);
        let report = vault.recover().unwrap();
        if point == Failpoint::AfterPrepared {
            assert_eq!(report.pending, vec![note.note_id.clone()]);
            assert_eq!(vault.read(&note.note_id).unwrap().source, note.source);
            assert!(vault.read(&note.note_id).unwrap().read_only);
        } else {
            assert_eq!(report.applied, vec![note.note_id.clone()]);
        }
        vault
            .delete_note(&note.note_id, 0, &note.content_hash, &operation)
            .unwrap();
        assert_eq!(vault.list_trash().unwrap().len(), 1);
        assert!(!dir.path().join("note.md").exists());
    }
}
#[test]
fn restore_crash_boundaries_preserve_tombstone_and_replay_original_result() {
    for point in [
        Failpoint::AfterPrepared,
        Failpoint::AfterReplace,
        Failpoint::BeforeDbCommit,
    ] {
        let (dir, mut vault, history) = setup();
        let note = vault.create_note("note.md", "original", &op()).unwrap();
        let entry = vault
            .delete_note(&note.note_id, 0, &note.content_hash, &op())
            .unwrap();
        let operation = op();
        vault.push_failpoint(point);
        vault
            .restore_trash(&entry.object_id, "restore.md", 1, &operation)
            .unwrap_err();
        drop(vault);
        let mut vault = reopen(&dir, &history);
        let report = vault.recover().unwrap();
        if point == Failpoint::AfterPrepared {
            assert_eq!(report.pending, vec![note.note_id.clone()]);
        } else {
            assert_eq!(report.applied, vec![note.note_id.clone()]);
        }
        let restored = vault
            .restore_trash(&entry.object_id, "restore.md", 1, &operation)
            .unwrap();
        assert_eq!(restored.local_revision, 2);
        assert_eq!(vault.read(&note.note_id).unwrap().source, note.source);
        assert!(vault.list_trash().unwrap().is_empty());
    }
}
#[test]
fn corruption_and_reparse_points_never_turn_into_overwrite_or_outside_writes() {
    let (dir, mut vault, history) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let entry = vault
        .delete_note(&note.note_id, 0, &note.content_hash, &op())
        .unwrap();
    let path = dir
        .path()
        .join(".opennote/trash")
        .join(format!("{}.json", entry.object_id));
    fs::write(&path, "{}").unwrap();
    assert!(vault
        .restore_trash(&entry.object_id, "restore.md", 1, &op())
        .is_err());
    assert!(!dir.path().join("restore.md").exists());
    assert!(vault.purge_trash(&entry.object_id, 1, &op()).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "{}");
    assert!(history.read_deleted("../note").is_err());
    let outside = tempfile::tempdir().unwrap();
    let link = dir.path().join("linked");
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .unwrap()
        .status
        .success());
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    assert!(vault
        .restore_trash(&entry.object_id, "linked/out.md", 1, &op())
        .is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
#[test]
fn purge_does_not_erase_history_or_assets_and_restarts_idempotently() {
    let (dir, mut vault, history) = setup();
    let note = vault
        .create_note("note.md", "[asset](shared.pdf)", &op())
        .unwrap();
    fs::write(dir.path().join("shared.pdf"), b"shared").unwrap();
    let entry = vault
        .delete_note(&note.note_id, 0, &note.content_hash, &op())
        .unwrap();
    let operation = op();
    vault.push_failpoint(Failpoint::AfterReplace);
    vault
        .purge_trash(&entry.object_id, 1, &operation)
        .unwrap_err();
    drop(vault);
    let mut vault = reopen(&dir, &history);
    assert_eq!(vault.recover().unwrap().applied, vec![note.note_id.clone()]);
    vault.purge_trash(&entry.object_id, 1, &operation).unwrap();
    assert!(vault.list_trash().unwrap().is_empty());
    assert!(!history.list(&note.note_id).unwrap().is_empty());
    assert_eq!(fs::read(dir.path().join("shared.pdf")).unwrap(), b"shared");
    assert!(vault
        .restore_trash(&entry.object_id, "restore.md", 1, &op())
        .is_err());
}
#[test]
fn externally_reappeared_identity_is_not_silently_resurrected_or_duplicated() {
    let (dir, mut vault, history) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let entry = vault
        .delete_note(&note.note_id, 0, &note.content_hash, &op())
        .unwrap();
    fs::write(dir.path().join("reappeared.md"), &note.source).unwrap();
    assert_eq!(
        vault
            .restore_trash(&entry.object_id, "restored.md", 1, &op())
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    drop(vault);
    let vault = reopen(&dir, &history);
    assert!(!vault.vault().writable);
    assert!(vault
        .vault()
        .findings
        .iter()
        .any(|f| f.code == "DeletedNoteReappeared"));
    assert!(vault.list().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("reappeared.md")).unwrap(),
        note.source
    );
}

#[test]
fn archive_first_retry_uses_the_existing_timestamp_after_a_missing_journal_write() {
    let (_, mut vault, history) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let operation = op();
    let entry = onote_domain::TrashEntry {
        object_id: operation.clone(),
        note_id: note.note_id.clone(),
        original_path: note.relative_path.clone(),
        content_hash: note.content_hash.clone(),
        deleted_revision: 1,
        deleted_at: "2026-10-17T00:00:00Z".into(),
        retention_days: 30,
    };
    history
        .capture_deleted(&onote_domain::TrashRecord {
            entry: entry.clone(),
            document: note.clone(),
        })
        .unwrap();
    assert_eq!(
        vault
            .delete_note(&note.note_id, 0, &note.content_hash, &operation)
            .unwrap(),
        entry
    );
}

#[test]
fn unavailable_archive_and_foreign_auxiliary_file_cannot_remove_the_original() {
    let (dir, mut vault, history) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let operation = op();
    let sentinel = dir.path().join(format!("note.md.onote-trash-{operation}"));
    fs::write(&sentinel, b"unrelated sentinel").unwrap();
    assert!(vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"unrelated sentinel");
    assert_eq!(
        fs::read_to_string(dir.path().join("note.md")).unwrap(),
        note.source
    );
    let archive = dir
        .path()
        .join(".opennote/trash")
        .join(format!("{operation}.json"));
    let original_record = fs::read(&archive).unwrap();
    fs::write(&archive, "{}").unwrap();
    assert_eq!(vault.recover().unwrap().pending, vec![note.note_id.clone()]);
    assert_eq!(fs::read(&sentinel).unwrap(), b"unrelated sentinel");
    assert!(history.read_deleted(&operation).is_err());
    assert!(!original_record.is_empty());
}

#[test]
fn repeated_delete_restore_cycles_never_replay_an_old_delete_against_a_new_body() {
    let (_, mut vault, _) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let saved_op = op();
    let saved = vault.save(&note.note_id, "second", 0, &saved_op).unwrap();
    let current = vault.read(&note.note_id).unwrap();
    let delete_op = op();
    let first = vault
        .delete_note(&note.note_id, 1, &current.content_hash, &delete_op)
        .unwrap();
    assert_eq!(
        vault.save(&note.note_id, "second", 0, &saved_op).unwrap(),
        saved
    );
    let restore = vault
        .restore_trash(&first.object_id, "note.md", 2, &op())
        .unwrap();
    assert_eq!(restore.local_revision, 3);
    assert_eq!(
        vault
            .delete_note(&note.note_id, 1, &current.content_hash, &delete_op)
            .unwrap(),
        first
    );
    assert_eq!(vault.read(&note.note_id).unwrap().source, current.source);
    let second = vault
        .delete_note(&note.note_id, 3, &restore.content_hash, &op())
        .unwrap();
    assert_eq!(second.deleted_revision, 4);
    assert_eq!(
        vault
            .restore_trash(&second.object_id, "note.md", 4, &op())
            .unwrap()
            .local_revision,
        5
    );
}

#[test]
fn cancellation_rolls_back_delete_before_or_after_move_and_survives_its_own_crash() {
    for point in [Failpoint::AfterPrepared, Failpoint::AfterReplace] {
        let (dir, mut vault, history) = setup();
        let note = vault.create_note("note.md", "original", &op()).unwrap();
        let operation = op();
        vault.push_failpoint(point);
        vault
            .delete_note(&note.note_id, 0, &note.content_hash, &operation)
            .unwrap_err();
        vault.push_failpoint(Failpoint::BeforeDbCommit);
        vault.cancel_trash(&operation).unwrap_err();
        assert_eq!(
            fs::read_to_string(dir.path().join("note.md")).unwrap(),
            note.source
        );
        drop(vault);
        let mut vault = reopen(&dir, &history);
        let report = vault.recover().unwrap();
        assert!(report.pending.is_empty());
        assert!(report.applied.is_empty());
        assert_eq!(
            vault.cancel_trash(&operation).unwrap(),
            onote_domain::TrashCancellation::Cancelled
        );
        assert_eq!(
            vault
                .delete_note(&note.note_id, 0, &note.content_hash, &operation)
                .unwrap_err()
                .code,
            ErrorCode::VersionConflict
        );
        assert_eq!(vault.read(&note.note_id).unwrap(), note);
        assert!(vault.list_trash().unwrap().is_empty());
        vault
            .save(&note.note_id, "editing can continue", 0, &op())
            .unwrap();
    }
}

#[test]
fn cancellation_reports_a_published_restore_as_applied_instead_of_claiming_it_was_undone() {
    for point in [Failpoint::AfterPrepared, Failpoint::AfterReplace] {
        let (_, mut vault, _) = setup();
        let note = vault.create_note("note.md", "original", &op()).unwrap();
        let entry = vault
            .delete_note(&note.note_id, 0, &note.content_hash, &op())
            .unwrap();
        let operation = op();
        vault.push_failpoint(point);
        vault
            .restore_trash(&entry.object_id, "restore.md", 1, &operation)
            .unwrap_err();
        let outcome = vault.cancel_trash(&operation).unwrap();
        if point == Failpoint::AfterPrepared {
            assert_eq!(outcome, onote_domain::TrashCancellation::Cancelled);
            assert_eq!(vault.list_trash().unwrap(), vec![entry]);
        } else {
            assert_eq!(outcome, onote_domain::TrashCancellation::AlreadyApplied);
            assert_eq!(vault.read(&note.note_id).unwrap().source, note.source);
            assert!(vault.list_trash().unwrap().is_empty());
        }
    }
}

#[test]
fn cancelling_with_an_unrelated_auxiliary_file_keeps_it_and_unblocks_the_original() {
    let (dir, mut vault, _) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let operation = op();
    let path = dir.path().join(format!("note.md.onote-trash-{operation}"));
    fs::write(&path, b"unrelated").unwrap();
    vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .unwrap_err();
    assert_eq!(
        vault.cancel_trash(&operation).unwrap(),
        onote_domain::TrashCancellation::Cancelled
    );
    assert_eq!(fs::read(path).unwrap(), b"unrelated");
    assert_eq!(vault.read(&note.note_id).unwrap(), note);
}

#[test]
fn deletion_forces_the_latest_history_even_inside_the_edit_coalescing_window() {
    let (_, mut vault, history) = setup();
    let note = vault.create_note("note.md", "zero", &op()).unwrap();
    vault.save(&note.note_id, "one", 0, &op()).unwrap();
    let save = vault.save(&note.note_id, "two", 1, &op()).unwrap();
    assert_eq!(history.list(&note.note_id).unwrap().len(), 1);
    vault
        .delete_note(&note.note_id, 2, &save.content_hash, &op())
        .unwrap();
    assert!(history
        .list(&note.note_id)
        .unwrap()
        .iter()
        .any(|revision| revision.local_revision == 2));
}

#[test]
fn purge_cancellation_distinguishes_retained_content_from_already_removed_trash() {
    for point in [Failpoint::AfterPrepared, Failpoint::AfterReplace] {
        let (_, mut vault, history) = setup();
        let note = vault.create_note("note.md", "original", &op()).unwrap();
        let entry = vault
            .delete_note(&note.note_id, 0, &note.content_hash, &op())
            .unwrap();
        let operation = op();
        vault.push_failpoint(point);
        vault
            .purge_trash(&entry.object_id, 1, &operation)
            .unwrap_err();
        let result = vault.cancel_trash(&operation).unwrap();
        if point == Failpoint::AfterPrepared {
            assert_eq!(result, onote_domain::TrashCancellation::Cancelled);
            assert_eq!(
                history.read_deleted(&entry.object_id).unwrap().document,
                note
            );
            assert_eq!(vault.list_trash().unwrap(), vec![entry]);
        } else {
            assert_eq!(result, onote_domain::TrashCancellation::AlreadyApplied);
            assert!(history.read_deleted(&entry.object_id).is_err());
            assert!(vault.list_trash().unwrap().is_empty());
        }
    }
}

#[test]
fn pending_operations_expose_the_original_request_for_explicit_restart_resume() {
    let (dir, mut vault, history) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let operation = op();
    vault.push_failpoint(Failpoint::AfterPrepared);
    vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .unwrap_err();
    drop(vault);
    let mut vault = reopen(&dir, &history);
    let pending = vault.list_trash_operations().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, operation);
    assert_eq!(pending[0].kind, "delete");
    let result = vault.resume_trash(&operation).unwrap();
    assert_eq!(result.kind, "delete");
    assert_eq!(vault.trash_entry(&operation).unwrap(), result.entry);
    assert!(vault.list_trash_operations().unwrap().is_empty());
    assert_eq!(vault.resume_trash(&operation).unwrap(), result);
}

#[test]
fn disk_full_refusal_keeps_file_and_tombstone_state_unchanged() {
    let (_, mut vault, history) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    vault.push_failpoint(Failpoint::WriteFailure);
    assert_eq!(
        vault
            .delete_note(&note.note_id, 0, &note.content_hash, &op())
            .unwrap_err()
            .code,
        ErrorCode::DiskFull
    );
    assert_eq!(vault.read(&note.note_id).unwrap(), note);
    assert!(vault.list_trash().unwrap().is_empty());
    let entry = vault
        .delete_note(&note.note_id, 0, &note.content_hash, &op())
        .unwrap();
    vault.push_failpoint(Failpoint::WriteFailure);
    assert_eq!(
        vault
            .restore_trash(&entry.object_id, "note.md", 1, &op())
            .unwrap_err()
            .code,
        ErrorCode::DiskFull
    );
    assert_eq!(vault.list_trash().unwrap(), vec![entry.clone()]);
    assert_eq!(
        history.read_deleted(&entry.object_id).unwrap().document,
        note
    );
    vault.push_failpoint(Failpoint::WriteFailure);
    assert_eq!(
        vault
            .purge_trash(&entry.object_id, 1, &op())
            .unwrap_err()
            .code,
        ErrorCode::DiskFull
    );
    assert_eq!(
        history.read_deleted(&entry.object_id).unwrap().document,
        note
    );
}

#[test]
fn forged_commit_status_cannot_report_a_delete_that_never_reached_the_catalog() {
    use sha2::{Digest, Sha256};
    let (dir, mut vault, _) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let operation = op();
    vault.push_failpoint(Failpoint::AfterPrepared);
    vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .unwrap_err();
    let conn = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
    let payload: String = conn
        .query_row(
            "SELECT payload FROM trash_actions WHERE operation_id=?1",
            [&operation],
            |r| r.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    value["status"] = "committed".into();
    let payload = serde_json::to_string(&value).unwrap();
    conn.execute(
        "UPDATE trash_actions SET status='committed',payload=?2,checksum=?3 WHERE operation_id=?1",
        rusqlite::params![
            operation,
            payload,
            hex::encode(Sha256::digest(payload.as_bytes()))
        ],
    )
    .unwrap();
    assert!(vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("note.md")).unwrap(),
        note.source
    );
}

#[test]
fn untrusted_journal_paths_are_refused_before_any_outside_file_is_touched() {
    use sha2::{Digest, Sha256};
    let (dir, mut vault, _) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let operation = op();
    vault.push_failpoint(Failpoint::AfterPrepared);
    vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .unwrap_err();
    let outside = tempfile::tempdir().unwrap();
    let outside_path = outside.path().join("keep.md");
    fs::write(&outside_path, b"outside sentinel").unwrap();
    let conn = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
    let payload: String = conn
        .query_row(
            "SELECT payload FROM trash_actions WHERE operation_id=?1",
            [&operation],
            |r| r.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    value["entry"]["originalPath"] = outside_path.to_string_lossy().to_string().into();
    let payload = serde_json::to_string(&value).unwrap();
    conn.execute(
        "UPDATE trash_actions SET payload=?2,checksum=?3 WHERE operation_id=?1",
        rusqlite::params![
            operation,
            payload,
            hex::encode(Sha256::digest(payload.as_bytes()))
        ],
    )
    .unwrap();
    assert!(vault.resume_trash(&operation).is_err());
    assert_eq!(fs::read(outside_path).unwrap(), b"outside sentinel");
    assert_eq!(
        fs::read_to_string(dir.path().join("note.md")).unwrap(),
        note.source
    );
}

#[test]
fn cancelling_never_deletes_a_preexisting_auxiliary_even_if_its_bytes_match() {
    let (dir, mut vault, _) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let operation = op();
    let path = dir.path().join(format!("note.md.onote-trash-{operation}"));
    fs::write(&path, &note.source).unwrap();
    vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .unwrap_err();
    assert_eq!(
        vault.cancel_trash(&operation).unwrap(),
        onote_domain::TrashCancellation::Cancelled
    );
    assert_eq!(fs::read_to_string(path).unwrap(), note.source);
    assert_eq!(vault.read(&note.note_id).unwrap(), note);
}

#[test]
fn pending_deletion_reserves_unicode_case_variants_before_new_files_can_be_created() {
    let (dir, mut vault, _) = setup();
    let note = vault.create_note("Σ.md", "original", &op()).unwrap();
    let operation = op();
    vault.push_failpoint(Failpoint::AfterReplace);
    vault
        .delete_note(&note.note_id, 0, &note.content_hash, &operation)
        .unwrap_err();
    assert_eq!(
        vault
            .create_note("σ.md", "unrelated", &op())
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    vault.cancel_trash(&operation).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("Σ.md")).unwrap(),
        note.source
    );
}

#[test]
fn duplicate_authoritative_identity_refuses_before_any_original_file_is_moved() {
    let (dir, mut vault, history) = setup();
    let note = vault.create_note("note.md", "original", &op()).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
    conn.execute("INSERT INTO notes(relative_path,note_id,content_hash,local_revision,read_only,id_embedded) VALUES ('alias.md',?1,?2,0,0,1)",rusqlite::params![note.note_id,note.content_hash]).unwrap();
    let operation = op();
    assert_eq!(
        vault
            .delete_note(&note.note_id, 0, &note.content_hash, &operation)
            .unwrap_err()
            .code,
        ErrorCode::RescanRequired
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("note.md")).unwrap(),
        note.source
    );
    assert!(history.read_deleted(&operation).is_err());
}

#[test]
fn catalog_discovery_does_not_silently_resurrect_a_deleted_identity() {
    let (dir, mut vault, history) = setup();
    let note = vault.create_note("Deleted.md", "body", &op()).unwrap();
    let entry = vault
        .delete_note(
            &note.note_id,
            note.local_revision,
            &note.content_hash,
            &op(),
        )
        .unwrap();
    fs::write(dir.path().join("Unexpected.md"), &note.source).unwrap();
    let observed = vault
        .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
        .unwrap();
    assert!(observed.notes.iter().all(|n| n.note_id != note.note_id));
    assert!(observed
        .findings
        .iter()
        .any(|finding| finding.code == "DeletedNoteReappeared"));
    assert_eq!(
        history.read_deleted(&entry.object_id).unwrap().document,
        note
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("Unexpected.md")).unwrap(),
        note.source
    );
}
