use onote_vault_storage::{Failpoint, VaultStorage};
use std::fs;

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn upgrade_cannot_reuse_a_note_operation_id() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let mut storage = VaultStorage::create(source.path(), "operation scope").unwrap();
    let operation = op();
    storage.create_note("a.md", "original", &operation).unwrap();
    drop(storage);
    let path = source.path().join(".opennote/vault.json");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["schemaVersion"] = 1.into();
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let original = fs::read(&path).unwrap();
    let storage = VaultStorage::open(source.path()).unwrap();
    assert_eq!(
        storage
            .apply_schema_upgrade(
                &destination.path().join("state.sqlite"),
                &operation,
                &"a".repeat(64),
                |_, _, _, _| Ok(())
            )
            .unwrap_err()
            .code,
        onote_domain::ErrorCode::IdempotencyMismatch
    );
    assert_eq!(fs::read(path).unwrap(), original);
    assert!(!source.path().join(".opennote/upgrade-v1-v2").exists());
}

#[test]
fn interrupted_upgrade_stays_readonly_and_retries_the_same_durable_request() {
    for stop in [
        Failpoint::UpgradePrepared,
        Failpoint::UpgradeDatabase,
        Failpoint::UpgradeMetadata,
    ] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(source.path(), "upgrade").unwrap();
        let note = storage.create_note("a.md", "original", &op()).unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        meta["custom"] = serde_json::json!({"keep":true});
        fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let db = rusqlite::Connection::open(source.path().join(".opennote/state.sqlite")).unwrap();
        db.execute_batch("DROP TABLE draft_checkpoints; DROP TABLE snapshot_restores; CREATE TABLE old_queue(payload TEXT); INSERT INTO old_queue VALUES ('unconfirmed original work');").unwrap();
        drop(db);
        let storage = VaultStorage::open(source.path()).unwrap();
        let operation = op();
        let manifest = "a".repeat(64);
        storage.push_failpoint(stop);
        assert!(storage
            .apply_schema_upgrade(
                &destination.path().join("state.sqlite"),
                &operation,
                &manifest,
                |_, _, _, resume| {
                    assert!(!resume);
                    Ok(())
                }
            )
            .is_err());
        drop(storage);
        let reopened = VaultStorage::open(source.path()).unwrap();
        assert!(!reopened.vault().writable);
        assert!(reopened
            .vault()
            .findings
            .iter()
            .any(|f| f.code == "SchemaUpgradeInterrupted"));
        assert!(reopened
            .apply_schema_upgrade(
                &destination.path().join("retry.sqlite"),
                &op(),
                &manifest,
                |_, _, _, _| Ok(())
            )
            .is_err());
        assert!(reopened
            .apply_schema_upgrade(
                &destination.path().join("retry.sqlite"),
                &operation,
                &"b".repeat(64),
                |_, _, _, _| Ok(())
            )
            .is_err());
        let result = reopened
            .apply_schema_upgrade(
                &destination.path().join("retry.sqlite"),
                &operation,
                &manifest,
                |_, _, _, resume| {
                    assert!(resume);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(result.to_schema, 3);
        let replay = reopened
            .apply_schema_upgrade(
                &destination.path().join("unused.sqlite"),
                &operation,
                &manifest,
                |_, _, _, _| panic!("completed replay needs no mutation"),
            )
            .unwrap();
        assert_eq!(result, replay);
        drop(reopened);
        let current = VaultStorage::open(source.path()).unwrap();
        assert!(current.vault().writable);
        assert_eq!(current.vault().schema_version, 3);
        let opened = current.read(&note.note_id).unwrap();
        assert_eq!(opened.source, note.source);
        assert_eq!(opened.local_revision, note.local_revision);
        assert_eq!(
            fs::read(source.path().join("a.md")).unwrap(),
            note.source.as_bytes()
        );
        let after: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(after["custom"], meta["custom"]);
        assert_eq!(after["vaultId"], meta["vaultId"]);
        let db = rusqlite::Connection::open(source.path().join(".opennote/state.sqlite")).unwrap();
        assert_eq!(
            db.query_row("SELECT payload FROM old_queue", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "unconfirmed original work"
        );
    }
}

#[test]
fn refused_verification_never_starts_and_damaged_or_changed_requests_never_resume() {
    for scenario in [
        "verification",
        "journal",
        "metadata",
        "completion",
        "database",
    ] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "refusal").unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let before = fs::read(&path).unwrap();
        let storage = VaultStorage::open(source.path()).unwrap();
        let operation = op();
        let manifest = "a".repeat(64);
        if scenario == "verification" {
            assert!(storage
                .apply_schema_upgrade(
                    &destination.path().join("export.sqlite"),
                    &operation,
                    &manifest,
                    |_, _, _, _| Err(onote_domain::ServiceError {
                        code: onote_domain::ErrorCode::BackupIncomplete,
                        message_key: "error.backupIncomplete".into(),
                        params: Default::default(),
                        retryable: false,
                        operation_id: None,
                        retry_after_seconds: None
                    })
                )
                .is_err());
            assert!(!source.path().join(".opennote/upgrade-v1-v2").exists());
            assert_eq!(fs::read(path).unwrap(), before);
            continue;
        }
        storage.push_failpoint(Failpoint::UpgradePrepared);
        assert!(storage
            .apply_schema_upgrade(
                &destination.path().join("export.sqlite"),
                &operation,
                &manifest,
                |_, _, _, _| Ok(())
            )
            .is_err());
        match scenario {
            "database" => fs::remove_file(source.path().join(".opennote/state.sqlite")).unwrap(),
            "journal" => fs::write(
                source.path().join(".opennote/upgrade-v1-v2/request.json"),
                b"partial corrupt request",
            )
            .unwrap(),
            "completion" => fs::write(
                source.path().join(".opennote/upgrade-v1-v2/complete"),
                b"wrong checksum",
            )
            .unwrap(),
            _ => {
                meta["custom"] = true.into();
                fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
            }
        }
        let modified = fs::read(&path).unwrap();
        let reopened = VaultStorage::open(source.path()).unwrap();
        assert!(!reopened.vault().writable);
        assert!(reopened
            .apply_schema_upgrade(
                &destination.path().join("retry.sqlite"),
                &operation,
                &manifest,
                |_, _, _, _| {
                    if scenario == "database" {
                        Ok(())
                    } else {
                        panic!("integrity refusal must precede verifier")
                    }
                }
            )
            .is_err());
        assert_eq!(fs::read(path).unwrap(), modified);
        if scenario == "database" {
            assert!(!source.path().join(".opennote/state.sqlite").exists());
        }
    }
}
