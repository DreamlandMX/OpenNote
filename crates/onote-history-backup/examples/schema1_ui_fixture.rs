use onote_history_backup::BackupStore;
use onote_vault_storage::VaultStorage;
use std::{fs, sync::atomic::AtomicBool};

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn main() {
    let fixture = std::env::temp_dir().join(format!("opennote-schema1-ui-{}", op()));
    let source = fixture.join("source");
    let destination = fixture.join("backups");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();

    let mut vault = VaultStorage::create(&source, "Schema 1 restore fixture").unwrap();
    vault
        .create_note(
            "legacy.md",
            "# Legacy restore\n\nRead-only schema 1 body.",
            &op(),
        )
        .unwrap();
    drop(vault);

    let metadata_path = source.join(".opennote/vault.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["schemaVersion"] = 1.into();
    metadata["custom"] = "preserve".into();
    fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

    let connection = rusqlite::Connection::open(source.join(".opennote/state.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE old_queue(payload TEXT);
             INSERT INTO old_queue VALUES ('keep legacy work');",
        )
        .unwrap();
    drop(connection);

    let vault = VaultStorage::open(&source).unwrap();
    assert_eq!(vault.vault().schema_version, 1);
    assert!(!vault.vault().writable);
    let backup = BackupStore::open(&source, &vault.vault().vault_id, &destination).unwrap();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();

    println!(
        "{}",
        serde_json::json!({
            "fixture": fixture,
            "source": source,
            "destination": destination,
            "snapshot": snapshot
        })
    );
}
