use onote_domain::{TagWriteItem, TagWritePlan};
use onote_vault_storage::{Failpoint, VaultStorage};
use std::fs;

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn fixture() -> (tempfile::TempDir, VaultStorage, TagWritePlan) {
    let dir = tempfile::tempdir().unwrap();
    for path in ["A.md", "B.md"] {
        fs::write(dir.path().join(path),format!("\u{feff}---\r\nopennote-id: {}\r\ncustom: unchanged\r\ntags: [old]\r\n---\r\n#old `#old`\r\n",op())).unwrap();
    }
    let storage = VaultStorage::create(dir.path(), "tags").unwrap();
    let mut items = Vec::new();
    for summary in storage.list().unwrap() {
        let note = storage.read(&summary.note_id).unwrap();
        items.push(TagWriteItem {
            note_id: note.note_id,
            relative_path: note.relative_path,
            expected_revision: note.local_revision,
            content_hash: note.content_hash,
            source: note
                .source
                .replace("tags: [old]", "tags: [new]")
                .replace("\r\n#old ", "\r\n#new "),
        });
    }
    items.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let plan = TagWritePlan {
        plan_id: op(),
        vault_id: storage.vault().vault_id,
        old_tag: "old".into(),
        new_tag: Some("new".into()),
        is_merge: false,
        items,
    };
    (dir, storage, plan)
}

#[test]
fn batch_replays_across_reopen_without_incrementing_completed_revisions() {
    let (dir, mut storage, plan) = fixture();
    let id = op();
    let result = storage.apply_tag_update(&plan, &id).unwrap();
    assert!(result.completed);
    assert_eq!(result.items.len(), 2);
    assert!(result
        .items
        .iter()
        .all(|i| i.result.as_ref().unwrap().local_revision == 1));
    let source = fs::read(dir.path().join("A.md")).unwrap();
    assert!(String::from_utf8(source)
        .unwrap()
        .contains("#new `#old`\r\n"));
    drop(storage);
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(storage.resume_tag_update(&id).unwrap(), result);
    let mut changed = plan.clone();
    changed.new_tag = Some("different".into());
    assert!(storage.apply_tag_update(&changed, &id).is_err());
}

#[test]
fn an_external_change_pauses_only_that_note_and_reports_partial_success() {
    let (dir, mut storage, plan) = fixture();
    fs::write(dir.path().join("A.md"), "external text").unwrap();
    let result = storage.apply_tag_update(&plan, &op()).unwrap();
    assert!(!result.completed);
    assert_eq!(
        result.items[0].error.as_ref().unwrap().code,
        onote_domain::ErrorCode::VersionConflict
    );
    assert!(result.items[1].result.is_some());
    assert_eq!(
        fs::read_to_string(dir.path().join("A.md")).unwrap(),
        "external text"
    );
}

#[test]
fn interrupted_batch_keeps_child_ids_and_cancellation_keeps_completed_work() {
    for cancel in [false, true] {
        let (dir, mut storage, plan) = fixture();
        let id = op();
        storage.push_failpoint(Failpoint::TagBatchItemCommitted);
        let partial = storage.apply_tag_update(&plan, &id).unwrap();
        assert!(!partial.completed);
        assert!(partial.items[0].result.is_some());
        assert!(partial.items[1].result.is_none());
        drop(storage);
        let mut storage = VaultStorage::open(dir.path()).unwrap();
        assert!(storage
            .list_tag_updates()
            .unwrap()
            .iter()
            .any(|b| b.operation_id == id));
        if cancel {
            storage.cancel_tag_update(&id).unwrap();
        }
        let result = storage.resume_tag_update(&id).unwrap();
        assert_eq!(result.cancelled, cancel);
        assert_eq!(result.completed, !cancel);
        assert_eq!(
            storage.read(&plan.items[0].note_id).unwrap().local_revision,
            1
        );
        assert_eq!(
            storage.read(&plan.items[1].note_id).unwrap().local_revision,
            if cancel { 0 } else { 1 }
        );
    }
}

#[test]
fn cooperative_cancellation_stops_before_the_next_note() {
    let (_, mut storage, plan) = fixture();
    let mut checks = 0;
    let result = storage
        .apply_tag_update_with_cancel(&plan, &op(), || {
            checks += 1;
            checks > 1
        })
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(
        result.items.iter().filter(|i| i.result.is_some()).count(),
        1
    );
    assert_eq!(
        storage.read(&plan.items[1].note_id).unwrap().local_revision,
        0
    );
}

#[test]
fn child_save_failpoints_recover_with_the_original_child_operations() {
    for stop in [
        Failpoint::AfterPrepared,
        Failpoint::BeforeReplace,
        Failpoint::AfterReplace,
        Failpoint::BeforeDbCommit,
    ] {
        let (dir, mut storage, plan) = fixture();
        let id = op();
        storage.push_failpoint(stop);
        let result = storage.apply_tag_update(&plan, &id).unwrap();
        assert!(!result.completed);
        assert!(result.items[0].error.is_some());
        assert!(result.items[1].result.is_some());
        drop(storage);
        let mut storage = VaultStorage::open(dir.path()).unwrap();
        storage.recover().unwrap();
        let result = storage.resume_tag_update(&id).unwrap();
        assert!(result.completed, "{stop:?}: {result:?}");
        assert!(result
            .items
            .iter()
            .all(|i| i.result.as_ref().unwrap().local_revision == 1));
        let db = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM operations WHERE kind='save'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
    }
}

#[test]
fn exact_hash_and_parent_operation_identity_are_enforced() {
    let (dir, mut storage, mut plan) = fixture();
    let original = fs::read(dir.path().join("A.md")).unwrap();
    plan.items[0].content_hash = "a".repeat(64);
    let id = op();
    let result = storage.apply_tag_update(&plan, &id).unwrap();
    assert_eq!(
        result.items[0].error.as_ref().unwrap().code,
        onote_domain::ErrorCode::VersionConflict
    );
    assert_eq!(fs::read(dir.path().join("A.md")).unwrap(), original);
    assert_eq!(
        storage
            .create_note("No.md", "not created", &id)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::IdempotencyMismatch
    );
    assert!(!dir.path().join("No.md").exists());
}

#[test]
fn a_readonly_disk_session_or_damaged_parent_cannot_resume_writes() {
    for corrupt in [false, true] {
        let (dir, mut storage, plan) = fixture();
        let id = op();
        storage.push_failpoint(Failpoint::TagBatchItemCommitted);
        storage.apply_tag_update(&plan, &id).unwrap();
        let original = fs::read(dir.path().join("B.md")).unwrap();
        if corrupt {
            let db = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
            db.execute(
                "UPDATE tag_batches SET cancellation_hash='damaged' WHERE operation_id=?1",
                [&id],
            )
            .unwrap();
            assert!(storage.resume_tag_update(&id).is_err());
        } else {
            let path = dir.path().join(".opennote/vault.json");
            let mut metadata: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            metadata["writable"] = false.into();
            fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
            let result = storage.resume_tag_update(&id).unwrap();
            assert!(result.items[1].error.is_some());
        }
        assert_eq!(fs::read(dir.path().join("B.md")).unwrap(), original);
    }
}

#[test]
fn unsafe_plan_shapes_are_rejected_before_any_note_write() {
    let (dir, mut storage, plan) = fixture();
    let original = fs::read(dir.path().join("A.md")).unwrap();
    for variant in 0..5 {
        let mut changed = plan.clone();
        match variant {
            0 => changed.items[0].relative_path = "../outside.md".into(),
            1 => changed.items.push(changed.items[0].clone()),
            2 => changed.vault_id = op(),
            3 => changed.items[0].expected_revision = u64::MAX,
            _ => changed.plan_id = "invalid".into(),
        }
        assert!(storage.apply_tag_update(&changed, &op()).is_err());
        assert_eq!(fs::read(dir.path().join("A.md")).unwrap(), original);
    }
    assert!(storage.list_tag_updates().unwrap().is_empty());
}

#[test]
fn copied_parent_receipts_are_historical_and_cannot_resume_in_a_different_root() {
    let (dir, mut storage, plan) = fixture();
    let id = op();
    storage.push_failpoint(Failpoint::TagBatchItemCommitted);
    storage.apply_tag_update(&plan, &id).unwrap();
    drop(storage);
    let copy = tempfile::tempdir().unwrap();
    fs::create_dir(copy.path().join(".opennote")).unwrap();
    for path in [
        "A.md",
        "B.md",
        ".opennote/vault.json",
        ".opennote/state.sqlite",
    ] {
        fs::copy(dir.path().join(path), copy.path().join(path)).unwrap();
    }
    let mut cloned = VaultStorage::open(copy.path()).unwrap();
    let original = fs::read(copy.path().join("B.md")).unwrap();
    assert!(cloned.list_tag_updates().unwrap().is_empty());
    assert!(cloned.resume_tag_update(&id).is_err());
    assert_eq!(fs::read(copy.path().join("B.md")).unwrap(), original);
}
