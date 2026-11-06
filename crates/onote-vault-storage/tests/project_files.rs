//! F45 LaTeX project files: bounded listing that refuses instead of skipping,
//! source saves that never overwrite a newer edit or write invalid text, and a
//! project record replaced atomically against the hash it was read with.

use onote_domain::ErrorCode;
use onote_vault_storage::{AttachmentWrite, VaultStorage};
use sha2::{Digest, Sha256};

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn vault() -> (tempfile::TempDir, std::path::PathBuf, VaultStorage) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let storage = VaultStorage::create(&root, "Tex").unwrap();
    (directory, root, storage)
}
fn write(root: &std::path::Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn listing_covers_nested_files_with_hashes_and_skips_hidden_folders() {
    let (_d, root, storage) = vault();
    write(&root, "thesis/main.tex", b"\\documentclass{article}");
    write(&root, "thesis/chapters/one.tex", b"one");
    write(&root, "thesis/figures/a.png", &[0x89, 0x50, 0x4e, 0x47]);
    write(&root, "thesis/.latexmkrc", b"$pdf_mode = 1;");
    write(&root, "thesis/.git/config", b"[core]");
    write(&root, "thesis/.main.tex.1.opennote-partial", b"half");
    write(&root, "other/elsewhere.tex", b"not in project");

    let files = storage.list_project_files("thesis").unwrap();
    let paths: Vec<_> = files
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "thesis/.latexmkrc",
            "thesis/chapters/one.tex",
            "thesis/figures/a.png",
            "thesis/main.tex",
        ]
    );
    let main = files
        .iter()
        .find(|file| file.relative_path == "thesis/main.tex")
        .unwrap();
    assert_eq!(main.sha256, sha(b"\\documentclass{article}"));
    assert_eq!(main.size, 23);
}

#[test]
fn listing_refuses_unsafe_or_missing_folders() {
    let (_d, _root, storage) = vault();
    for folder in ["../outside", "C:/abs", ".opennote", "a/../b"] {
        assert!(storage.list_project_files(folder).is_err(), "{folder}");
    }
    assert_eq!(
        storage.list_project_files("nope").unwrap_err().code,
        ErrorCode::ObjectNotFound
    );
}

#[test]
fn listing_refuses_an_oversized_file_instead_of_dropping_it() {
    let (_d, root, storage) = vault();
    write(&root, "big/main.tex", b"x");
    let file = std::fs::File::create(root.join("big/huge.pdf")).unwrap();
    file.set_len(onote_vault_storage::MAX_PROJECT_FILE_BYTES + 1)
        .unwrap();
    let error = storage.list_project_files("big").unwrap_err();
    assert_eq!(error.message_key, "error.projectFileTooLarge");
}

#[cfg(windows)]
#[test]
fn listing_refuses_a_junction_inside_the_project() {
    let (_d, root, storage) = vault();
    write(&root, "p/main.tex", b"x");
    let outside = root.parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.tex"), b"secret").unwrap();
    // `cmd` would read a forward slash in the path as a switch.
    let link = root.join("p").join("linked");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(status.status.success(), "mklink failed");
    let error = storage.list_project_files("p").unwrap_err();
    std::fs::remove_dir(&link).unwrap();
    assert_eq!(error.message_key, "error.projectReparse");
    assert_eq!(
        std::fs::read(outside.join("secret.tex")).unwrap(),
        b"secret"
    );
}

#[test]
fn reading_returns_exact_bytes_and_refuses_traversal() {
    let (_d, root, storage) = vault();
    write(&root, "p/main.tex", "京都 \\LaTeX\r\n".as_bytes());
    let file = storage.read_project_file("p/main.tex").unwrap();
    assert_eq!(file.bytes, "京都 \\LaTeX\r\n".as_bytes());
    assert_eq!(file.sha256, sha("京都 \\LaTeX\r\n".as_bytes()));
    assert!(storage.read_project_file("../x.tex").is_err());
    assert!(storage.read_project_file(".opennote/vault.json").is_err());
    assert_eq!(
        storage.read_project_file("p/none.tex").unwrap_err().code,
        ErrorCode::ObjectNotFound
    );
}

#[test]
fn source_save_is_compare_and_swap_and_idempotent() {
    let (_d, root, mut storage) = vault();
    write(&root, "p/main.tex", b"old");
    let loaded = sha(b"old");
    let (_, hash, wrote) = storage
        .save_project_source("p/main.tex", b"new", Some(&loaded), &op())
        .unwrap();
    assert_eq!(
        (hash.as_str(), wrote),
        (sha(b"new").as_str(), AttachmentWrite::Stored)
    );
    assert_eq!(std::fs::read(root.join("p/main.tex")).unwrap(), b"new");

    // A lost reply retried with the same bytes writes nothing.
    let (_, _, again) = storage
        .save_project_source("p/main.tex", b"new", Some(&loaded), &op())
        .unwrap();
    assert_eq!(again, AttachmentWrite::AlreadyPresent);

    // An external edit is never overwritten by the stale buffer.
    std::fs::write(root.join("p/main.tex"), b"theirs").unwrap();
    let error = storage
        .save_project_source("p/main.tex", b"mine", Some(&sha(b"new")), &op())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::VersionConflict);
    assert_eq!(std::fs::read(root.join("p/main.tex")).unwrap(), b"theirs");
}

#[test]
fn source_save_refuses_non_sources_bad_encoding_and_markdown() {
    let (_d, root, mut storage) = vault();
    write(&root, "p/main.tex", b"ok");
    let loaded = sha(b"ok");
    let error = storage
        .save_project_source("p/main.tex", &[0xff, 0xfe], Some(&loaded), &op())
        .unwrap_err();
    assert_eq!(error.message_key, "error.projectEncoding");
    assert_eq!(std::fs::read(root.join("p/main.tex")).unwrap(), b"ok");
    for path in ["p/a.png", "p/notes.md", "p/run.bat", "../a.tex"] {
        assert!(
            storage
                .save_project_source(path, b"x", None, &op())
                .is_err(),
            "{path}"
        );
    }
    // A brand-new source file is created only where nothing exists yet.
    let (_, _, created) = storage
        .save_project_source("p/refs.bib", b"@book{a,}", None, &op())
        .unwrap();
    assert_eq!(created, AttachmentWrite::Stored);
}

#[test]
fn project_record_is_created_once_and_replaced_only_against_its_hash() {
    let (_d, root, mut storage) = vault();
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(storage.read_project_record(&id).unwrap(), None);

    let first = storage
        .write_project_record(&id, b"{\"v\":1}", None, &op())
        .unwrap();
    assert_eq!(first, sha(b"{\"v\":1}"));
    let path = root.join(".opennote/latex").join(&id).join("project.json");
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"v\":1}");

    // Creating again over an existing record is a conflict.
    let error = storage
        .write_project_record(&id, b"{\"v\":9}", None, &op())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::VersionConflict);

    let second = storage
        .write_project_record(&id, b"{\"v\":2}", Some(&first), &op())
        .unwrap();
    // A stale hash cannot replace the newer record.
    let error = storage
        .write_project_record(&id, b"{\"v\":3}", Some(&first), &op())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::VersionConflict);
    let read = storage.read_project_record(&id).unwrap().unwrap();
    assert_eq!(
        (read.bytes.as_slice(), read.sha256),
        (&b"{\"v\":2}"[..], second)
    );

    for bad in ["not-a-uuid", "../x", &id.to_uppercase()] {
        assert!(storage.read_project_record(bad).is_err(), "{bad}");
    }
}

#[test]
fn records_are_listed_and_removed_without_touching_project_files() {
    let (_d, root, mut storage) = vault();
    write(&root, "p/main.tex", b"keep me");
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let first_sha = storage
        .write_project_record(&first, b"{\"a\":1}", None, &op())
        .unwrap();
    storage
        .write_project_record(&second, b"{\"b\":2}", None, &op())
        .unwrap();
    // Unrelated directories are not projects.
    std::fs::create_dir_all(root.join(".opennote/latex/not-a-project")).unwrap();
    std::fs::create_dir_all(
        root.join(".opennote/latex")
            .join(uuid::Uuid::new_v4().to_string()),
    )
    .unwrap();

    let mut expected = vec![first.clone(), second.clone()];
    expected.sort();
    let ids: Vec<_> = storage
        .list_project_records()
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, expected);

    // A stale hash cannot remove a record that changed.
    assert_eq!(
        storage
            .remove_project_record(&first, &sha(b"other"), &op())
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    storage
        .remove_project_record(&first, &first_sha, &op())
        .unwrap();
    // A retry after success is still success.
    storage
        .remove_project_record(&first, &first_sha, &op())
        .unwrap();
    assert!(!root.join(".opennote/latex").join(&first).exists());
    assert_eq!(storage.list_project_records().unwrap().len(), 1);
    assert_eq!(std::fs::read(root.join("p/main.tex")).unwrap(), b"keep me");
}
