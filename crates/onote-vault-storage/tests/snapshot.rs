use onote_domain::{CheckpointRequest, ErrorCode};
use onote_vault_storage::{Failpoint, VaultStorage};
use rusqlite::Connection;
use std::fs;

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn legacy_backup_refuses_unverified_catalog_changed_notes_and_changed_schema() {
    for scenario in ["catalog", "body", "schema", "future"] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let mut vault = VaultStorage::create(source.path(), "legacy refusal").unwrap();
        vault.create_note("a.md", "original", &op()).unwrap();
        drop(vault);
        let meta_path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
        meta["schemaVersion"] = if scenario == "future" {
            999.into()
        } else {
            1.into()
        };
        fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let readonly = VaultStorage::open(source.path()).unwrap();
        match scenario {
            "catalog" => {
                fs::remove_file(source.path().join(".opennote/state.sqlite")).unwrap();
            }
            "body" => {
                fs::write(source.path().join("a.md"), b"external change").unwrap();
            }
            "schema" => {
                meta["schemaVersion"] = 2.into();
                fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
            }
            _ => (),
        }
        let target = destination.path().join("refused.sqlite");
        assert!(readonly
            .with_snapshot_state::<()>(&target, |_, _, _| panic!("unverified snapshot callback"))
            .is_err());
        assert!(!target.exists());
    }
}

#[test]
fn legacy_protection_snapshot_exports_wal_without_migrating_the_source() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "legacy backup").unwrap();
    let note = vault.create_note("a.md", "original", &op()).unwrap();
    drop(vault);
    let database = source.path().join(".opennote/state.sqlite");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE old_queue(payload TEXT); INSERT INTO old_queue VALUES ('keep original pending work');").unwrap();
    let meta_path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let before_meta = fs::read(&meta_path).unwrap();
    let before_db = fs::read(&database).unwrap();
    let wal = source.path().join(".opennote/state.sqlite-wal");
    let before_wal = fs::read(&wal).unwrap();
    let readonly = VaultStorage::open(source.path()).unwrap();
    let target = destination.path().join("legacy.sqlite");
    readonly
        .with_snapshot_state(&target, |_, vault, notes| {
            assert_eq!(vault.schema_version, 1);
            assert!(!vault.writable);
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].note_id, note.note_id);
            assert_eq!(notes[0].local_revision, note.local_revision);
            let exported = Connection::open(&target).unwrap();
            assert_eq!(
                exported
                    .query_row("SELECT payload FROM old_queue", [], |r| r
                        .get::<_, String>(0))
                    .unwrap(),
                "keep original pending work"
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(fs::read(meta_path).unwrap(), before_meta);
    assert_eq!(fs::read(database).unwrap(), before_db);
    assert_eq!(fs::read(wal).unwrap(), before_wal);
    assert_eq!(readonly.read(&note.note_id).unwrap().source, note.source);
}

#[test]
fn snapshot_exports_committed_sqlite_without_copying_live_wal_or_stale_instance_cache() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let first = VaultStorage::create(source.path(), "冻结").unwrap();
    let mut other = VaultStorage::open(source.path()).unwrap();
    let note = other.create_note("中文.md", "原字节", &op()).unwrap();
    let draft = other
        .write_checkpoint(CheckpointRequest {
            note_id: note.note_id.clone(),
            relative_path: note.relative_path.clone(),
            source: "尚未提交的中文草稿".into(),
            base_revision: note.local_revision,
            base_content_hash: note.content_hash.clone(),
            operation_id: op(),
        })
        .unwrap();
    let target = destination.path().join("state.sqlite");
    first
        .with_snapshot_state(&target, |root, vault, notes| {
            assert_eq!(fs::canonicalize(source.path()).unwrap(), root);
            assert_eq!(vault.vault_id, note.vault_id);
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].content_hash, note.content_hash);
            let competing_lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.join(".opennote/writer.lock"))
                .unwrap();
            assert!(matches!(
                competing_lock.try_lock(),
                Err(fs::TryLockError::WouldBlock)
            ));
            let db = Connection::open(&target).unwrap();
            assert_eq!(
                db.query_row("pragma integrity_check", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
            assert_eq!(
                db.query_row("select note_id from notes", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                note.note_id
            );
            assert_eq!(
                db.query_row(
                    "select source from draft_checkpoints where draft_id=?1",
                    [&draft.draft_id],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "尚未提交的中文草稿"
            );
            Ok(())
        })
        .unwrap();
    assert!(!destination.path().join("state.sqlite-wal").exists());
    assert_eq!(
        first
            .with_snapshot_state(&target, |_, _, _| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::PathCollision
    );
}

#[test]
fn incomplete_or_changed_vault_refuses_before_exporting_a_successful_baseline() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(source.path(), "test").unwrap();
    let note = vault.create_note("note.md", "old", &op()).unwrap();
    let target = destination.path().join("state.sqlite");
    fs::write(source.path().join("note.md"), "external").unwrap();
    assert_eq!(
        vault
            .with_snapshot_state(&target, |_, _, _| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    assert!(!target.exists());
    fs::write(source.path().join("note.md"), note.source.as_bytes()).unwrap();
    vault.push_failpoint(Failpoint::AfterPrepared);
    vault.save(&note.note_id, "pending", 0, &op()).unwrap_err();
    assert_eq!(
        vault
            .with_snapshot_state(&target, |_, _, _| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::RescanRequired
    );
    assert!(!target.exists());
}

#[test]
fn snapshot_refuses_recursive_destinations_and_keeps_preexisting_targets() {
    let source = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(source.path(), "test").unwrap();
    let recursive = source.path().join("export.sqlite");
    assert_eq!(
        vault
            .with_snapshot_state(&recursive, |_, _, _| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert!(!recursive.exists());
    let destination = tempfile::tempdir().unwrap();
    let existing = destination.path().join("state.sqlite");
    fs::write(&existing, b"sentinel").unwrap();
    assert_eq!(
        vault
            .with_snapshot_state(&existing, |_, _, _| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::PathCollision
    );
    assert_eq!(fs::read(existing).unwrap(), b"sentinel");
}

#[test]
fn restoring_snapshot_baseline_advances_revisions_once_and_keeps_drafts_and_raw_bytes() {
    let source = tempfile::tempdir().unwrap();
    let mut original = VaultStorage::create(source.path(), "restore").unwrap();
    let note = original.create_note("note.md", "body", &op()).unwrap();
    let draft = original
        .write_checkpoint(CheckpointRequest {
            note_id: note.note_id.clone(),
            relative_path: note.relative_path.clone(),
            source: "未提交草稿".into(),
            base_revision: 0,
            base_content_hash: note.content_hash.clone(),
            operation_id: op(),
        })
        .unwrap();
    let notes = original.list().unwrap();
    drop(original);
    let meta = source.path().join(".opennote/vault.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
    value["writable"] = false.into();
    value["syncEnabled"] = false.into();
    value["unknown"] = "preserve".into();
    fs::write(&meta, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut restored = VaultStorage::open(source.path()).unwrap();
    let operation = op();
    let hash = "a".repeat(64);
    assert!(!restored.vault().writable);
    assert!(restored.read(&note.note_id).unwrap().read_only);
    restored.push_failpoint(Failpoint::BeforeDbCommit);
    assert!(restored
        .finalize_snapshot_restore(&notes, &hash, &operation)
        .is_err());
    assert_eq!(restored.read(&note.note_id).unwrap().local_revision, 0);
    let result = restored
        .finalize_snapshot_restore(&notes, &hash, &operation)
        .unwrap();
    assert_eq!(result[0].local_revision, 1);
    assert_eq!(
        restored.read_draft(&draft.draft_id).unwrap().source,
        "未提交草稿"
    );
    assert_eq!(
        fs::read(source.path().join("note.md")).unwrap(),
        note.source.as_bytes()
    );
    assert_eq!(
        restored
            .finalize_snapshot_restore(&notes, &hash, &operation)
            .unwrap(),
        result
    );
    assert_eq!(
        restored
            .finalize_snapshot_restore(&notes, &"b".repeat(64), &operation)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        restored
            .create_note("reused.md", "bad", &operation)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    let metadata: serde_json::Value = serde_json::from_slice(&fs::read(meta).unwrap()).unwrap();
    assert_eq!(metadata["writable"], true);
    assert_eq!(metadata["syncEnabled"], false);
    assert_eq!(metadata["unknown"], "preserve");
}
