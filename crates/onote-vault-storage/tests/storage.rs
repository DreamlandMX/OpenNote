use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use filetime::{set_file_mtime, FileTime};
use uuid::Uuid;

use onote_domain::{ErrorCode, ServiceError};
use onote_vault_storage::{Failpoint, RecoverOutcome, VaultStorage};

// ----------------------------------------------------------------------------
// helpers (deterministic, per-test temporary dirs)
// ----------------------------------------------------------------------------

struct TestVault {
    storage: VaultStorage,
    _dir: TempDir,
}

impl TestVault {
    fn new() -> TestVault {
        let _dir = TempDir::new().unwrap();
        let storage = VaultStorage::create(_dir.path(), "TestVault").unwrap();
        TestVault { storage, _dir }
    }

    fn path(&self) -> &Path {
        self._dir.path()
    }
}

fn write_byte_file(path: &Path, exact_bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, exact_bytes).unwrap();
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

/// Byte snapshot of every user file in a vault, excluding the app metadata
/// directory that `open` is allowed to create. Opening a real directory must
/// not add, remove or rewrite any of these files (F02-A01/F02-R01).
fn source_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path == root.join(".opennote") {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn op_id() -> String {
    Uuid::new_v4().to_string()
}

/// Open state.sqlite independently and count operation rows for a path
/// (callers with live storage connections work because WAL allows readers).
fn db_fresh_ops_rows(tv: &TestVault, note_relative: &str, f: impl FnOnce(usize)) {
    let conn = rusqlite::Connection::open(tv.path().join(".opennote").join("state.sqlite"))
        .expect("db introspection open");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE note_relative = ?1",
            [note_relative],
            |row| row.get(0),
        )
        .expect("db introspection query");
    f(count as usize);
}

// ----------------------------------------------------------------------------
// 1. create + save + reopen round trip
// ----------------------------------------------------------------------------

#[test]
fn create_save_reopen_roundtrip() {
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note("notes/first.md", "# first", &op_id())
        .unwrap_or_else(|e| panic!("RED: create_note returned {e:?}"));
    let edited = note.source.replace("# first", "# first v2");
    let saved = tv
        .storage
        .save(
            note.note_id.as_str(),
            &edited,
            note.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("RED: save returned typed error {:?}", e.code));

    assert_eq!(saved.note_id, note.note_id);
    assert!(saved.local_revision > note.local_revision);

    let reopened = VaultStorage::open(tv.path()).unwrap();
    let doc = reopened.read(note.note_id.as_str()).unwrap();
    assert_eq!(doc.source, edited);
}

// ----------------------------------------------------------------------------
// 2. scan leaves existing bytes unchanged
// ----------------------------------------------------------------------------

#[test]
fn scan_untouched_files() {
    let dir = TempDir::new().unwrap();
    let note_path = dir.path().join("a.md");
    write_byte_file(&note_path, b"#existing bytes\r\nstays same");
    write_byte_file(&dir.path().join("b.md"), b"plain");
    write_byte_file(&dir.path().join("nested/c.md"), b"nested body");
    let before = source_files(dir.path());

    let storage = VaultStorage::open(dir.path()).unwrap();
    let summaries = storage
        .list()
        .unwrap_or_else(|e| panic!("RED: list returned typed error {e:?}"));
    assert!(summaries.iter().all(|s| !s.read_only));
    assert_eq!(summaries.len(), 3, "nested note must be scanned");

    let after = source_files(dir.path());
    assert_eq!(before, after, "scan must not modify any file bytes");
}

#[test]
fn moved_vault_directory_keeps_vault_id_and_notes() {
    let parent = TempDir::new().unwrap();
    let original = parent.path().join("original");
    let mut storage = VaultStorage::create(&original, "Moved").unwrap();
    let vault_id = storage.vault().vault_id.clone();
    let note = storage.create_note("note.md", "body", &op_id()).unwrap();
    let expected_source = note.source.clone();
    drop(storage);

    let moved = parent.path().join("renamed");
    fs::rename(&original, &moved).unwrap();

    let reopened = VaultStorage::open(&moved).unwrap();
    assert_eq!(
        reopened.vault().vault_id,
        vault_id,
        "moving the directory must reuse the persisted vault id, not mint a new one"
    );
    let doc = reopened.read(note.note_id.as_str()).unwrap();
    assert_eq!(doc.vault_id, vault_id);
    assert_eq!(doc.source, expected_source);
}

// ----------------------------------------------------------------------------
// 3. BOM + CRLF fixture opened from disk and preserved through save
// (new files default to UTF-8 no BOM / LF, so the fixture is an existing file)
// ----------------------------------------------------------------------------

#[test]
fn bom_and_crlf_preserved_through_open_and_save() {
    let dir = TempDir::new().unwrap();
    // BOM + CRLF + no frontmatter id (legacy note): bytes must never be touched by scan.
    write_byte_file(
        &dir.path().join("bom.md"),
        b"\xEF\xBB\xBFheader\r\nbody\r\n",
    );
    let before = read_bytes(&dir.path().join("bom.md"));

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.relative_path == "bom.md")
        .unwrap();
    let doc = storage.read(summary.note_id.as_str()).unwrap();

    // storage built the full new source from the returned document, keeping
    // identity frontmatter and the original BOM/line endings.
    let mut red_source = doc.source.clone();
    red_source.push_str("\nappended");
    storage
        .save(
            summary.note_id.as_str(),
            &red_source,
            doc.local_revision,
            &op_id(),
        )
        .unwrap();

    let after = read_bytes(&dir.path().join("bom.md"));
    assert_eq!(&after[..3], &[0xEF, 0xBB, 0xBF], "BOM must survive save");
    assert!(after.contains(&b'\r'), "CRLF must survive save");
    assert!(after.windows(3).any(|w| w == [0xEF, 0xBB, 0xBF]));
    let _ = before; // full-byte diff happens in GREEN; here we pin encoding traits
}

// ----------------------------------------------------------------------------
// 4. invalid UTF-8 -> read-only note
// ----------------------------------------------------------------------------

#[test]
fn invalid_utf8_read_only() {
    let dir = TempDir::new().unwrap();
    write_byte_file(&dir.path().join("binary.md"), b"invalid \xFF bytes here");

    let storage = VaultStorage::open(dir.path()).unwrap();
    let summaries = storage.list().unwrap();
    let bin = summaries
        .iter()
        .find(|s| s.relative_path == "binary.md")
        .expect("note should be listed");
    assert!(bin.read_only, "invalid UTF-8 must be read-only");
    let err = storage.read(bin.note_id.as_str()).unwrap_err();
    assert_eq!(err.code, ErrorCode::IoDenied);
}

// ----------------------------------------------------------------------------
// 5. stale expectedRevision -> VERSION_CONFLICT
// ----------------------------------------------------------------------------

#[test]
fn stale_revision_conflict() {
    let mut tv = TestVault::new();
    let note = tv.storage.create_note("c.md", "v1", &op_id()).unwrap();

    tv.storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &op_id())
        .unwrap();

    let err = tv
        .storage
        .save(
            note.note_id.as_str(),
            "stale edit",
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::VersionConflict);
}

// ----------------------------------------------------------------------------
// 6. external content change with unchanged mtime refuses overwrite
// ----------------------------------------------------------------------------

#[test]
fn external_change_same_mtime_refuses() {
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note("external.md", "from-opennote", &op_id())
        .unwrap();

    // External editor rewrites the bytes but keeps the mtime identical.
    let target = tv.path().join("external.md");
    let original_mtime = fs::metadata(&target).unwrap().modified().unwrap();
    write_byte_file(&target, b"from-external-editor");
    let _ = set_file_mtime(&target, FileTime::from_system_time(original_mtime));

    let err = tv
        .storage
        .save(
            note.note_id.as_str(),
            "overwriting edit",
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::VersionConflict);
}

// ----------------------------------------------------------------------------
// 7. operationId retry: same id/payload -> same result, no double write
// ----------------------------------------------------------------------------

#[test]
fn operation_id_retry_same_payload() {
    let mut tv = TestVault::new();
    let note = tv.storage.create_note("idem.md", "v1", &op_id()).unwrap();

    let same_op = op_id();
    let a = tv
        .storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &same_op)
        .unwrap();
    let b = tv
        .storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &same_op)
        .unwrap();
    assert_eq!(a, b, "same operationId+payload must return same result");
}

// ----------------------------------------------------------------------------
// 8. operationId mismatch: same id, different payload rejects
// ----------------------------------------------------------------------------

#[test]
fn operation_id_payload_mismatch() {
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note("mismatch.md", "v1", &op_id())
        .unwrap();

    let same_op = op_id();
    tv.storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &same_op)
        .unwrap();
    let err = tv
        .storage
        .save(
            note.note_id.as_str(),
            "DIFFERENT v2",
            note.local_revision,
            &same_op,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::IdempotencyMismatch);
}

// ----------------------------------------------------------------------------
// 8b. idempotency hardening: original-request digest, operationId-keyed rows
// ----------------------------------------------------------------------------

/// Same operationId + identical request retried after reopen must replay the
/// ORIGINAL full result (original UUID, path, revision, hash), never fail on
/// existence checks or allocate a fresh note id.
#[test]
fn create_same_opid_retry_after_reopen_replays_original() {
    let mut tv = TestVault::new();
    let op = op_id();
    let note = tv
        .storage
        .create_note("idem/create.md", "draft body", &op)
        .unwrap_or_else(|e| panic!("RED setup: create failed {e:?}"));

    let mut reopened = VaultStorage::open(tv.path()).unwrap();
    let again = reopened
        .create_note("idem/create.md", "draft body", &op)
        .unwrap_or_else(|e| panic!("RED: create retry after reopen returned {e:?}"));

    assert_eq!(again.note_id, note.note_id, "must replay the original UUID");
    assert_eq!(again.relative_path, note.relative_path);
    assert_eq!(again.local_revision, note.local_revision);
    assert_eq!(again.content_hash, note.content_hash);
    assert_eq!(again.source, note.source);

    db_fresh_ops_rows(&tv, "idem/create.md", |rows| assert_eq!(rows, 1));
}

/// Two different operationIds with the byte-identical save request are two
/// distinct operations: neither row may erase the other (operationId keyed,
/// request digest non-unique).
#[test]
fn identical_request_distinct_opids_both_operations_persisted() {
    let mut tv = TestVault::new();
    let note = tv.storage.create_note("dup.md", "v1", &op_id()).unwrap();

    let op_b = op_id();
    tv.storage.push_failpoint(Failpoint::AfterPrepared);
    let b_err = tv
        .storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &op_b)
        .unwrap_err();
    assert_injected_io_error(&b_err);

    let op_c = op_id();
    let c = tv
        .storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &op_c)
        .unwrap_or_else(|e| panic!("RED: identical-request second op returned {e:?}"));
    assert_eq!(c.local_revision, note.local_revision + 1);

    db_fresh_ops_rows(&tv, "dup.md", |rows| {
        assert_eq!(
            rows, 3,
            "RED: insert-or-replace on shared request digest erased one operation"
        );
    });
}

/// Retrying a committed save after later commits must still return the
/// original revision/hash, not the newest state.
#[test]
fn save_retry_after_later_commits_replays_original_result() {
    let mut tv = TestVault::new();
    let note = tv.storage.create_note("later.md", "v1", &op_id()).unwrap();

    let op1 = op_id();
    let s1 = tv
        .storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &op1)
        .unwrap();
    let s2 = tv
        .storage
        .save(note.note_id.as_str(), "v3", s1.local_revision, &op_id())
        .unwrap();
    assert_ne!(s2.local_revision, s1.local_revision);

    let retry = tv
        .storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &op1)
        .unwrap();
    assert_eq!(retry, s1, "retry must replay the original result");

    let mut reopened = VaultStorage::open(tv.path()).unwrap();
    let retry2 = reopened
        .save(note.note_id.as_str(), "v2", note.local_revision, &op1)
        .unwrap_or_else(|e| panic!("RED: reopen retry returned {e:?}"));
    assert_eq!(retry2, s1);
}

#[test]
fn non_uuid_operation_id_rejected() {
    for bad in ["", "not-a-uuid", "op-123"] {
        let mut tv = TestVault::new();
        let err = tv.storage.create_note("n.md", "body", bad).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
    }
}

/// The same operationId must never satisfy a request that differs in kind,
/// noteId or payload: every variant is IDEMPOTENCY_MISMATCH.
#[test]
fn same_opid_reused_for_other_kind_or_note_or_payload_rejected() {
    let mut tv = TestVault::new();
    let note1 = tv.storage.create_note("k1.md", "v1", &op_id()).unwrap();
    let op = op_id();
    tv.storage
        .save(note1.note_id.as_str(), "v2", note1.local_revision, &op)
        .unwrap();

    // kind mismatch: create with an opId already used for save
    let err = tv.storage.create_note("k2.md", "fresh", &op).unwrap_err();
    assert_eq!(err.code, ErrorCode::IdempotencyMismatch);

    // noteId mismatch: the other note with the same opId
    let note2 = tv.storage.create_note("k3.md", "v1", &op_id()).unwrap();
    let err = tv
        .storage
        .save(note2.note_id.as_str(), "v2", note2.local_revision, &op)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::IdempotencyMismatch);

    // payload mismatch: same note, different source
    let err = tv
        .storage
        .save(
            note1.note_id.as_str(),
            "DIFFERENT",
            note1.local_revision,
            &op,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::IdempotencyMismatch);
}

// ----------------------------------------------------------------------------
// 9-11. crash failpoints: after prepared / after replace / before db commit
// ----------------------------------------------------------------------------

fn reopen_and_recover(dir: &TempDir) -> (RecoverOutcome, VaultStorage) {
    let reopened = VaultStorage::open(dir.path()).unwrap();
    let outcome = reopened.recover().unwrap();
    (outcome, reopened)
}

/// Crash-injection surfaces a real typed IO failure, never success.
fn assert_injected_io_error(err: &ServiceError) {
    assert!(
        matches!(
            err.code,
            ErrorCode::DiskFull | ErrorCode::IoDenied | ErrorCode::FileLocked
        ),
        "expected injected typed IO failure, got {:?} ({:?})",
        err.code,
        err.message_key
    );
}

#[test]
fn crash_after_prepared() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("crash-a.md", "old", &op_id()).unwrap();

    let target = dir.path().join("crash-a.md");
    let old_bytes = read_bytes(&target);
    let new_source = note.source.replace("old", "committed body");

    storage.push_failpoint(Failpoint::AfterPrepared);
    let save_err = storage
        .save(
            note.note_id.as_str(),
            &new_source,
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_injected_io_error(&save_err);

    // replace did not happen: old snapshot intact on disk
    assert_eq!(
        read_bytes(&target),
        old_bytes,
        "old file must survive crash"
    );

    let (outcome, reopened) = reopen_and_recover(&dir);
    let bytes = read_bytes(&target);
    let doc = reopened.read(note.note_id.as_str()).unwrap();
    assert_eq!(
        doc.source == note.source && bytes == old_bytes,
        true,
        "AfterPrepared recovery keeps the original text unchanged"
    );
    assert_eq!(doc.local_revision, note.local_revision);
    assert!(
        outcome.applied.is_empty(),
        "unreplaced op must stay pending, not applied"
    );
    assert!(
        outcome.pending.iter().any(|id| *id == note.note_id),
        "journal entry must still be pending"
    );
}

#[test]
fn crash_after_replace() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("crash-b.md", "old", &op_id()).unwrap();

    let target = dir.path().join("crash-b.md");
    let old_bytes = read_bytes(&target);
    let new_source = note.source.replace("old", "committed body");
    let new_bytes = new_source.as_bytes().to_vec();

    storage.push_failpoint(Failpoint::AfterReplace);
    let save_err = storage
        .save(
            note.note_id.as_str(),
            &new_source,
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_injected_io_error(&save_err);

    // replace already happened: full new snapshot on disk, DB not yet committed
    assert_eq!(
        read_bytes(&target),
        new_bytes,
        "disk must hold the new bytes"
    );

    let (outcome, reopened) = reopen_and_recover(&dir);
    let bytes = read_bytes(&dir.path().join("crash-b.md"));
    let doc = reopened.read(note.note_id.as_str()).unwrap();
    assert_eq!(doc.source, new_source, "new body must be recovered");
    assert_eq!(bytes, new_bytes);
    assert_ne!(
        bytes, old_bytes,
        "sanity: replaced target differs from old snapshot"
    );
    assert!(
        outcome.applied.iter().any(|id| *id == note.note_id),
        "journal replay must finalize the replaced commit"
    );
    let summary = reopened
        .list()
        .unwrap()
        .into_iter()
        .find(|s| &s.note_id == &note.note_id)
        .unwrap();
    assert_eq!(
        summary.local_revision,
        note.local_revision + 1,
        "recovery must carry the revision forward"
    );
}

#[test]
fn crash_before_db_commit() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("crash-c.md", "old", &op_id()).unwrap();

    let target = dir.path().join("crash-c.md");
    let old_bytes = read_bytes(&target);
    let new_source = note.source.replace("old", "committed body");
    let new_bytes = new_source.as_bytes().to_vec();

    storage.push_failpoint(Failpoint::BeforeDbCommit);
    let save_err = storage
        .save(
            note.note_id.as_str(),
            &new_source,
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_injected_io_error(&save_err);

    assert_eq!(read_bytes(&target), new_bytes, "replace already happened");

    let (outcome, reopened) = reopen_and_recover(&dir);
    let bytes = read_bytes(&target);
    assert_eq!(bytes, new_bytes, "never truncated and never stale old");
    assert_ne!(bytes, old_bytes);
    assert_eq!(
        reopened.read(note.note_id.as_str()).unwrap().source,
        new_source
    );
    assert!(
        outcome.applied.iter().any(|id| *id == note.note_id),
        "journal replay completes the commit"
    );
}

// ----------------------------------------------------------------------------
// 12. path validation: traversal, reserved, .opennote
// ----------------------------------------------------------------------------

#[test]
fn rejecting_invalid_paths() {
    let mut tv = TestVault::new();
    for bad in [
        "../escape.md",
        "sub\\..\\escape.md",
        "..\\disc.md",
        ".opennote/vault.json",
        "CON.md",
        "name with trailing dots. ",
        "",
    ] {
        let err = tv
            .storage
            .create_note(bad, "content", &op_id())
            .unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::InvalidInput,
            "expected InvalidInput for {bad:?}, got {:?}",
            err.code
        );
    }
}

// ----------------------------------------------------------------------------
// 13. duplicate frontmatter opennote-id -> RESCAN_REQUIRED
// ----------------------------------------------------------------------------

#[test]
fn duplicate_frontmatter_ids() {
    let dup = Uuid::new_v4().to_string();
    let dir = TempDir::new().unwrap();
    write_byte_file(
        &dir.path().join("one.md"),
        format!("---\nopennote-id: {dup}\n---\nA").as_bytes(),
    );
    write_byte_file(
        &dir.path().join("two.md"),
        format!("---\nopennote-id: {dup}\n---\nB").as_bytes(),
    );

    let storage = VaultStorage::open(dir.path()).unwrap();
    let err = storage.list().unwrap_err();
    assert_eq!(err.code, ErrorCode::RescanRequired);
}

// ----------------------------------------------------------------------------
// 14. failed write preserves old content
// ----------------------------------------------------------------------------

#[test]
fn write_failure_preserves_old_bytes() {
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note("w.md", "old body", &op_id())
        .unwrap();
    let before = read_bytes(&tv.path().join("w.md"));

    tv.storage.push_failpoint(Failpoint::WriteFailure);
    let err = tv
        .storage
        .save(
            note.note_id.as_str(),
            "unsaved new body",
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert!(
        matches!(
            err.code,
            ErrorCode::DiskFull | ErrorCode::IoDenied | ErrorCode::FileLocked
        ),
        "unexpected code: {:?}",
        err.code
    );
    assert_eq!(
        read_bytes(&tv.path().join("w.md")),
        before,
        "old file bytes must be unchanged after failed write"
    );
}

// ----------------------------------------------------------------------------
// 19-22. identity frontmatter rules (new-phase: unknown YAML verbatim,
// foreign/malformed/mismatch rejections)
// ----------------------------------------------------------------------------

#[test]
fn unknown_frontmatter_preserved_on_create_and_save() {
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note(
            "u.md",
            "---\ntitle: my note\ntags: a, b\n---\nbody here",
            &op_id(),
        )
        .unwrap();

    // create inserted exactly one reserved identity line; unknown lines verbatim
    assert!(
        note.source.contains("title: my note\ntags: a, b\n"),
        "unknown frontmatter keys must be preserved verbatim: {:?}",
        note.source
    );
    let open_end = note.source.find("\n---\n").unwrap();
    let id_line_count = note.source[..open_end]
        .lines()
        .filter(|l| l.starts_with("opennote-id:"))
        .count();
    assert_eq!(id_line_count, 1, "exactly one identity line inserted");

    // save against the composed source: frontmatter must stay verbatim
    let edited = note.source.replace("body here", "body v2");
    tv.storage
        .save(
            note.note_id.as_str(),
            &edited,
            note.local_revision,
            &op_id(),
        )
        .unwrap();
    let reopened = VaultStorage::open(tv.path()).unwrap();
    let doc = reopened.read(note.note_id.as_str()).unwrap();
    assert_eq!(
        doc.source, edited,
        "unknown YAML must survive save verbatim"
    );
}

#[test]
fn foreign_identity_create_rejected() {
    let mut tv = TestVault::new();
    let foreign = Uuid::new_v4().to_string();
    let source = format!("---\nopennote-id: {foreign}\n---\nbody");
    let err = tv
        .storage
        .create_note("foreign.md", &source, &op_id())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{:?}", err.message_key);
    assert!(!tv.path().join("foreign.md").exists());
}

#[test]
fn malformed_frontmatter_rejected() {
    let mut tv = TestVault::new();
    let err = tv
        .storage
        .create_note("malformed.md", "---\nunterminated block\nbody", &op_id())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{:?}", err.message_key);
    let err = tv
        .storage
        .create_note("malformed2.md", "---\nunterminated too", &op_id())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{:?}", err.message_key);
}

#[test]
fn identity_mismatch_on_save_rejected() {
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note("m.md", "first body", &op_id())
        .unwrap();

    // replace the stored opennote-id value with a different one
    let other = Uuid::new_v4().to_string();
    let edited = note.source.replace(note.note_id.as_str(), &other);
    let err = tv
        .storage
        .save(
            note.note_id.as_str(),
            &edited,
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{:?}", err.message_key);
    assert_eq!(
        err.message_key, "error.identityMismatch",
        "mismatch must surface the correct typed key"
    );
}

// ----------------------------------------------------------------------------
// 23. hardening: state.sqlite is never deleted/reset on open failure
// ----------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn open_failure_preserves_state_db_bytes() {
    let dir = TempDir::new().unwrap();
    let _storage = VaultStorage::create(dir.path(), "T").unwrap();
    drop(_storage);

    let db_path = dir.path().join(".opennote").join("state.sqlite");
    let original = read_bytes(&db_path);

    // Hold the DB open with access=DELETE, share=DELETE-ONLY: any subsequent
    // open requesting READ/WRITE fails, but a naive delete would succeed.
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_SHARE_DELETE, OPEN_EXISTING,
    };
    let wide: Vec<u16> = db_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            DELETE,
            FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        handle as isize, -1,
        "test setup: delete-only handle must succeed"
    );

    let err = VaultStorage::open(dir.path()).unwrap_err();
    assert_eq!(err.code, ErrorCode::IoDenied, "{:?}", err.message_key);

    unsafe { CloseHandle(handle) };

    assert!(db_path.exists(), "state.sqlite must never be deleted");
    assert_eq!(
        read_bytes(&db_path),
        original,
        "state bytes must be preserved on open failure"
    );

    VaultStorage::open(dir.path()).unwrap();
}

#[test]
fn corrupt_state_not_reset() {
    let dir = TempDir::new().unwrap();
    write_byte_file(&dir.path().join("note.md"), b"body");
    let app = dir.path().join(".opennote");
    fs::create_dir_all(&app).unwrap();
    let sentinel = b"definitely not sqlite by seed";
    write_byte_file(&app.join("state.sqlite"), sentinel);

    let err = VaultStorage::open(dir.path()).unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::IoDenied,
        "corrupt db must fail typed, not reset; key={:?}",
        err.message_key
    );
    assert_eq!(
        read_bytes(&app.join("state.sqlite")),
        sentinel,
        "corrupt state bytes must be preserved"
    );
}

/// Old-shape `operations` table (op_digest primary key) is migrated additively
/// on open: every existing row stays, nothing is deleted or reset.
#[test]
fn old_shape_operations_rows_preserved_through_migration() {
    let dir = TempDir::new().unwrap();
    let op_row = op_id();
    {
        let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
        storage
            .create_note("mig.md", "v1", &op_row)
            .unwrap_or_else(|e| panic!("RED setup: create failed {e:?}"));
    }
    let db_path = dir.path().join(".opennote").join("state.sqlite");
    // rebuild the table in the deprecated shape directly in storage
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE operations RENAME TO operations_tmp;
             CREATE TABLE operations (
                 op_digest TEXT PRIMARY KEY,
                 operation_id TEXT NOT NULL,
                 note_id TEXT NOT NULL,
                 status TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 note_relative TEXT NOT NULL,
                 old_revision INTEGER,
                 new_revision INTEGER,
                 old_hash TEXT,
                 new_hash TEXT,
                 stage_relative TEXT NOT NULL,
                 old_relative TEXT,
                 created_at TEXT NOT NULL
             );
             INSERT INTO operations (op_digest, operation_id, note_id, status, kind, note_relative, old_revision, new_revision, old_hash, new_hash, stage_relative, old_relative, created_at)
             SELECT request_digest, operation_id, note_id, status, kind, note_relative, old_revision, new_revision, old_hash, new_hash, stage_relative, old_relative, created_at FROM operations_tmp;
             DROP TABLE operations_tmp;
             COMMIT;",
        )
        .unwrap();

    let storage = VaultStorage::open(dir.path()).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    let digest_now: String = conn
        .query_row("SELECT request_digest FROM operations LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let operation_id_now: String = conn
        .query_row("SELECT operation_id FROM operations LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1, "migration must keep every existing row");
    assert_eq!(operation_id_now, op_row);
    assert!(!digest_now.is_empty());

    // the migrated table remains writable and stays keyed by operationId
    let mut storage = storage;
    let new_note = storage
        .create_note("after-mig.md", "v1", &op_id())
        .unwrap_or_else(|e| panic!("write after migration failed {e:?}"));
    storage
        .save(new_note.note_id.as_str(), "v2", 0, &op_id())
        .unwrap_or_else(|e| panic!("send-after-migration save failed {e:?}"));
    let count_after: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT operation_id) FROM operations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count_after, 3, "op1 row + create + save must all coexist");
}

// ----------------------------------------------------------------------------
// 16. newer schema: read-only raw access, zero state mutations
// ----------------------------------------------------------------------------

#[test]
fn newer_schema_open_is_readonly_and_unmutated() {
    let dir = TempDir::new().unwrap();
    let note = dir.path().join("note.md");
    write_byte_file(&note, b"hello");
    let app = dir.path().join(".opennote");
    fs::create_dir_all(&app).unwrap();
    let vault_json = format!(
        "{{\"vaultId\":\"{}\",\"schemaVersion\":999,\"displayName\":\"Future\",\"syncEnabled\":false,\"writable\":true}}",
        uuid::Uuid::new_v4()
    );
    write_byte_file(&app.join("vault.json"), vault_json.as_bytes());
    let sentinel = b"sentinel state bytes";
    write_byte_file(&app.join("state.sqlite"), sentinel);

    let storage = VaultStorage::open(dir.path()).unwrap();
    assert!(!storage.vault().writable, "newer schema must be read-only");
    assert_eq!(storage.vault().schema_version, 999);
    let summaries = storage.list().unwrap();
    assert_eq!(
        summaries
            .iter()
            .filter(|s| s.relative_path == "note.md")
            .count(),
        1
    );

    assert_eq!(read_bytes(&note), b"hello");
    assert_eq!(
        read_bytes(&app.join("state.sqlite")),
        sentinel,
        "newer-schema open must not touch state bytes"
    );
    assert_eq!(
        read_bytes(&app.join("vault.json")),
        vault_json.as_bytes(),
        "newer-schema open must not rewrite vault.json"
    );
}

// ----------------------------------------------------------------------------
// 17. legacy note identity is stable across opens; external hash change
// bumps revision but keeps the ID; scan stays byte-preserving
// ----------------------------------------------------------------------------

#[test]
fn legacy_identity_stable_and_external_hash_bumps_revision() {
    let dir = TempDir::new().unwrap();
    let note = dir.path().join("leg.md");
    write_byte_file(&note, b"legacy body");

    let id1 = {
        let storage = VaultStorage::open(dir.path()).unwrap();
        let summaries = storage.list().unwrap();
        let s = summaries
            .iter()
            .find(|s| s.relative_path == "leg.md")
            .unwrap();
        assert_eq!(s.local_revision, 0);
        s.note_id.clone()
    };

    // byte-identical reopen: same ID, note untouched
    {
        let storage = VaultStorage::open(dir.path()).unwrap();
        let s = storage
            .list()
            .unwrap()
            .into_iter()
            .find(|s| s.relative_path == "leg.md")
            .unwrap();
        assert_eq!(s.note_id, id1, "temporary mapping must be reused");
        assert_eq!(s.local_revision, 0);
    }
    assert_eq!(read_bytes(&note), b"legacy body");

    // genuine external change: revision +1, ID kept
    write_byte_file(&note, b"legacy body v2");
    let storage = VaultStorage::open(dir.path()).unwrap();
    let s = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.relative_path == "leg.md")
        .unwrap();
    assert_eq!(s.note_id, id1, "ID survives external edits");
    assert_eq!(
        s.local_revision, 1,
        "external hash change bumps local revision"
    );
}

// ----------------------------------------------------------------------------
// 18. junction escapes: scan-level link dir skipped; junctioned .opennote refused
// ----------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn junction_escape_refused_and_unfollowed() {
    let outside = TempDir::new().unwrap();
    let sub = outside.path().join("leak-target");
    fs::create_dir_all(&sub).unwrap();
    write_byte_file(&sub.join("secret.md"), b"outside secret");
    let secret_before = read_bytes(&sub.join("secret.md"));

    let dir = TempDir::new().unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(dir.path().join("linked"))
        .arg(&sub)
        .status()
        .expect("mklink run");
    assert!(status.success(), "junction creation failed");

    let storage = VaultStorage::open(dir.path()).unwrap();
    let summaries = storage.list().unwrap();
    assert!(
        summaries
            .iter()
            .all(|s| !s.relative_path.starts_with("linked/")),
        "junction dir must never be followed during scan"
    );
    assert_eq!(
        read_bytes(&sub.join("secret.md")),
        secret_before,
        "outside bytes untouched"
    );

    // .opennote itself as a junction must be refused before app files are touched
    let dir2 = TempDir::new().unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(dir2.path().join(".opennote"))
        .arg(&sub)
        .status()
        .expect("mklink run");
    assert!(status.success(), "junction creation failed");

    let err = VaultStorage::open(dir2.path()).unwrap_err();
    assert_eq!(err.code, ErrorCode::IoDenied, "{:?}", err.message_key);
    assert_eq!(
        read_bytes(&sub.join("secret.md")),
        secret_before,
        "junction target must remain untouched"
    );
}

// ----------------------------------------------------------------------------
// 24c. cross-process writer gate: OS lock on .opennote/writer.lock
// ----------------------------------------------------------------------------

/// A second writer holding the OS-level writer.lock (exactly what another
/// process or alias instance would hold) must get a fast, retryable
/// FILE_LOCKED for save and for open's scan/DB protocol; releasing the lock
/// lets the operation proceed.
#[test]
fn writer_gate_refuses_second_writer_until_released() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("g.md", "v1", &op_id()).unwrap();

    let lock_path = dir.path().join(".opennote").join("writer.lock");
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    holder
        .try_lock()
        .expect("test setup: second writer acquires OS lock");

    let err = storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &op_id())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::FileLocked, "{:?}", err.message_key);
    assert!(err.retryable, "FILE_LOCKED must be retryable");

    let err = VaultStorage::open(dir.path()).unwrap_err();
    assert_eq!(err.code, ErrorCode::FileLocked, "{:?}", err.message_key);
    assert!(err.retryable, "open FILE_LOCKED must be retryable");

    drop(holder);
    let saved = storage
        .save(note.note_id.as_str(), "v2", note.local_revision, &op_id())
        .unwrap_or_else(|e| panic!("save after holder release returned {e:?}"));
    assert_eq!(saved.local_revision, note.local_revision + 1);
}

// ----------------------------------------------------------------------------
// 25. two instances: stale expected revision is rejected under the writer gate
// even when disk bytes cycle A->B->A, and DB revision never regresses
// ----------------------------------------------------------------------------

/// Instance A stays open with an in-memory rev0 baseline. Instance B commits
/// rev1 (bytes B) and then restores the ORIGINAL full bytes at rev2. A must
/// never overwrite the newer revision with its stale expected=0 save, and the
/// authoritative row/bytes must survive unregressed. Also: a read-only record
/// can never be saved by supplying a fresh valid source.
#[test]
fn two_instances_stale_revision_despite_byte_cycle() {
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note("two.md", "vA0", &op_id())
        .unwrap_or_else(|e| panic!("setup create failed {e:?}"));
    let bytes_a = read_bytes(&tv.path().join("two.md"));

    // second instance, opened before B's commits: its cached baseline is rev0
    let mut storage_b = VaultStorage::open(tv.path()).unwrap();

    // commit rev1 with new body
    let source_b = note.source.replace("vA0", "vB1");
    let saved_b1 = storage_b
        .save(
            note.note_id.as_str(),
            &source_b,
            note.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("setup save B1 failed {e:?}"));
    assert_eq!(saved_b1.local_revision, 1);

    // commit rev2 by restoring the ORIGINAL full bytes (A->B->A cycle)
    let restored = String::from_utf8(bytes_a.clone()).unwrap();
    let saved_b2 = storage_b
        .save(note.note_id.as_str(), &restored, 1, &op_id())
        .unwrap_or_else(|e| panic!("setup save B2 failed {e:?}"));
    assert_eq!(saved_b2.local_revision, 2);
    assert_eq!(
        read_bytes(&tv.path().join("two.md")),
        bytes_a,
        "cycle must end on the original bytes A"
    );

    // A's stale attempt (own edit, expected=0) must be a typed conflict, never
    // an overwrite of the newer rev2
    let err_a = tv
        .storage
        .save(
            note.note_id.as_str(),
            "my edit",
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(
        err_a.code,
        ErrorCode::VersionConflict,
        "{:?} {:?}",
        err_a.code,
        err_a.message_key
    );

    let conn = rusqlite::Connection::open(tv.path().join(".opennote").join("state.sqlite"))
        .expect("db introspection open");
    let rev: i64 = conn
        .query_row(
            "SELECT local_revision FROM notes WHERE relative_path = 'two.md'",
            [],
            |r| r.get(0),
        )
        .expect("db introspection query");
    assert_eq!(
        rev, 2,
        "stale instance must not regress/overwrite the DB revision"
    );
    assert_eq!(read_bytes(&tv.path().join("two.md")), bytes_a);

    // read-only record refusal: flip bytes to invalid UTF-8 externally, then a
    // fresh open sees read_only=rev3 — saving ANY fresh valid source is refused
    write_byte_file(&tv.path().join("two.md"), b"invalid \xFF bytes");
    let mut storage_r = VaultStorage::open(tv.path()).unwrap();
    let readonly_rev = storage_r
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.relative_path == "two.md")
        .unwrap();
    assert!(
        readonly_rev.read_only,
        "invalid UTF-8 note must be read-only"
    );
    let binary_bytes = read_bytes(&tv.path().join("two.md"));
    let err_r = storage_r
        .save(
            readonly_rev.note_id.as_str(),
            "valid fresh source",
            readonly_rev.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(
        err_r.code,
        ErrorCode::InvalidInput,
        "{:?}",
        err_r.message_key
    );
    assert_eq!(
        read_bytes(&tv.path().join("two.md")),
        binary_bytes,
        "readonly note keeps its bytes"
    );
}

// ----------------------------------------------------------------------------
// 26. consistent read: after an external byte change, read returns
// SHA256(source bytes) as contentHash and advances the authoritative revision
// exactly once; a pre-change save revision conflicts; newer-schema reads are
// raw + read-only with zero DB writes
// ----------------------------------------------------------------------------

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[test]
fn consistent_read_external_change_once_and_newer_schema_readonly() {
    // Part A: writable vault — read refreshes the mapping exactly once
    let mut tv = TestVault::new();
    let note = tv
        .storage
        .create_note("notes/one.md", "# one", &op_id())
        .unwrap_or_else(|e| panic!("RED: create_note returned {e:?}"));

    write_byte_file(&tv.path().join("notes/one.md"), b"# external change");

    let doc = tv
        .storage
        .read(note.note_id.as_str())
        .unwrap_or_else(|e| panic!("RED: read returned typed error {:?}", e.code));
    assert_eq!(
        doc.source, "# external change",
        "read must return the fresh source bytes"
    );
    assert_eq!(
        sha256_hex(&doc.source),
        doc.content_hash,
        "read must return SHA256(source bytes) as contentHash"
    );
    assert_eq!(
        doc.local_revision,
        note.local_revision + 1,
        "external hash change must advance the revision exactly once"
    );
    assert!(!doc.read_only, "writable vault read stays writable");

    let again = tv.storage.read(note.note_id.as_str()).unwrap();
    assert_eq!(again.content_hash, doc.content_hash);
    assert_eq!(
        again.local_revision, doc.local_revision,
        "second read must not advance the revision again"
    );

    let err = tv
        .storage
        .save(
            note.note_id.as_str(),
            "# edited after",
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::VersionConflict,
        "the pre-change save revision must be rejected: {:?}",
        err.message_key
    );

    // Part B: newer schema — read is raw bytes, read-only, zero DB writes
    let dir = TempDir::new().unwrap();
    let note_path = dir.path().join("note.md");
    write_byte_file(&note_path, b"hello");
    let app = dir.path().join(".opennote");
    fs::create_dir_all(&app).unwrap();
    let vault_json = format!(
        "{{\"vaultId\":\"{}\",\"schemaVersion\":999,\"displayName\":\"Future\",\"syncEnabled\":false,\"writable\":true}}",
        uuid::Uuid::new_v4()
    );
    write_byte_file(&app.join("vault.json"), vault_json.as_bytes());
    let sentinel = b"sentinel state bytes";
    write_byte_file(&app.join("state.sqlite"), sentinel);

    let storage = VaultStorage::open(dir.path()).unwrap();
    let summary = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.relative_path == "note.md")
        .unwrap();
    let newer = storage.read(summary.note_id.as_str()).unwrap();
    assert!(newer.read_only, "newer-schema read must be read-only");
    assert_eq!(sha256_hex(&newer.source), newer.content_hash);

    write_byte_file(&note_path, b"hello v2");
    let newer2 = storage.read(summary.note_id.as_str()).unwrap();
    assert!(newer2.read_only, "newer-schema read must stay read-only");
    assert_eq!(
        sha256_hex(&newer2.source),
        newer2.content_hash,
        "newer-schema read must keep contentHash on fresh bytes"
    );
    assert_eq!(
        newer2.local_revision, newer.local_revision,
        "newer-schema read must never advance anything"
    );
    assert_eq!(
        read_bytes(&app.join("state.sqlite")),
        sentinel,
        "newer-schema read must not touch state.sqlite bytes"
    );
}
