use onote_vault_storage::VaultStorage;
use std::fs;
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn refresh(storage: &mut VaultStorage) -> onote_domain::CatalogRefresh {
    let scan = VaultStorage::scan_catalog(storage.root_path()).unwrap();
    storage.refresh_catalog(scan).unwrap()
}

#[test]
fn discovers_new_files_and_follows_unique_embedded_identity_without_rewriting_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "catalog").unwrap();
    let original = storage.create_note("Original.md", "body\n", &op()).unwrap();
    fs::create_dir(dir.path().join("Folder")).unwrap();
    fs::rename(
        dir.path().join("Original.md"),
        dir.path().join("Folder/Moved.md"),
    )
    .unwrap();
    let bytes = fs::read(dir.path().join("Folder/Moved.md")).unwrap();
    fs::write(
        dir.path().join("New.md"),
        "\u{feff}---\r\nunknown: unchanged\r\n---\r\nnew body\r\n",
    )
    .unwrap();
    let result = refresh(&mut storage);
    assert!(!result.deferred);
    assert_eq!(result.notes.len(), 2);
    let moved = storage.read(&original.note_id).unwrap();
    assert_eq!(moved.relative_path, "Folder/Moved.md");
    assert_eq!(moved.local_revision, original.local_revision + 1);
    assert_eq!(fs::read(dir.path().join("Folder/Moved.md")).unwrap(), bytes);
    assert!(fs::read_to_string(dir.path().join("New.md"))
        .unwrap()
        .starts_with('\u{feff}'));
    let again = refresh(&mut storage);
    assert_eq!(again.fingerprint, result.fingerprint);
    assert!(again.changes.is_empty());
    drop(storage);
    let storage = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(
        storage.read(&original.note_id).unwrap().relative_path,
        "Folder/Moved.md"
    );
}

#[test]
fn a_missing_legacy_file_is_not_reidentified_from_equal_content() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Legacy.md"), "same bytes").unwrap();
    let mut storage = VaultStorage::create(dir.path(), "catalog").unwrap();
    let old = storage.list().unwrap().remove(0);
    fs::rename(dir.path().join("Legacy.md"), dir.path().join("Other.md")).unwrap();
    let result = refresh(&mut storage);
    assert!(result.unavailable_note_ids.contains(&old.note_id));
    let new = result
        .notes
        .iter()
        .find(|n| n.relative_path == "Other.md")
        .unwrap();
    assert_ne!(new.note_id, old.note_id);
    assert!(result
        .findings
        .iter()
        .any(|f| f.code == "CatalogLegacyMoveUnresolved"));
    assert_eq!(
        fs::read_to_string(dir.path().join("Other.md")).unwrap(),
        "same bytes"
    );
}

#[test]
fn conflicting_identity_and_duplicates_preserve_baselines_while_other_notes_are_discovered() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "catalog").unwrap();
    let first = storage.create_note("First.md", "first", &op()).unwrap();
    let second = storage.create_note("Second.md", "second", &op()).unwrap();
    let foreign = second.source.replace(&second.note_id, &op());
    fs::write(dir.path().join("Second.md"), &foreign).unwrap();
    fs::copy(dir.path().join("First.md"), dir.path().join("Duplicate.md")).unwrap();
    fs::write(dir.path().join("Healthy.md"), "new healthy").unwrap();
    let result = refresh(&mut storage);
    assert!(result.notes.iter().any(|n| n.relative_path == "Healthy.md"));
    let preserved = result
        .notes
        .iter()
        .find(|n| n.note_id == second.note_id)
        .unwrap();
    assert_eq!(preserved.content_hash, second.content_hash);
    assert!(preserved.read_only);
    assert!(storage.read(&second.note_id).is_err());
    assert!(storage
        .save(&first.note_id, &first.source, first.local_revision, &op())
        .is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("Second.md")).unwrap(),
        foreign
    );
    drop(storage);
    let reopened = VaultStorage::open(dir.path()).unwrap();
    let db =
        rusqlite::Connection::open(reopened.root_path().join(".opennote/state.sqlite")).unwrap();
    let retained: String = db
        .query_row(
            "SELECT note_id FROM notes WHERE relative_path='Second.md'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, second.note_id);
}

#[test]
fn a_scan_cannot_adopt_bytes_that_changed_after_its_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "catalog").unwrap();
    let note = storage.create_note("Note.md", "old", &op()).unwrap();
    fs::write(dir.path().join("New.md"), "before").unwrap();
    let scan = VaultStorage::scan_catalog(dir.path()).unwrap();
    fs::write(dir.path().join("New.md"), "after").unwrap();
    assert!(storage.refresh_catalog(scan).is_err());
    assert_eq!(storage.list().unwrap().len(), 1);
    assert_eq!(
        storage.read(&note.note_id).unwrap().local_revision,
        note.local_revision
    );
}

#[test]
fn pending_save_baselines_are_not_promoted_and_recovery_remains_authoritative() {
    use onote_vault_storage::Failpoint;
    for point in [
        Failpoint::AfterPrepared,
        Failpoint::AfterReplace,
        Failpoint::BeforeDbCommit,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "pending").unwrap();
        let note = storage.create_note("Note.md", "before", &op()).unwrap();
        storage.push_failpoint(point);
        let operation_id = op();
        let requested = note.source.replace("before", "after");
        assert!(storage
            .save(
                &note.note_id,
                &requested,
                note.local_revision,
                &operation_id
            )
            .is_err());
        fs::write(dir.path().join("New.md"), "not yet indexed").unwrap();
        let result = refresh(&mut storage);
        assert!(result.deferred);
        assert!(result.writable);
        assert_eq!(result.notes.len(), 1);
        assert_eq!(result.notes[0].content_hash, note.content_hash);
        storage.recover().unwrap();
        storage
            .save(
                &note.note_id,
                &requested,
                note.local_revision,
                &operation_id,
            )
            .unwrap();
        let result = refresh(&mut storage);
        assert!(!result.deferred);
        assert_eq!(result.notes.len(), 2);
    }
}

#[test]
fn swapping_unique_embedded_notes_updates_mapping_atomically_without_losing_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "swap").unwrap();
    let a = storage.create_note("A.md", "first", &op()).unwrap();
    let b = storage.create_note("B.md", "second", &op()).unwrap();
    fs::rename(dir.path().join("A.md"), dir.path().join("temporary.md")).unwrap();
    fs::rename(dir.path().join("B.md"), dir.path().join("A.md")).unwrap();
    fs::rename(dir.path().join("temporary.md"), dir.path().join("B.md")).unwrap();
    let result = refresh(&mut storage);
    assert!(result.unavailable_note_ids.is_empty());
    assert_eq!(storage.read(&a.note_id).unwrap().relative_path, "B.md");
    assert_eq!(storage.read(&b.note_id).unwrap().relative_path, "A.md");
    assert_eq!(storage.read(&a.note_id).unwrap().source, a.source);
    drop(storage);
    let storage = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(
        storage.read(&a.note_id).unwrap().local_revision,
        a.local_revision + 1
    );
}

#[test]
fn readonly_observation_does_not_write_database_or_source_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "readonly").unwrap();
    storage.create_note("Known.md", "known", &op()).unwrap();
    drop(storage);
    let path = dir.path().join(".opennote/vault.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["writable"] = false.into();
    fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let baseline = fs::read(dir.path().join(".opennote/state.sqlite")).unwrap();
    fs::write(dir.path().join("New.md"), "new external bytes").unwrap();
    let result = refresh(&mut storage);
    assert!(!result.persisted);
    assert!(!result.writable);
    assert_eq!(result.notes.len(), 2);
    assert!(result.notes.iter().all(|note| note.read_only));
    assert_eq!(
        fs::read(dir.path().join(".opennote/state.sqlite")).unwrap(),
        baseline
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("New.md")).unwrap(),
        "new external bytes"
    );
}

#[test]
fn changed_vault_identity_and_cross_root_scans_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "scope").unwrap();
    let scan = VaultStorage::scan_catalog(dir.path()).unwrap();
    let path = dir.path().join(".opennote/vault.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["vaultId"] = op().into();
    fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    assert!(storage.refresh_catalog(scan).is_err());
    let other = tempfile::tempdir().unwrap();
    let _ = VaultStorage::create(other.path(), "other").unwrap();
    assert!(storage
        .refresh_catalog(VaultStorage::scan_catalog(other.path()).unwrap())
        .is_err());
}

#[test]
fn a_foreign_embedded_identity_is_never_adopted_by_an_ordinary_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "identity").unwrap();
    let note = storage.create_note("Note.md", "body", &op()).unwrap();
    fs::write(
        dir.path().join("Note.md"),
        note.source.replace(&note.note_id, &op()),
    )
    .unwrap();
    assert!(storage.read(&note.note_id).is_err());
    let db = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
    let hash: String = db
        .query_row(
            "SELECT content_hash FROM notes WHERE note_id=?1",
            [&note.note_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hash, note.content_hash);
}

#[test]
fn an_open_rescans_unique_external_moves_without_resetting_the_revision() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "reopen").unwrap();
    let note = storage.create_note("Before.md", "before", &op()).unwrap();
    storage
        .save(
            &note.note_id,
            &note.source.replace("before", "current"),
            note.local_revision,
            &op(),
        )
        .unwrap();
    let latest = storage.read(&note.note_id).unwrap();
    drop(storage);
    fs::rename(dir.path().join("Before.md"), dir.path().join("After.md")).unwrap();
    let storage = VaultStorage::open(dir.path()).unwrap();
    let moved = storage.read(&note.note_id).unwrap();
    assert_eq!(moved.relative_path, "After.md");
    assert_eq!(moved.local_revision, latest.local_revision + 1);
    assert_eq!(moved.source, latest.source);
}

#[test]
fn a_blocked_path_reuse_does_not_displace_the_whole_identity_chain() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "blocked chain").unwrap();
    let a = storage.create_note("A.md", "first", &op()).unwrap();
    let b = storage.create_note("B.md", "second", &op()).unwrap();
    let c = storage.create_note("C.md", "third", &op()).unwrap();
    fs::write(dir.path().join("B.md"), &a.source).unwrap();
    fs::write(dir.path().join("C.md"), &b.source).unwrap();
    fs::remove_file(dir.path().join("A.md")).unwrap();
    let result = refresh(&mut storage);
    for note in [&a, &b, &c] {
        let kept = result
            .notes
            .iter()
            .find(|n| n.note_id == note.note_id)
            .unwrap();
        assert_eq!(kept.relative_path, note.relative_path);
        assert_eq!(kept.content_hash, note.content_hash);
        assert!(kept.read_only);
    }
}

#[test]
fn readonly_external_moves_are_visible_without_persisting_a_new_revision() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "readonly move").unwrap();
    let note = storage.create_note("Before.md", "body", &op()).unwrap();
    drop(storage);
    let path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["writable"] = false.into();
    fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let baseline = fs::read(dir.path().join(".opennote/state.sqlite")).unwrap();
    fs::rename(dir.path().join("Before.md"), dir.path().join("After.md")).unwrap();
    let result = refresh(&mut storage);
    assert!(!result.persisted);
    assert!(!result.writable);
    let observed = storage.read(&note.note_id).unwrap();
    assert_eq!(observed.relative_path, "After.md");
    assert_eq!(observed.local_revision, note.local_revision);
    assert!(observed.read_only);
    assert_eq!(
        fs::read(dir.path().join(".opennote/state.sqlite")).unwrap(),
        baseline
    );
}

#[test]
fn corrupted_authoritative_identifiers_are_refused_before_reconciliation() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "corrupted mapping").unwrap();
    let note = storage.create_note("Note.md", "body", &op()).unwrap();
    let db = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
    db.execute(
        "UPDATE notes SET note_id='invalid' WHERE note_id=?1",
        [&note.note_id],
    )
    .unwrap();
    let snapshot = VaultStorage::scan_catalog(dir.path()).unwrap();
    assert!(storage.refresh_catalog(snapshot).is_err());
    let unchanged: (String, bool) = db
        .query_row("SELECT note_id,read_only FROM notes", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(unchanged, ("invalid".into(), false));
    assert_eq!(
        fs::read_to_string(dir.path().join("Note.md")).unwrap(),
        note.source
    );
}

#[test]
fn a_readonly_metadata_change_remains_effective_while_catalog_updates_are_deferred() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "readonly pending").unwrap();
    let note = storage.create_note("Note.md", "original", &op()).unwrap();
    storage.push_failpoint(onote_vault_storage::Failpoint::AfterPrepared);
    assert!(storage
        .save(
            &note.note_id,
            &note.source.replace("original", "pending"),
            note.local_revision,
            &op()
        )
        .is_err());
    let path = dir.path().join(".opennote/vault.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["writable"] = false.into();
    fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let result = refresh(&mut storage);
    assert!(result.deferred);
    assert!(!result.writable);
    assert!(storage
        .create_note("Other.md", "must not write", &op())
        .is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("Note.md")).unwrap(),
        note.source
    );
}

#[test]
fn a_failed_catalog_transaction_rolls_back_every_mapping_and_keeps_external_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "transaction").unwrap();
    let a = storage.create_note("A.md", "first", &op()).unwrap();
    let b = storage.create_note("B.md", "second", &op()).unwrap();
    fs::rename(dir.path().join("A.md"), dir.path().join("temporary.md")).unwrap();
    fs::rename(dir.path().join("B.md"), dir.path().join("A.md")).unwrap();
    fs::rename(dir.path().join("temporary.md"), dir.path().join("B.md")).unwrap();
    let db = rusqlite::Connection::open(dir.path().join(".opennote/state.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER refuse_catalog BEFORE INSERT ON notes WHEN NEW.relative_path='B.md' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(storage
        .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
        .is_err());
    for note in [&a, &b] {
        let path: String = db
            .query_row(
                "SELECT relative_path FROM notes WHERE note_id=?1",
                [&note.note_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(path, note.relative_path);
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("A.md")).unwrap(),
        b.source
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("B.md")).unwrap(),
        a.source
    );
    db.execute_batch("DROP TRIGGER refuse_catalog;").unwrap();
    refresh(&mut storage);
    assert_eq!(
        storage.read(&a.note_id).unwrap().local_revision,
        a.local_revision + 1
    );
    assert_eq!(
        storage.read(&b.note_id).unwrap().local_revision,
        b.local_revision + 1
    );
}
#[test]
fn note_source_snapshots_require_the_captured_identity_and_exact_bytes_without_advancing_revisions()
{
    let dir = tempfile::tempdir().unwrap();
    let mut storage = onote_vault_storage::VaultStorage::create(dir.path(), "Snapshots").unwrap();
    let note = storage
        .create_note("a.md", "alpha", &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let summary = storage.list().unwrap().remove(0);
    let original = std::fs::read(dir.path().join("a.md")).unwrap();
    assert!(
        onote_vault_storage::VaultStorage::read_note_source_snapshot(dir.path(), &summary)
            .unwrap()
            .contains("alpha")
    );
    std::fs::write(
        dir.path().join("a.md"),
        String::from_utf8(original.clone())
            .unwrap()
            .replace("alpha", "beta"),
    )
    .unwrap();
    assert_eq!(
        onote_vault_storage::VaultStorage::read_note_source_snapshot(dir.path(), &summary)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::VersionConflict
    );
    let foreign = format!("---\nopennote-id: {}\n---\nforeign", uuid::Uuid::new_v4());
    std::fs::write(dir.path().join("a.md"), &foreign).unwrap();
    let mut current_hash = summary.clone();
    use sha2::Digest;
    current_hash.content_hash = format!("{:x}", sha2::Sha256::digest(foreign.as_bytes()));
    assert_eq!(
        onote_vault_storage::VaultStorage::read_note_source_snapshot(dir.path(), &current_hash)
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::VersionConflict
    );
    assert_eq!(
        storage.list().unwrap()[0].local_revision,
        note.local_revision
    );
    assert_eq!(
        std::fs::read(dir.path().join("a.md")).unwrap(),
        foreign.as_bytes()
    );
}
