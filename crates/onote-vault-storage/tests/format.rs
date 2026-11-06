use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

use onote_domain::{ErrorCode, ServiceError};
use onote_vault_storage::{Failpoint, VaultStorage};

// ----------------------------------------------------------------------------
// format preservation at the save boundary (BOM presence + LF/CRLF style are
// taken from the authoritative on-disk bytes, never from the caller text)
// ----------------------------------------------------------------------------

fn op_id() -> String {
    Uuid::new_v4().to_string()
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Legacy BOM+CRLF note with unknown YAML (comment, title, tags — no
/// embedded identity): an edit supplied as CodeMirror-normalized LF text
/// without a BOM must land on disk with the original BOM and CRLF style,
/// the identity line inserted in CRLF style, and all unknown YAML bytes
/// verbatim. SaveResult hash equals SHA-256 of the actual on-disk bytes.
#[test]
fn bom_crlf_legacy_preserved_when_editing_with_lf_nobom_caller() {
    let dir = TempDir::new().unwrap();
    let baseline =
        "\u{FEFF}---\r\n# kept comment\r\ntitle: legacy\r\ntags: a, b\r\n---\r\nbody one\r\n";
    fs::write(dir.path().join("legacy.md"), baseline.as_bytes()).unwrap();

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "legacy.md")
        .unwrap();
    let note_id = summary.note_id.clone();
    assert_eq!(summary.local_revision, 0);

    // editor-supplied body edit: normalized LF, no BOM
    let caller = "---\n# kept comment\ntitle: legacy\ntags: a, b\n---\nbody edited\n";
    let saved = storage
        .save(note_id.as_str(), caller, 0, &op_id())
        .unwrap_or_else(|e| panic!("legacy BOM/CRLF save failed {e:?}"));

    let expected = format!(
        "\u{FEFF}---\r\nopennote-id: {note_id}\r\n# kept comment\r\ntitle: legacy\r\ntags: a, b\r\n---\r\nbody edited\r\n"
    );
    let disk = read_bytes(&dir.path().join("legacy.md"));
    assert_eq!(
        String::from_utf8(disk.clone()).unwrap(),
        expected,
        "BOM, CRLF style and unknown YAML must be preserved exactly"
    );
    assert_eq!(
        saved.content_hash,
        sha256_hex(&disk),
        "returned hash must equal SHA-256 of the actual on-disk bytes"
    );
    assert_eq!(saved.local_revision, 1);

    let reopened = VaultStorage::open(dir.path()).unwrap();
    let doc = reopened.read(note_id.as_str()).unwrap();
    assert_eq!(doc.source, expected, "re-read returns the raw disk bytes");
}

// ----------------------------------------------------------------------------
// prepared save retry reconstruction: the same raw request resumes with
// IDENTICAL composed bytes (format inferred only from validated journal
// candidates), never a second revision; all-missing/all-foreign references
// refuse typed and preserve every pending artifact
// ----------------------------------------------------------------------------

fn assert_injected_io_error(label: &str, err: &ServiceError) {
    assert!(
        matches!(
            err.code,
            ErrorCode::DiskFull | ErrorCode::IoDenied | ErrorCode::FileLocked
        ),
        "{label}: expected typed injected IO error, got {:?} ({:?})",
        err.code,
        err.message_key
    );
}

fn open_state(dir: &TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join(".opennote").join("state.sqlite")).unwrap()
}

fn ops_count(dir: &TempDir, relative: &str) -> i64 {
    open_state(dir)
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE note_relative = ?1",
            [relative],
            |r| r.get(0),
        )
        .unwrap()
}

fn note_revision(dir: &TempDir, relative: &str) -> i64 {
    open_state(dir)
        .query_row(
            "SELECT local_revision FROM notes WHERE relative_path = ?1",
            [relative],
            |r| r.get(0),
        )
        .unwrap()
}

fn op_status(dir: &TempDir, op: &str) -> String {
    open_state(dir)
        .query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [op],
            |r| r.get(0),
        )
        .unwrap()
}

const LEGACY_BASELINE: &str =
    "\u{FEFF}---\r\n# kept comment\r\ntitle: legacy\r\ntags: a, b\r\n---\r\nbody one\r\n";
const LEGACY_CALLER_LF: &str = "---\n# kept comment\ntitle: legacy\ntags: a, b\n---\nbody edited\n";

/// The legacy scenario prepared at the AfterPrepared boundary: the same raw
/// request resumes (no reopen, no recover) and reconstructs the exact
/// BOM+CRLF/unknown-YAML composed bytes with exactly one revision.
#[test]
fn prepared_retry_after_prepared_reconstructs_exact_bytes_and_one_revision() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("legacy.md"), LEGACY_BASELINE.as_bytes()).unwrap();

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "legacy.md")
        .unwrap();
    let note_id = summary.note_id.clone();
    let revision = summary.local_revision;

    let op = op_id();
    let stage = dir.path().join(format!("legacy.md.onote-stage-{op}"));
    let backup = dir.path().join(format!("legacy.md.onote-old-{op}"));

    storage.push_failpoint(Failpoint::AfterPrepared);
    let err = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_err();
    assert_injected_io_error("AfterPrepared", &err);
    assert!(stage.exists() && backup.exists());
    assert_eq!(
        fs::read(&backup).unwrap(),
        LEGACY_BASELINE.as_bytes(),
        "the prepared backup holds the original BOM/CRLF bytes"
    );

    let retried = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_or_else(|e| panic!("same-request retry failed {e:?}"));

    let expected = format!(
        "\u{FEFF}---\r\nopennote-id: {note_id}\r\n# kept comment\r\ntitle: legacy\r\ntags: a, b\r\n---\r\nbody edited\r\n"
    );
    let disk = read_bytes(&dir.path().join("legacy.md"));
    assert_eq!(
        String::from_utf8(disk.clone()).unwrap(),
        expected,
        "retry must reconstruct the identical BOM/CRLF/unknown-YAML bytes"
    );
    assert_eq!(
        retried.content_hash,
        sha256_hex(&disk),
        "retry hash must match the actual on-disk bytes"
    );
    assert_eq!(retried.local_revision, revision + 1);
    assert_eq!(note_revision(&dir, "legacy.md"), 1);
    assert_eq!(ops_count(&dir, "legacy.md"), 1, "exactly one save revision");
    assert!(!stage.exists() && !backup.exists());
}

/// The same raw request retried AFTER REPLACE (target already carries the
/// composed bytes, DB baseline untouched) through a fresh reopen with NO
/// recover() call: reconstruction must be byte-exact and finalize exactly
/// one revision.
#[test]
fn prepared_retry_after_replace_reopen_reconstructs_exact_bytes_one_revision() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("legacy.md"), LEGACY_BASELINE.as_bytes()).unwrap();

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "legacy.md")
        .unwrap();
    let note_id = summary.note_id.clone();
    let revision = summary.local_revision;

    let op = op_id();
    let stage = dir.path().join(format!("legacy.md.onote-stage-{op}"));
    let backup = dir.path().join(format!("legacy.md.onote-old-{op}"));

    storage.push_failpoint(Failpoint::AfterReplace);
    let err = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_err();
    assert_injected_io_error("AfterReplace", &err);
    assert!(backup.exists(), "crash keeps the prepared backup");
    assert!(!stage.exists(), "the stage was consumed by the rename");

    drop(storage);
    let mut reopened = VaultStorage::open(dir.path()).unwrap();
    let retried = reopened
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_or_else(|e| panic!("reopen retry failed {e:?}"));

    let expected = format!(
        "\u{FEFF}---\r\nopennote-id: {note_id}\r\n# kept comment\r\ntitle: legacy\r\ntags: a, b\r\n---\r\nbody edited\r\n"
    );
    let disk = read_bytes(&dir.path().join("legacy.md"));
    assert_eq!(
        String::from_utf8(disk.clone()).unwrap(),
        expected,
        "reopen retry must reconstruct the identical composed bytes"
    );
    assert_eq!(
        retried.content_hash,
        sha256_hex(&disk),
        "reopen retry hash must match the actual on-disk bytes"
    );
    assert_eq!(retried.local_revision, revision + 1);
    assert_eq!(note_revision(&dir, "legacy.md"), 1);
    assert_eq!(ops_count(&dir, "legacy.md"), 1, "exactly one save revision");
    assert!(!backup.exists(), "finalized slot cleans the backup");
}

/// A committed save retried after the target file was deleted still returns
/// the original metadata before any reconstruction and writes nothing.
#[test]
fn committed_replay_after_target_file_deleted_returns_original_metadata() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage
        .create_note("gone-format.md", "# v0", &op_id())
        .unwrap();

    let op = op_id();
    let saved = storage
        .save(note.note_id.as_str(), "# v1", note.local_revision, &op)
        .unwrap();

    fs::remove_file(dir.path().join("gone-format.md")).unwrap();

    let retried1 = storage
        .save(note.note_id.as_str(), "# v1", note.local_revision, &op)
        .unwrap_or_else(|e| panic!("committed replay after deletion failed {e:?}"));
    assert_eq!(retried1, saved);

    drop(storage);
    let mut reopened = VaultStorage::open(dir.path()).unwrap();
    let retried2 = reopened
        .save(note.note_id.as_str(), "# v1", note.local_revision, &op)
        .unwrap_or_else(|e| panic!("reopened committed replay failed {e:?}"));
    assert_eq!(retried2, saved);
    assert!(
        !dir.path().join("gone-format.md").exists(),
        "a committed replay must not recreate any file"
    );
    assert_eq!(ops_count(&dir, "gone-format.md"), 2);
    assert_eq!(note_revision(&dir, "gone-format.md"), 1);
}

/// Every candidate gone or overwritten with foreign bytes: the same request
/// refuses typed, keeps the pending journal and never rewrites any file.
#[test]
fn retry_with_all_foreign_candidates_refuses_and_preserves_everything() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("legacy.md"), LEGACY_BASELINE.as_bytes()).unwrap();

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "legacy.md")
        .unwrap();
    let note_id = summary.note_id.clone();
    let revision = summary.local_revision;

    let op = op_id();
    let target = dir.path().join("legacy.md");
    let stage = dir.path().join(format!("legacy.md.onote-stage-{op}"));
    let backup = dir.path().join(format!("legacy.md.onote-old-{op}"));

    storage.push_failpoint(Failpoint::AfterPrepared);
    let err = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_err();
    assert_injected_io_error("AfterPrepared", &err);

    // a foreign writer replaces every candidate after the crash
    fs::write(&target, b"foreign-target").unwrap();
    fs::write(&stage, b"foreign-stage").unwrap();
    fs::write(&backup, b"foreign-backup").unwrap();

    let err = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::VersionConflict,
        "no validated reference must refuse typed, got {:?} {:?}",
        err.code,
        err.message_key
    );
    assert_eq!(
        fs::read(&target).unwrap(),
        b"foreign-target",
        "refusal must never rewrite the target"
    );
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"foreign-stage",
        "refusal must never delete or rewrite the staged bytes"
    );
    assert_eq!(
        fs::read(&backup).unwrap(),
        b"foreign-backup",
        "refusal must never delete or rewrite the backup"
    );
    assert_eq!(op_status(&dir, &op), "prepared", "the slot stays pending");
    assert_eq!(note_revision(&dir, "legacy.md"), 0, "no revision change");
    assert_eq!(ops_count(&dir, "legacy.md"), 1, "only the pending save row");
}

/// The pending slot stays resumable after the foreign overwrites are
/// reverted: restoring the baseline outside the vault file makes the same
/// request complete with the exact artifact and one revision.
#[test]
fn retry_after_foreign_refusal_is_preserved_and_resumable() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("legacy.md"), LEGACY_BASELINE.as_bytes()).unwrap();

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "legacy.md")
        .unwrap();
    let note_id = summary.note_id.clone();
    let revision = summary.local_revision;

    let op = op_id();
    let stage = dir.path().join(format!("legacy.md.onote-stage-{op}"));
    let backup = dir.path().join(format!("legacy.md.onote-old-{op}"));
    let target = dir.path().join("legacy.md");

    storage.push_failpoint(Failpoint::AfterPrepared);
    let err = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_err();
    assert_injected_io_error("AfterPrepared", &err);

    // foreign overwrite, then a full externally induced revert
    fs::write(&target, b"foreign-target").unwrap();
    fs::write(&stage, b"foreign-stage").unwrap();
    fs::write(&backup, b"foreign-backup").unwrap();
    let refusal = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_err();
    assert_eq!(refusal.code, ErrorCode::VersionConflict);

    fs::write(&target, LEGACY_BASELINE.as_bytes()).unwrap();
    fs::remove_file(&stage).unwrap();
    fs::remove_file(&backup).unwrap();

    let retried = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_or_else(|e| panic!("retry after revert failed {e:?}"));
    let expected = format!(
        "\u{FEFF}---\r\nopennote-id: {note_id}\r\n# kept comment\r\ntitle: legacy\r\ntags: a, b\r\n---\r\nbody edited\r\n"
    );
    let disk = read_bytes(&target);
    assert_eq!(String::from_utf8(disk.clone()).unwrap(), expected);
    assert_eq!(retried.content_hash, sha256_hex(&disk));
    assert_eq!(note_revision(&dir, "legacy.md"), 1);
    assert_eq!(ops_count(&dir, "legacy.md"), 1);
    assert!(!stage.exists() && !backup.exists());
}

/// An existing valid-identity BOM+mixed LF/CRLF file saves byte-identically
/// when the caller text matches disk exactly, while a normalized (BOM-less,
/// LF-only) edited caller is refused typed with bytes and revision untouched.
#[test]
fn mixed_legacy_identical_caller_passes_unchanged_and_edited_caller_refuses() {
    let dir = TempDir::new().unwrap();
    let id = Uuid::new_v4();
    let baseline =
        format!("\u{FEFF}---\r\nopennote-id: {id}\n# kept comment\n---\r\nbody\r\nline\n");
    fs::write(dir.path().join("mixed.md"), baseline.as_bytes()).unwrap();

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "mixed.md")
        .unwrap();
    assert_eq!(
        summary.note_id,
        id.to_string(),
        "embedded id extracted from mixed-EOL frontmatter"
    );

    let identical = storage
        .save(
            &summary.note_id,
            &baseline,
            summary.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("identical caller must save {e:?}"));
    let disk = read_bytes(&dir.path().join("mixed.md"));
    assert_eq!(
        String::from_utf8(disk.clone()).unwrap(),
        baseline,
        "mixed LF/CRLF bytes with BOM preserved verbatim"
    );
    assert_eq!(identical.content_hash, sha256_hex(&disk));
    assert_eq!(identical.local_revision, summary.local_revision + 1);

    let caller = format!("---\nopennote-id: {id}\n# kept comment\n---\nbody two\n");
    let err = storage
        .save(
            &summary.note_id,
            &caller,
            identical.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidInput,
        "{:?} {:?}",
        err.code,
        err.message_key
    );
    assert_eq!(err.message_key, "error.lineEndingsUnsupported");
    assert_eq!(
        String::from_utf8(read_bytes(&dir.path().join("mixed.md"))).unwrap(),
        baseline,
        "refusal keeps the original bytes"
    );
    assert_eq!(
        note_revision(&dir, "mixed.md"),
        1,
        "refusal leaves the revision unchanged"
    );
}

/// AfterReplace pending save for a BOM/CRLF file whose old backup was
/// externally removed afterwards: the same raw request retries using the
/// target that already matches the new hash and finalizes exactly one
/// revision (no foreign bytes, no manual DB edits).
#[test]
fn prepared_retry_after_replace_survives_missing_old_backup() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("legacy.md"), LEGACY_BASELINE.as_bytes()).unwrap();

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "legacy.md")
        .unwrap();
    let note_id = summary.note_id.clone();
    let revision = summary.local_revision;

    let op = op_id();
    let backup = dir.path().join(format!("legacy.md.onote-old-{op}"));

    storage.push_failpoint(Failpoint::AfterReplace);
    let err = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_err();
    assert_injected_io_error("AfterReplace", &err);
    assert!(backup.exists(), "the backup exists right after the crash");

    fs::remove_file(&backup).unwrap();

    let retried = storage
        .save(note_id.as_str(), LEGACY_CALLER_LF, revision, &op)
        .unwrap_or_else(|e| panic!("retry without the old backup failed {e:?}"));

    let expected = format!(
        "\u{FEFF}---\r\nopennote-id: {note_id}\r\n# kept comment\r\ntitle: legacy\r\ntags: a, b\r\n---\r\nbody edited\r\n"
    );
    let disk = read_bytes(&dir.path().join("legacy.md"));
    assert_eq!(
        String::from_utf8(disk.clone()).unwrap(),
        expected,
        "retry reconstructs the exact composed bytes from the matching target"
    );
    assert_eq!(retried.content_hash, sha256_hex(&disk));
    assert_eq!(retried.local_revision, revision + 1);
    assert_eq!(note_revision(&dir, "legacy.md"), 1);
    assert_eq!(ops_count(&dir, "legacy.md"), 1);
    assert!(!backup.exists());
}
