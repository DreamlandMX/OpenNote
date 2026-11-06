use onote_domain::ErrorCode;
use onote_vault_storage::VaultStorage;
use std::fs;

#[test]
fn legacy_catalog_and_embedded_identity_collision_blocks_ambiguous_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "collision").unwrap();
    let note = vault
        .create_note("plain.md", "original", &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(vault);
    let meta_path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    fs::copy(dir.path().join("plain.md"), dir.path().join("copy.md")).unwrap();
    fs::write(dir.path().join("plain.md"), b"without embedded identity").unwrap();
    let readonly = VaultStorage::open(dir.path()).unwrap();
    assert!(readonly
        .vault()
        .findings
        .iter()
        .any(|f| f.code == "DuplicateNoteId"));
    assert_eq!(readonly.list().unwrap_err().code, ErrorCode::RescanRequired);
    assert_eq!(
        readonly.read(&note.note_id).unwrap_err().code,
        ErrorCode::RescanRequired
    );
    assert_eq!(
        fs::read(dir.path().join("plain.md")).unwrap(),
        b"without embedded identity"
    );
}

#[test]
fn future_schema_does_not_interpret_or_initialize_its_database() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "future").unwrap();
    let note = vault
        .create_note("future.md", "original", &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(vault);
    let meta_path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 999.into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let database = dir.path().join(".opennote/state.sqlite");
    fs::write(&database, b"unknown future database format").unwrap();
    let readonly = VaultStorage::open(dir.path()).unwrap();
    assert!(!readonly.vault().writable);
    assert_eq!(readonly.vault().schema_version, 999);
    assert!(readonly
        .vault()
        .findings
        .iter()
        .any(|f| f.code == "SchemaNewerThanSupported"));
    assert!(!readonly
        .vault()
        .findings
        .iter()
        .any(|f| f.code == "LegacyCatalogUnavailable"));
    assert_eq!(readonly.read(&note.note_id).unwrap().source, note.source);
    assert_eq!(
        fs::read(database).unwrap(),
        b"unknown future database format"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(meta_path).unwrap()).unwrap(),
        meta
    );
}

#[test]
fn version_one_open_is_readonly_and_preserves_notes_journal_and_unknown_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "legacy").unwrap();
    let op = uuid::Uuid::new_v4().to_string();
    let note = vault.create_note("中文.md", "原文", &op).unwrap();
    drop(vault);
    let meta_path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    meta["custom"] = serde_json::json!({"unknown":"不得丢失"});
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let before_meta = fs::read(&meta_path).unwrap();
    let before_db = fs::read(dir.path().join(".opennote/state.sqlite")).unwrap();
    let before_note = fs::read(dir.path().join("中文.md")).unwrap();
    let mut vault = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(vault.vault().schema_version, 1);
    assert!(!vault.vault().writable);
    assert!(vault
        .vault()
        .findings
        .iter()
        .any(|finding| finding.code == "SchemaUpgradeRequired"));
    let opened = vault.read(&note.note_id).unwrap();
    assert_eq!(opened.source, note.source);
    assert_eq!(opened.local_revision, note.local_revision);
    assert_eq!(opened.content_hash, note.content_hash);
    assert!(opened.read_only);
    let refusal = vault.create_note("中文.md", "原文", &op).unwrap_err();
    assert_eq!(refusal.code, ErrorCode::InvalidInput);
    assert_eq!(refusal.message_key, "error.schemaReadOnly");
    assert_eq!(fs::read(&meta_path).unwrap(), before_meta);
    assert_eq!(
        fs::read(dir.path().join(".opennote/state.sqlite")).unwrap(),
        before_db
    );
    assert_eq!(fs::read(dir.path().join("中文.md")).unwrap(), before_note);
    let after: serde_json::Value = serde_json::from_slice(&fs::read(meta_path).unwrap()).unwrap();
    assert_eq!(after["custom"], meta["custom"]);
    assert_eq!(after["vaultId"], meta["vaultId"]);
    assert_eq!(after["schemaVersion"], 1);
}

#[test]
fn legacy_plain_note_keeps_catalog_identity_without_advancing_revision_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "legacy mapping").unwrap();
    let note = vault
        .create_note("plain.md", "original", &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(vault);
    let meta_path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    fs::write(dir.path().join("plain.md"), b"external\r\n").unwrap();
    let database = fs::read(dir.path().join(".opennote/state.sqlite")).unwrap();
    for _ in 0..2 {
        let readonly = VaultStorage::open(dir.path()).unwrap();
        assert!(!readonly.vault().writable);
        let current = readonly.read(&note.note_id).unwrap();
        assert_eq!(current.note_id, note.note_id);
        assert_eq!(current.source, "external\r\n");
        assert_eq!(current.local_revision, note.local_revision);
        assert!(current.read_only);
    }
    assert_eq!(
        fs::read(dir.path().join(".opennote/state.sqlite")).unwrap(),
        database
    );
}

#[test]
fn legacy_without_catalog_opens_raw_readonly_without_creating_a_database() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "no catalog").unwrap();
    let note = vault
        .create_note("a.md", "keep", &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(vault);
    let meta_path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    fs::remove_file(dir.path().join(".opennote/state.sqlite")).unwrap();
    fs::remove_file(dir.path().join(".opennote/writer.lock")).unwrap();
    let readonly = VaultStorage::open(dir.path()).unwrap();
    assert!(!readonly.vault().writable);
    assert_eq!(readonly.read(&note.note_id).unwrap().source, note.source);
    assert!(readonly
        .vault()
        .findings
        .iter()
        .any(|finding| finding.code == "LegacyCatalogUnavailable"));
    assert!(!dir.path().join(".opennote/state.sqlite").exists());
    assert!(!dir.path().join(".opennote/writer.lock").exists());
}

#[test]
fn legacy_readonly_open_preserves_uncheckpointed_wal_and_original_queue_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "legacy WAL").unwrap();
    let note = vault
        .create_note("a.md", "unchanged", &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(vault);
    let database = dir.path().join(".opennote/state.sqlite");
    let conn = rusqlite::Connection::open(&database).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE original_queue(payload TEXT NOT NULL); INSERT INTO original_queue VALUES ('unconfirmed original payload');").unwrap();
    let meta_path = dir.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let wal = dir.path().join(".opennote/state.sqlite-wal");
    let before_db = fs::read(&database).unwrap();
    let before_wal = fs::read(&wal).unwrap();
    let readonly = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(readonly.read(&note.note_id).unwrap().source, note.source);
    assert!(!readonly.vault().writable);
    assert_eq!(fs::read(database).unwrap(), before_db);
    assert_eq!(fs::read(wal).unwrap(), before_wal);
    assert_eq!(
        conn.query_row("SELECT payload FROM original_queue", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "unconfirmed original payload"
    );
}
