use onote_vault_storage::{SearchCacheSource, VaultStorage};
use std::fs;

fn setup() -> (tempfile::TempDir, VaultStorage, Vec<SearchCacheSource>) {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "Search cache").unwrap();
    storage
        .create_note(
            "a.md",
            "---\nunknown: keep\n---\nİstanbul 中文 alpha",
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
    let notes = storage
        .list()
        .unwrap()
        .into_iter()
        .map(|note| SearchCacheSource {
            note_id: note.note_id.clone(),
            relative_path: note.relative_path.clone(),
            content_hash: note.content_hash.clone(),
            source: VaultStorage::read_note_source_snapshot(dir.path(), &note).unwrap(),
        })
        .collect();
    (dir, storage, notes)
}

#[test]
fn search_cache_survives_reopen_matches_only_captured_sources_and_does_not_take_the_writer_gate() {
    let (dir, storage, sources) = setup();
    let summaries = storage.list().unwrap();
    let before = fs::read(dir.path().join("a.md")).unwrap();
    let gate = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join(".opennote/writer.lock"))
        .unwrap();
    gate.try_lock().unwrap();
    VaultStorage::write_search_cache(
        dir.path(),
        &storage.vault().vault_id,
        sources.iter(),
        |_, _| true,
    )
    .unwrap();
    let loaded = VaultStorage::read_search_cache(
        dir.path(),
        &storage.vault().vault_id,
        &summaries,
        |_, _| true,
    )
    .unwrap();
    assert_eq!(loaded[&summaries[0].note_id].source, sources[0].source);
    let mut changed = summaries.clone();
    changed[0].content_hash = "0".repeat(64);
    assert!(VaultStorage::read_search_cache(
        dir.path(),
        &storage.vault().vault_id,
        &changed,
        |_, _| true
    )
    .unwrap()
    .is_empty());
    assert!(VaultStorage::read_search_cache(
        dir.path(),
        &uuid::Uuid::new_v4().to_string(),
        &summaries,
        |_, _| true
    )
    .is_err());
    assert_eq!(fs::read(dir.path().join("a.md")).unwrap(), before);
    assert_eq!(storage.list().unwrap(), summaries);
    assert!(dir.path().join(".opennote/index.sqlite").is_file());
}

#[test]
fn cancelled_cache_publication_preserves_previous_complete_database_and_cleans_owned_stages() {
    let (dir, storage, sources) = setup();
    let vault = &storage.vault().vault_id;
    VaultStorage::write_search_cache(dir.path(), vault, sources.iter(), |_, _| true).unwrap();
    let before = fs::read(dir.path().join(".opennote/index.sqlite")).unwrap();
    let mut checks = 0;
    let result = VaultStorage::write_search_cache(dir.path(), vault, sources.iter(), |_, _| {
        checks += 1;
        checks < 4
    });
    assert_eq!(
        result.unwrap_err().code,
        onote_domain::ErrorCode::OperationCancelled
    );
    assert_eq!(
        fs::read(dir.path().join(".opennote/index.sqlite")).unwrap(),
        before
    );
    assert!(!fs::read_dir(dir.path().join(".opennote"))
        .unwrap()
        .any(|item| item
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".index-stage-")));
}

#[test]
fn corrupt_derived_database_can_be_rebuilt_without_losing_its_bytes_or_authoritative_state() {
    let (dir, storage, sources) = setup();
    let state = fs::read(dir.path().join(".opennote/state.sqlite")).unwrap();
    let corrupt = b"broken cache, not a note";
    fs::write(dir.path().join(".opennote/index.sqlite"), corrupt).unwrap();
    assert!(VaultStorage::read_search_cache(
        dir.path(),
        &storage.vault().vault_id,
        &storage.list().unwrap(),
        |_, _| true
    )
    .is_err());
    VaultStorage::write_search_cache(
        dir.path(),
        &storage.vault().vault_id,
        sources.iter(),
        |_, _| true,
    )
    .unwrap();
    assert_eq!(
        VaultStorage::read_search_cache(
            dir.path(),
            &storage.vault().vault_id,
            &storage.list().unwrap(),
            |_, _| true
        )
        .unwrap()
        .len(),
        1
    );
    assert!(fs::read_dir(dir.path().join(".opennote"))
        .unwrap()
        .any(|item| {
            let path = item.unwrap().path();
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("index-rejected-")
                && fs::read(path).unwrap() == corrupt
        }));
    assert_eq!(
        fs::read(dir.path().join(".opennote/state.sqlite")).unwrap(),
        state
    );
}

#[test]
fn tampered_source_unknown_versions_readonly_metadata_and_foreign_roots_do_not_bypass_cache_boundaries(
) {
    let (dir, storage, sources) = setup();
    let vault = &storage.vault().vault_id;
    let path = dir.path().join(".opennote/index.sqlite");
    VaultStorage::write_search_cache(dir.path(), vault, sources.iter(), |_, _| true).unwrap();
    let other = tempfile::tempdir().unwrap();
    let other_storage = VaultStorage::create(other.path(), "Other").unwrap();
    fs::copy(
        dir.path().join(".opennote/vault.json"),
        other.path().join(".opennote/vault.json"),
    )
    .unwrap();
    fs::copy(&path, other.path().join(".opennote/index.sqlite")).unwrap();
    assert!(VaultStorage::read_search_cache(
        other.path(),
        vault,
        &storage.list().unwrap(),
        |_, _| true
    )
    .unwrap()
    .is_empty());
    drop(other_storage);
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("UPDATE sources SET source='forged body'", [])
            .unwrap();
    }
    assert!(VaultStorage::read_search_cache(
        dir.path(),
        vault,
        &storage.list().unwrap(),
        |_, _| true
    )
    .unwrap()
    .is_empty());
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.pragma_update(None, "user_version", 999).unwrap();
    }
    let future = fs::read(&path).unwrap();
    assert!(
        VaultStorage::write_search_cache(dir.path(), vault, sources.iter(), |_, _| true).is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), future);
    let meta_path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["writable"] = false.into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    assert!(
        VaultStorage::write_search_cache(dir.path(), vault, sources.iter(), |_, _| true).is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), future);
}

#[test]
fn independent_cache_lock_and_foreign_sqlite_sidecars_prevent_publication() {
    let (dir, storage, sources) = setup();
    let app = dir.path().join(".opennote");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(app.join("index.writer.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(VaultStorage::write_search_cache(
        dir.path(),
        &storage.vault().vault_id,
        sources.iter(),
        |_, _| true
    )
    .is_err());
    drop(lock);
    fs::write(app.join("index.sqlite-wal"), b"foreign writer").unwrap();
    assert!(VaultStorage::write_search_cache(
        dir.path(),
        &storage.vault().vault_id,
        sources.iter(),
        |_, _| true
    )
    .is_err());
    assert_eq!(
        fs::read(app.join("index.sqlite-wal")).unwrap(),
        b"foreign writer"
    );
    assert!(!app.join("index.sqlite").exists());
}

#[test]
fn immutable_reads_handle_unicode_percent_spaces_and_never_create_sqlite_sidecars() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("索引 % #");
    let mut storage = VaultStorage::create(&root, "Unicode cache").unwrap();
    storage
        .create_note("a.md", "alpha", &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let summaries = storage.list().unwrap();
    let source = SearchCacheSource {
        note_id: summaries[0].note_id.clone(),
        relative_path: "a.md".into(),
        content_hash: summaries[0].content_hash.clone(),
        source: fs::read_to_string(root.join("a.md")).unwrap(),
    };
    VaultStorage::write_search_cache(
        &root,
        &storage.vault().vault_id,
        [&source].into_iter(),
        |_, _| true,
    )
    .unwrap();
    assert_eq!(
        VaultStorage::read_search_cache(&root, &storage.vault().vault_id, &summaries, |_, _| true)
            .unwrap()
            .len(),
        1
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!root
            .join(format!(".opennote/index.sqlite{suffix}"))
            .exists());
    }
    assert_eq!(
        VaultStorage::read_search_cache(&root, &storage.vault().vault_id, &summaries, |_, _| false)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::OperationCancelled
    );
}
