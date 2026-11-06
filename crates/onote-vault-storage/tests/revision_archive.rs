use onote_domain::{ErrorCode, NoteDocument, ServiceError};
use onote_vault_storage::{Failpoint, RevisionArchive, VaultStorage};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Archive {
    notes: Mutex<Vec<NoteDocument>>,
    fail: bool,
}
impl RevisionArchive for Archive {
    fn force_preserve(&self, note: &NoteDocument) -> Result<(), ServiceError> {
        self.preserve(note)
    }
    fn preserve(&self, note: &NoteDocument) -> Result<(), ServiceError> {
        if self.fail {
            return Err(ServiceError {
                code: ErrorCode::DiskFull,
                message_key: "error.archiveFull".into(),
                params: Default::default(),
                retryable: false,
                operation_id: None,
                retry_after_seconds: None,
            });
        }
        self.notes.lock().unwrap().push(note.clone());
        Ok(())
    }
    fn verify(&self, note_id: &str, revision: u64, hash: &str) -> Result<(), ServiceError> {
        if self
            .notes
            .lock()
            .unwrap()
            .iter()
            .any(|n| n.note_id == note_id && n.local_revision == revision && n.content_hash == hash)
        {
            return Ok(());
        }
        Err(ServiceError {
            code: ErrorCode::HistoryMissing,
            message_key: "error.historyMissing".into(),
            params: Default::default(),
            retryable: false,
            operation_id: None,
            retry_after_seconds: None,
        })
    }
}

#[test]
fn recovery_refuses_cleanup_when_archive_is_unavailable_then_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "test").unwrap();
    let note = storage.create_note("note.md", "old", &op()).unwrap();
    let operation = op();
    storage.push_failpoint(Failpoint::AfterReplace);
    storage
        .save(&note.note_id, "new", note.local_revision, &operation)
        .unwrap_err();
    let backup = dir.path().join(format!("note.md.onote-old-{operation}"));
    let new_bytes = std::fs::read(dir.path().join("note.md")).unwrap();
    drop(storage);
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    storage.set_revision_archive(Arc::new(Archive {
        fail: true,
        ..Default::default()
    }));
    let recovery = storage.recover().unwrap();
    assert!(recovery.applied.is_empty());
    assert_eq!(recovery.pending, vec![note.note_id.clone()]);
    assert_eq!(std::fs::read(&backup).unwrap(), note.source.as_bytes());
    assert_eq!(
        std::fs::read(dir.path().join("note.md")).unwrap(),
        new_bytes
    );
    let archive = Arc::new(Archive::default());
    storage.set_revision_archive(archive.clone());
    assert_eq!(storage.recover().unwrap().applied, vec![note.note_id]);
    assert_eq!(archive.notes.lock().unwrap()[0].source, note.source);
    assert!(!backup.exists());
}

#[test]
fn recovery_with_missing_old_snapshot_requires_exact_existing_archive() {
    for available in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "test").unwrap();
        let note = storage.create_note("note.md", "old", &op()).unwrap();
        let operation = op();
        storage.push_failpoint(Failpoint::AfterReplace);
        storage
            .save(&note.note_id, "new", note.local_revision, &operation)
            .unwrap_err();
        std::fs::remove_file(dir.path().join(format!("note.md.onote-old-{operation}"))).unwrap();
        drop(storage);
        let mut storage = VaultStorage::open(dir.path()).unwrap();
        let archive = Arc::new(Archive::default());
        if available {
            archive.notes.lock().unwrap().push(note.clone());
        }
        storage.set_revision_archive(archive);
        let recovery = storage.recover().unwrap();
        assert_eq!(recovery.applied.contains(&note.note_id), available);
        assert_eq!(recovery.pending.contains(&note.note_id), !available);
        assert!(storage.read(&note.note_id).unwrap().source.ends_with("new"));
    }
}
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn archive_failure_keeps_old_disk_bytes_and_revision() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "test").unwrap();
    let note = storage.create_note("note.md", "old", &op()).unwrap();
    let old = std::fs::read(dir.path().join("note.md")).unwrap();
    storage.set_revision_archive(Arc::new(Archive {
        fail: true,
        ..Default::default()
    }));
    let error = storage
        .save(&note.note_id, "new", note.local_revision, &op())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::DiskFull);
    assert_eq!(std::fs::read(dir.path().join("note.md")).unwrap(), old);
    assert_eq!(
        storage.read(&note.note_id).unwrap().local_revision,
        note.local_revision
    );
}

#[test]
fn old_original_bytes_are_archived_before_replace_and_recovery_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "test").unwrap();
    let note = storage.create_note("note.md", "old", &op()).unwrap();
    let archive = Arc::new(Archive::default());
    storage.set_revision_archive(archive.clone());
    storage.push_failpoint(Failpoint::AfterReplace);
    storage
        .save(&note.note_id, "new", note.local_revision, &op())
        .unwrap_err();
    assert_eq!(archive.notes.lock().unwrap()[0], note);
    drop(storage);
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    storage.set_revision_archive(archive.clone());
    assert_eq!(
        storage.recover().unwrap().applied,
        vec![note.note_id.clone()]
    );
    assert!(archive
        .notes
        .lock()
        .unwrap()
        .iter()
        .all(|n| n.source == note.source));
}
