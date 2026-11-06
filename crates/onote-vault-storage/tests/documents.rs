//! Structured vault documents (`.canvas`): compare-and-swap saves never lose a
//! newer edit, retries are idempotent, and the vault format is unchanged.

use onote_domain::ErrorCode;
use onote_vault_storage::{AttachmentWrite, VaultStorage};
use sha2::{Digest, Sha256};

fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn vault() -> (tempfile::TempDir, std::path::PathBuf, VaultStorage) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let storage = VaultStorage::create(&root, "Docs").unwrap();
    (directory, root, storage)
}

#[test]
fn create_reopen_and_save_round_trip() {
    let (_d, root, mut storage) = vault();
    let first = r#"{"nodes":[],"edges":[]}"#;
    let (path, hash, write) = storage
        .save_document("boards/Plan.canvas", ".canvas", first, None, &op())
        .unwrap();
    assert_eq!(
        (path.as_str(), write),
        ("boards/Plan.canvas", AttachmentWrite::Stored)
    );
    assert_eq!(hash, sha(first));

    // Reopen through a fresh storage handle, as after a restart.
    drop(storage);
    let mut storage = VaultStorage::open(&root).unwrap();
    let read = storage
        .read_document("boards/Plan.canvas", ".canvas")
        .unwrap();
    assert_eq!(read.source, first);

    let second = r#"{"nodes":[{"id":"a"}],"edges":[]}"#;
    let (_, hash2, write2) = storage
        .save_document(
            "boards/Plan.canvas",
            ".canvas",
            second,
            Some(&read.sha256),
            &op(),
        )
        .unwrap();
    assert_eq!(write2, AttachmentWrite::Stored);
    assert_eq!(
        std::fs::read_to_string(root.join("boards/Plan.canvas")).unwrap(),
        second
    );
    let listed = storage.list_documents(".canvas").unwrap();
    assert_eq!(listed.len(), 1);

    // Retrying the same save (lost reply) succeeds without writing again.
    let (_, hash3, write3) = storage
        .save_document(
            "boards/Plan.canvas",
            ".canvas",
            second,
            Some(&read.sha256),
            &op(),
        )
        .unwrap();
    assert_eq!((hash3, write3), (hash2, AttachmentWrite::AlreadyPresent));
}

#[test]
fn an_external_edit_is_a_conflict_and_is_not_overwritten() {
    let (_d, root, mut storage) = vault();
    let (_, loaded, _) = storage
        .save_document("a.canvas", ".canvas", "{}", None, &op())
        .unwrap();
    std::fs::write(root.join("a.canvas"), "{\"external\":true}").unwrap();
    let error = storage
        .save_document(
            "a.canvas",
            ".canvas",
            "{\"mine\":true}",
            Some(&loaded),
            &op(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::VersionConflict);
    assert_eq!(
        std::fs::read_to_string(root.join("a.canvas")).unwrap(),
        "{\"external\":true}"
    );
}

#[test]
fn creating_over_an_existing_or_saving_a_deleted_file_is_a_conflict() {
    let (_d, root, mut storage) = vault();
    std::fs::write(root.join("b.canvas"), "{\"theirs\":1}").unwrap();
    let error = storage
        .save_document("b.canvas", ".canvas", "{}", None, &op())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::VersionConflict);
    assert_eq!(
        std::fs::read_to_string(root.join("b.canvas")).unwrap(),
        "{\"theirs\":1}"
    );

    let theirs = sha("{\"theirs\":1}");
    std::fs::remove_file(root.join("b.canvas")).unwrap();
    let error = storage
        .save_document("b.canvas", ".canvas", "{}", Some(&theirs), &op())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::VersionConflict);
    assert!(
        !root.join("b.canvas").exists(),
        "a deleted file is not silently recreated"
    );
}

#[test]
fn wrong_suffix_unsafe_paths_and_bad_encoding_are_refused() {
    let (_d, root, mut storage) = vault();
    for path in ["a.md", "a.txt", "../a.canvas", ".opennote/a.canvas"] {
        assert!(
            storage
                .save_document(path, ".canvas", "{}", None, &op())
                .is_err(),
            "{path}"
        );
    }
    std::fs::write(root.join("bad.canvas"), [0xff, 0xfe]).unwrap();
    assert_eq!(
        storage
            .read_document("bad.canvas", ".canvas")
            .unwrap_err()
            .message_key,
        "error.documentEncoding"
    );
    assert_eq!(
        storage
            .read_document("none.canvas", ".canvas")
            .unwrap_err()
            .code,
        ErrorCode::ObjectNotFound
    );
}

#[test]
fn listing_skips_hidden_folders_and_staging_files() {
    let (_d, root, storage) = vault();
    std::fs::create_dir_all(root.join("x")).unwrap();
    std::fs::write(root.join("x/one.canvas"), "{}").unwrap();
    std::fs::write(root.join(".opennote/hidden.canvas"), "{}").unwrap();
    std::fs::write(root.join("x/.one.canvas.1.opennote-partial"), "{}").unwrap();
    let names: Vec<_> = storage
        .list_documents(".canvas")
        .unwrap()
        .into_iter()
        .map(|file| file.relative_path)
        .collect();
    assert_eq!(names, vec!["x/one.canvas".to_owned()]);
}
