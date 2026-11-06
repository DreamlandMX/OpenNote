use onote_domain::ErrorCode;
use onote_vault_storage::VaultStorage;
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn conflict_copy_has_new_identity_and_retains_original_file_and_unknown_yaml() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "copy").unwrap();
    let note = vault
        .create_note(
            "original.md",
            "---\ncustom: '原样' # 注释\n---\n正文",
            &op(),
        )
        .unwrap();
    let local = note.source.replace("正文", "未保存正文");
    let operation = op();
    let copy = vault
        .create_copy(&note.note_id, "保留副本.md", &local, &operation)
        .unwrap();
    assert_ne!(copy.note_id, note.note_id);
    assert_eq!(copy.source, local.replace(&note.note_id, &copy.note_id));
    assert_eq!(vault.read(&note.note_id).unwrap(), note);
    assert_eq!(
        vault
            .create_copy(&note.note_id, "保留副本.md", &local, &operation)
            .unwrap(),
        copy
    );
    assert_eq!(
        vault
            .create_note("保留副本.md", &local, &operation)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        vault
            .create_copy(&note.note_id, "original.md", &local, &op())
            .unwrap_err()
            .code,
        ErrorCode::PathCollision
    );
    assert_eq!(vault.read(&note.note_id).unwrap(), note);
}

#[test]
fn copy_rebinds_only_reserved_identity_in_quoted_flow_and_alias_forms() {
    for shape in ["plain", "quoted", "flow", "alias", "escaped"] {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = VaultStorage::create(dir.path(), "copy").unwrap();
        let note = vault.create_note("original.md", "body", &op()).unwrap();
        let id = &note.note_id;
        let yaml = match shape {
            "plain" => format!("opennote-id: {id} # reserved\nother: '{id}'"),
            "quoted" => format!("\"opennote-id\": \"{id}\" # reserved\nother: '{id}'"),
            "flow" => format!("{{opennote-id: '{id}', other: '{id}'}}"),
            "alias" => format!("other: &source '{id}'\nopennote-id: *source"),
            _ => format!(
                "opennote-id: \"\\x{}{}\"\nother: '{id}'",
                hex::encode(&id.as_bytes()[..1]),
                &id[1..]
            ),
        };
        let source = format!("---\n{yaml}\n---\nbody {id}");
        let copy = vault.create_copy(id, "copy.md", &source, &op()).unwrap();
        assert!(
            copy.source.contains(&format!("other: '{id}'"))
                || copy.source.contains(&format!("other: &source '{id}'"))
        );
        assert!(copy.source.ends_with(&format!("body {id}")));
        assert_eq!(vault.read(&copy.note_id).unwrap().source, copy.source);
        assert_eq!(vault.read(id).unwrap(), note);
    }
}

#[test]
fn foreign_or_shared_anchored_identity_refuses_without_changing_unknown_properties() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "copy").unwrap();
    let note = vault.create_note("original.md", "body", &op()).unwrap();
    let foreign = format!("---\nopennote-id: {}\n---\nbody", op());
    let shared = format!(
        "---\nopennote-id: &shared '{}'\nother: *shared\n---\nbody",
        note.note_id
    );
    for source in [foreign, shared] {
        assert_eq!(
            vault
                .create_copy(&note.note_id, "copy.md", &source, &op())
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
        assert!(!dir.path().join("copy.md").exists());
    }
    assert_eq!(vault.read(&note.note_id).unwrap(), note);
}

#[test]
fn interrupted_copy_resumes_its_original_identity_without_a_second_file() {
    for failpoint in [
        onote_vault_storage::Failpoint::AfterPrepared,
        onote_vault_storage::Failpoint::AfterReplace,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = VaultStorage::create(dir.path(), "copy").unwrap();
        let note = vault.create_note("original.md", "body", &op()).unwrap();
        let operation = op();
        vault.push_failpoint(failpoint);
        vault
            .create_copy(&note.note_id, "copy.md", &note.source, &operation)
            .unwrap_err();
        drop(vault);
        let mut vault = VaultStorage::open(dir.path()).unwrap();
        let copy = vault
            .create_copy(&note.note_id, "copy.md", &note.source, &operation)
            .unwrap();
        assert_ne!(copy.note_id, note.note_id);
        assert_eq!(
            vault
                .create_copy(&note.note_id, "copy.md", &note.source, &operation)
                .unwrap(),
            copy
        );
        assert_eq!(vault.list().unwrap().len(), 2);
        assert_eq!(vault.read(&note.note_id).unwrap(), note);
    }
}
