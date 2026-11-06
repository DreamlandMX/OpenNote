//! F19: attachment bytes are written verified, never overwrite, and retry safely.

use onote_vault_storage::{AttachmentWrite, VaultStorage};
use sha2::{Digest, Sha256};

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn vault() -> (tempfile::TempDir, VaultStorage) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let storage = VaultStorage::create(&root, "Attachments").unwrap();
    (directory, storage)
}

#[test]
fn stores_verified_bytes_and_leaves_no_staging_file() {
    let (directory, mut storage) = vault();
    let bytes = b"\x89PNG\r\n\x1a\nimage";
    let (path, write) = storage
        .store_attachment("assets/pic.png", bytes, &sha(bytes), &op())
        .unwrap();
    assert_eq!(path, "assets/pic.png");
    assert_eq!(write, AttachmentWrite::Stored);
    let root = directory.path().join("vault");
    assert_eq!(std::fs::read(root.join("assets/pic.png")).unwrap(), bytes);
    let leftovers: Vec<_> = std::fs::read_dir(root.join("assets"))
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("opennote-partial")
        })
        .collect();
    assert!(leftovers.is_empty());
    let listed = storage.list_attachments("assets", 1 << 20).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].sha256, sha(bytes));
}

#[test]
fn retry_with_same_bytes_is_already_present_and_different_bytes_never_overwrite() {
    let (directory, mut storage) = vault();
    let first = b"first";
    let id = op();
    storage
        .store_attachment("a.bin", first, &sha(first), &id)
        .unwrap();
    let (_, again) = storage
        .store_attachment("a.bin", first, &sha(first), &id)
        .unwrap();
    assert_eq!(again, AttachmentWrite::AlreadyPresent);

    let other = b"other bytes";
    let error = storage
        .store_attachment("a.bin", other, &sha(other), &op())
        .unwrap_err();
    assert_eq!(error.message_key, "error.attachmentPathOccupied");
    assert_eq!(
        std::fs::read(directory.path().join("vault/a.bin")).unwrap(),
        first,
        "existing bytes are never replaced"
    );
}

#[test]
fn mismatched_hash_is_rejected_before_anything_is_written() {
    let (directory, mut storage) = vault();
    let error = storage
        .store_attachment("x.pdf", b"bytes", &sha(b"different"), &op())
        .unwrap_err();
    assert_eq!(error.message_key, "error.attachmentSourceChanged");
    assert!(!directory.path().join("vault/x.pdf").exists());
}

#[test]
fn unsafe_or_note_paths_are_refused() {
    let (_directory, mut storage) = vault();
    let bytes = b"x";
    for path in [
        "../escape.png",
        "/abs.png",
        "C:/abs.png",
        ".opennote/evil.png",
        "notes/fake.md",
        "CON.png",
        "a/b:ads.png",
    ] {
        assert!(
            storage
                .store_attachment(path, bytes, &sha(bytes), &op())
                .is_err(),
            "{path} must be refused"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_folder_is_not_followed() {
    let (directory, mut storage) = vault();
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, directory.path().join("vault/link")).unwrap();
    let bytes = b"x";
    let error = storage
        .store_attachment("link/pic.png", bytes, &sha(bytes), &op())
        .unwrap_err();
    assert_eq!(error.message_key, "error.attachmentPathReparse");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn listing_skips_notes_and_hidden_files() {
    let (directory, storage) = vault();
    let root = directory.path().join("vault/assets");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("keep.png"), b"img").unwrap();
    std::fs::write(root.join("note.md"), b"# n").unwrap();
    std::fs::write(root.join(".hidden"), b"h").unwrap();
    let listed = storage.list_attachments("assets/", 1 << 20).unwrap();
    let names: Vec<_> = listed
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect();
    assert_eq!(names, vec!["assets/keep.png"]);
    assert!(storage.list_attachments("../up", 10).is_err());
}

#[test]
fn a_reader_returns_size_hash_head_and_bounded_chunks() {
    let (_directory, mut storage) = vault();
    let bytes: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
    storage
        .store_attachment("assets/data.bin", &bytes, &sha(&bytes), &op())
        .unwrap();
    let reader = storage.attachment_reader();
    let probe = reader.probe("assets/data.bin").unwrap();
    assert_eq!(probe.file.size, 10_000);
    assert_eq!(probe.file.sha256, sha(&bytes));
    assert_eq!(probe.head, bytes[..4096]);
    assert_eq!(
        reader.read_chunk("assets/data.bin", 9_000, 5_000).unwrap(),
        bytes[9_000..]
    );
    assert!(reader
        .read_chunk("assets/data.bin", 10_000, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn a_reader_refuses_notes_internal_files_and_paths_outside_the_vault() {
    let (directory, storage) = vault();
    let root = directory.path().join("vault");
    std::fs::write(directory.path().join("outside.png"), b"secret").unwrap();
    std::fs::write(root.join("note.md"), b"# note").unwrap();
    let reader = storage.attachment_reader();
    for path in [
        "note.md",
        ".opennote/vault.json",
        "../outside.png",
        "C:/x.png",
    ] {
        assert!(reader.probe(path).is_err(), "{path} must be refused");
        assert!(
            reader.read_chunk(path, 0, 16).is_err(),
            "{path} must be refused"
        );
    }
    let missing = reader.probe("assets/none.png").unwrap_err();
    assert_eq!(missing.code, onote_domain::ErrorCode::ObjectNotFound);
}

#[cfg(windows)]
#[test]
fn a_reader_refuses_a_file_behind_a_junction() {
    let (directory, storage) = vault();
    let root = directory.path().join("vault");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("pic.png"), b"secret").unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("linked"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(storage.attachment_reader().probe("linked/pic.png").is_err());
}
