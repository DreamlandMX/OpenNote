use onote_domain::{RenameBaseline, RenameEdit, RenamePlan};
use onote_vault_storage::{Failpoint, VaultStorage};
use std::fs;

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn fixture() -> (tempfile::TempDir, VaultStorage, RenamePlan) {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "rename").unwrap();
    let target = storage.create_note("Old.md", "target\r\n", &op()).unwrap();
    let linked = storage
        .create_note(
            "Linked.md",
            "\u{feff}---\r\ncustom: keep\r\n---\r\n[[Old|label]] prose Old\r\n",
            &op(),
        )
        .unwrap();
    let offset = linked.source.find("[[Old|label]]").unwrap();
    let baseline = |d: &onote_domain::NoteDocument| RenameBaseline {
        note_id: d.note_id.clone(),
        relative_path: d.relative_path.clone(),
        expected_revision: d.local_revision,
        content_hash: d.content_hash.clone(),
    };
    let plan = RenamePlan {
        plan_id: op(),
        vault_id: storage.vault().vault_id,
        note_id: target.note_id.clone(),
        from_path: target.relative_path.clone(),
        to_path: "Folder/New.md".into(),
        keeps_identity: true,
        expected_hashes: vec![baseline(&target), baseline(&linked)],
        edits: vec![RenameEdit {
            note_id: linked.note_id,
            relative_path: linked.relative_path,
            offset,
            length: "[[Old|label]]".len(),
            before: "[[Old|label]]".into(),
            after: "[[Folder/New|label]]".into(),
        }],
        untouched: vec![],
    };
    (dir, storage, plan)
}

#[test]
fn move_rewrites_only_ranges_preserves_exact_bytes_and_replays() {
    let (dir, storage, plan) = fixture();
    let old = fs::read(dir.path().join("Linked.md")).unwrap();
    let id = op();
    let result = storage.rename(&plan, 0, &id).unwrap();
    assert!(result.completed);
    assert_eq!(result.items.len(), 2);
    assert!(!dir.path().join("Old.md").exists());
    assert_eq!(
        storage.read(&plan.note_id).unwrap().relative_path,
        "Folder/New.md"
    );
    let source = String::from_utf8(old)
        .unwrap()
        .replace("[[Old|label]]", "[[Folder/New|label]]");
    assert_eq!(
        fs::read(dir.path().join("Linked.md")).unwrap(),
        source.as_bytes()
    );
    assert_eq!(storage.rename(&plan, 0, &id).unwrap(), result);
    drop(storage);
    let storage = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(storage.rename(&plan, 0, &id).unwrap(), result);
    let mut changed = plan.clone();
    changed.to_path = "other.md".into();
    assert!(storage.rename(&changed, 0, &id).is_err());
}

#[test]
fn stale_bytes_or_collision_never_publish_any_changes() {
    for collision in [false, true] {
        let (dir, storage, plan) = fixture();
        if collision {
            fs::create_dir(dir.path().join("Folder")).unwrap();
            fs::write(dir.path().join("Folder/new.MD"), "occupied").unwrap();
        } else {
            fs::write(dir.path().join("Linked.md"), "external edit").unwrap();
        }
        assert!(storage.rename(&plan, 0, &op()).is_err());
        assert!(dir.path().join("Old.md").exists());
        assert_eq!(
            storage
                .list()
                .unwrap()
                .iter()
                .find(|n| n.note_id == plan.note_id)
                .unwrap()
                .local_revision,
            0
        );
    }
}

#[test]
fn interrupted_move_reopens_without_reconciling_the_baseline_and_resumes_once() {
    for stop in [
        Failpoint::AfterPrepared,
        Failpoint::BeforeReplace,
        Failpoint::AfterReplace,
        Failpoint::BeforeDbCommit,
    ] {
        let (dir, storage, plan) = fixture();
        let id = op();
        storage.push_failpoint(stop);
        assert!(storage.rename(&plan, 0, &id).is_err());
        drop(storage);
        let storage = VaultStorage::open(dir.path()).unwrap();
        let notes = storage.list().unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(
            notes
                .iter()
                .find(|n| n.note_id == plan.note_id)
                .unwrap()
                .relative_path,
            "Old.md"
        );
        let result = storage.rename(&plan, 0, &id).unwrap();
        assert!(result.completed);
        assert!(result.items.iter().all(|i| i.local_revision == 1));
        assert_eq!(storage.rename(&plan, 0, &id).unwrap(), result);
    }
}

#[test]
fn external_mutation_stops_resume_and_keeps_journal_and_original() {
    let (dir, storage, plan) = fixture();
    let id = op();
    storage.push_failpoint(Failpoint::AfterReplace);
    assert!(storage.rename(&plan, 0, &id).is_err());
    fs::write(dir.path().join("Linked.md"), "external edit").unwrap();
    drop(storage);
    let storage = VaultStorage::open(dir.path()).unwrap();
    assert!(storage.rename(&plan, 0, &id).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("Linked.md")).unwrap(),
        "external edit"
    );
    assert!(dir
        .path()
        .join(format!(".opennote/rename-{id}-0.old"))
        .exists());
    assert!(!storage.recover().unwrap().pending.is_empty());
}

#[test]
fn case_only_rename_preserves_identity_and_casing() {
    let (dir, storage, mut plan) = fixture();
    plan.to_path = "old.md".into();
    plan.edits.clear();
    plan.expected_hashes.truncate(1);
    let id = op();
    storage.rename(&plan, 0, &id).unwrap();
    let names = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(names.contains(&"old.md".to_owned()));
    assert!(!names.contains(&"Old.md".to_owned()));
    assert_eq!(storage.read(&plan.note_id).unwrap().relative_path, "old.md");
}

#[test]
fn duplicate_paths_and_case_only_gaps_recover_without_losing_the_catalog() {
    for stop in [
        Failpoint::RenamePublished,
        Failpoint::RenameSourceRemoved,
        Failpoint::RenameItemCommitted,
    ] {
        let (dir, storage, mut plan) = fixture();
        if stop == Failpoint::RenameSourceRemoved {
            plan.to_path = "old.md".into();
        }
        let id = op();
        storage.push_failpoint(stop);
        assert!(storage.rename(&plan, 0, &id).is_err());
        drop(storage);
        let storage = VaultStorage::open(dir.path()).unwrap();
        assert_eq!(storage.list().unwrap().len(), 2);
        assert!(storage.vault().writable);
        let report = storage.recover().unwrap();
        assert!(report.pending.is_empty(), "{stop:?}: {report:?}");
        let result = storage.rename(&plan, 0, &id).unwrap();
        assert!(result.completed);
        assert!(result.items.iter().all(|i| i.local_revision == 1));
    }
}

#[test]
fn deleted_source_before_publication_is_an_external_change() {
    for stop in [Failpoint::AfterPrepared, Failpoint::BeforeReplace] {
        let (dir, storage, plan) = fixture();
        let id = op();
        storage.push_failpoint(stop);
        assert!(storage.rename(&plan, 0, &id).is_err());
        fs::remove_file(dir.path().join("Old.md")).unwrap();
        drop(storage);
        let storage = VaultStorage::open(dir.path()).unwrap();
        assert!(storage.rename(&plan, 0, &id).is_err());
        assert!(!dir.path().join("Folder/New.md").exists());
    }
}

#[test]
fn unrelated_duplicate_identity_still_blocks_recovery() {
    let (dir, storage, plan) = fixture();
    storage.push_failpoint(Failpoint::RenamePublished);
    assert!(storage.rename(&plan, 0, &op()).is_err());
    fs::copy(
        dir.path().join("Linked.md"),
        dir.path().join("Duplicate.md"),
    )
    .unwrap();
    drop(storage);
    let storage = VaultStorage::open(dir.path()).unwrap();
    assert!(!storage.vault().writable);
    assert!(storage
        .vault()
        .findings
        .iter()
        .any(|f| f.code == "DuplicateNoteId"));
    assert!(!storage.recover().unwrap().pending.is_empty());
    assert!(dir.path().join("Old.md").exists());
}

#[test]
fn partial_commit_refuses_mutations_and_changed_completed_files() {
    let (dir, mut storage, plan) = fixture();
    let id = op();
    storage.push_failpoint(Failpoint::RenameItemCommitted);
    assert!(storage.rename(&plan, 0, &id).is_err());
    assert!(storage.create_note("Other.md", "blocked", &op()).is_err());
    let linked = &plan.expected_hashes[1];
    assert!(storage
        .save(&linked.note_id, "blocked", linked.expected_revision, &op())
        .is_err());
    fs::write(
        dir.path().join("Folder/New.md"),
        "external change after item commit",
    )
    .unwrap();
    drop(storage);
    let storage = VaultStorage::open(dir.path()).unwrap();
    assert!(storage.rename(&plan, 0, &id).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("Folder/New.md")).unwrap(),
        "external change after item commit"
    );
}

#[test]
fn legacy_note_moves_without_inserting_or_normalizing_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = "\u{feff}---\r\ncustom: [one, two]\r\n---\r\noriginal\r\n";
    fs::write(dir.path().join("Legacy.md"), bytes).unwrap();
    let storage = VaultStorage::create(dir.path(), "legacy rename").unwrap();
    let note = storage.list().unwrap().remove(0);
    let plan = RenamePlan {
        plan_id: op(),
        vault_id: storage.vault().vault_id,
        note_id: note.note_id.clone(),
        from_path: note.relative_path.clone(),
        to_path: "Folder/Legacy.md".into(),
        keeps_identity: true,
        edits: vec![],
        untouched: vec![],
        expected_hashes: vec![RenameBaseline {
            note_id: note.note_id.clone(),
            relative_path: note.relative_path,
            expected_revision: note.local_revision,
            content_hash: note.content_hash,
        }],
    };
    storage.rename(&plan, 0, &op()).unwrap();
    drop(storage);
    let storage = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(storage.read(&note.note_id).unwrap().source, bytes);
    assert_eq!(
        fs::read(dir.path().join("Folder/Legacy.md")).unwrap(),
        bytes.as_bytes()
    );
}

#[test]
fn history_failure_prevents_the_first_file_publication() {
    struct Refused;
    impl onote_vault_storage::RevisionArchive for Refused {
        fn preserve(
            &self,
            _: &onote_domain::NoteDocument,
        ) -> Result<(), onote_domain::ServiceError> {
            unreachable!()
        }
        fn verify(&self, _: &str, _: u64, _: &str) -> Result<(), onote_domain::ServiceError> {
            unreachable!()
        }
        fn force_preserve(
            &self,
            _: &onote_domain::NoteDocument,
        ) -> Result<(), onote_domain::ServiceError> {
            Err(onote_domain::ServiceError {
                code: onote_domain::ErrorCode::IoDenied,
                message_key: "error.historyUnavailable".into(),
                params: Default::default(),
                retryable: false,
                operation_id: None,
                retry_after_seconds: None,
            })
        }
    }
    let (dir, mut storage, plan) = fixture();
    storage.set_revision_archive(std::sync::Arc::new(Refused));
    let error = storage.rename(&plan, 0, &op()).unwrap_err();
    assert_eq!(error.params["completedItems"], "0");
    assert_eq!(error.params["totalItems"], "2");
    assert!(dir.path().join("Old.md").exists());
    assert!(!dir.path().join("Folder/New.md").exists());
}

#[test]
fn an_untrusted_edit_cannot_change_the_embedded_note_identity() {
    let (dir, storage, mut plan) = fixture();
    let source = storage.read(&plan.note_id).unwrap().source;
    let id = plan.note_id.clone();
    plan.expected_hashes.truncate(1);
    plan.edits = vec![RenameEdit {
        note_id: id.clone(),
        relative_path: plan.from_path.clone(),
        offset: source.find(&id).unwrap(),
        length: id.len(),
        before: id,
        after: op(),
    }];
    assert!(storage.rename(&plan, 0, &op()).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("Old.md")).unwrap(),
        source
    );
    assert!(!dir.path().join("Folder/New.md").exists());
}
