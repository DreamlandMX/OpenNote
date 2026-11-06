use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

use onote_domain::{CheckpointRequest, ErrorCode, ServiceError};
use onote_vault_storage::{Failpoint, VaultStorage};

// ----------------------------------------------------------------------------
// helpers (deterministic, per-test temporary dirs)
// ----------------------------------------------------------------------------

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn op_id() -> String {
    Uuid::new_v4().to_string()
}

struct TestVault {
    storage: VaultStorage,
    _dir: TempDir,
}

impl TestVault {
    fn new() -> TestVault {
        let _dir = TempDir::new().unwrap();
        let storage = VaultStorage::create(_dir.path(), "DraftVault").unwrap();
        TestVault { storage, _dir }
    }

    fn path(&self) -> &Path {
        self._dir.path()
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.path().join(".opennote").join("state.sqlite"))
            .expect("db introspection open")
    }
}

/// Create a baseline note and return (noteId, relativePath, revision, hash).
fn seed_note(storage: &mut VaultStorage, relative_path: &str, body: &str) -> (String, u64, String) {
    let doc = storage
        .create_note(relative_path, body, &op_id())
        .unwrap_or_else(|e| panic!("seed create failed {e:?}"));
    (
        doc.note_id.clone(),
        doc.local_revision,
        doc.content_hash.clone(),
    )
}

fn checkpoint_request(
    note_id: &str,
    relative_path: &str,
    source: &str,
    base_revision: u64,
    base_hash: &str,
) -> CheckpointRequest {
    CheckpointRequest {
        note_id: note_id.to_owned(),
        relative_path: relative_path.to_owned(),
        source: source.to_owned(),
        base_revision,
        base_content_hash: base_hash.to_owned(),
        operation_id: op_id(),
    }
}

fn draft_count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .expect("introspect draft_checkpoints")
}

fn assert_drafts(corrupted: &ServiceError, code: ErrorCode) {
    assert_eq!(
        corrupted.code, code,
        "code={:?} key={:?}",
        corrupted.code, corrupted.message_key
    );
}

// ----------------------------------------------------------------------------
// 1. reopen reads the EXACT original draft bytes (BOM/CRLF/Unicode/incomplete
//    YAML), hash matches, timestamps are UTC RFC3339 with Z
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_reopen_exact_raw_bytes_and_hash() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "notes/d.md", "body");

    // BOM + CRLF + incomplete/broken YAML + Unicode — never normalized here
    let raw =
        "\u{FEFF}---\r\nunfinished: [\r\n  broken: \"unterminated\r\n正文 Déjà-vu 🌿 café\r\n";
    let req = checkpoint_request(&note_id, "notes/d.md", raw, rev, &hash);
    let op = req.operation_id.clone();
    let result = tv
        .storage
        .write_checkpoint(req)
        .unwrap_or_else(|e| panic!("RED: write_checkpoint returned {e:?}"));

    assert_eq!(
        result.draft_id, op,
        "draftId must equal the caller operationId"
    );
    assert_eq!(result.note_id, note_id);
    assert_eq!(
        result.source_hash,
        sha256_hex(raw),
        "hash pinned over exact raw bytes"
    );
    assert!(result.sequence >= 1);
    assert!(
        result.saved_at.len() >= 20
            && result.saved_at.ends_with('Z')
            && result.saved_at.contains('T'),
        "savedAt must be UTC RFC3339, got {:?}",
        result.saved_at
    );

    // reopen: draft survives restart byte-for-byte
    let reopened = VaultStorage::open(tv.path()).unwrap();
    let draft = reopened
        .read_draft(&result.draft_id)
        .unwrap_or_else(|e| panic!("RED reopen: read_draft returned {e:?}"));
    assert_eq!(draft.source, raw, "BOM/CRLF/incomplete YAML must be intact");
    assert_eq!(draft.relative_path, "notes/d.md");
    assert_eq!(draft.base_revision, rev, "baseline revision preserved");
    assert_eq!(draft.base_content_hash, hash, "baseline hash preserved");
    assert_eq!(draft.source_hash, sha256_hex(raw));
    assert_eq!(draft.draft_id, result.draft_id);
    assert_eq!(draft.sequence, result.sequence);
}

// ----------------------------------------------------------------------------
// 2. same operationId + same request replay -> identical result;
//    same operationId + different payload -> typed mismatch
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_same_id_replays_mismatch_rejects() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "idem.md", "body");
    let req = checkpoint_request(&note_id, "idem.md", "draft v1", rev, &hash);
    let first = tv.storage.write_checkpoint(req.clone()).unwrap();
    let replay_same = tv
        .storage
        .write_checkpoint(req.clone())
        .unwrap_or_else(|e| panic!("RED: identical replay rejected {e:?}"));
    assert_eq!(
        replay_same, first,
        "same id + same payload must return the original result verbatim"
    );

    // SAME operationId, different payload -> typed mismatch
    let mut other = checkpoint_request(&note_id, "idem.md", "draft OTHER", rev, &hash);
    other.operation_id = first.draft_id.clone();
    let mismatch = tv.storage.write_checkpoint(other).unwrap_err();
    assert_drafts(&mismatch, ErrorCode::IdempotencyMismatch);

    // one row only despite the two identical writes
    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        1
    );
}

// ----------------------------------------------------------------------------
// 3. two instances interleave: both operations' checkpoints are preserved
//    (no erase/reuse), notes table untouched
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_two_instances_interleave_preserves_both() {
    let mut a = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut a.storage, "two.md", "body");

    let res_a = a
        .storage
        .write_checkpoint(checkpoint_request(&note_id, "two.md", "cfg A", rev, &hash))
        .unwrap_or_else(|e| panic!("RED: instance A write failed {e:?}"));

    // second independent instance of the SAME vault on disk
    let b = VaultStorage::open(a.path()).unwrap();
    let res_b = b
        .write_checkpoint(checkpoint_request(&note_id, "two.md", "cfg B", rev, &hash))
        .unwrap_or_else(|e| panic!("RED: instance B write failed {e:?}"));

    assert_ne!(res_a.draft_id, res_b.draft_id);
    assert_ne!(
        res_a.sequence, res_b.sequence,
        "distinct DB sequences, no overwrite"
    );

    let page = b.list_drafts(None, None, Some(50)).unwrap();
    assert_eq!(page.items.len(), 2, "both instances' drafts must coexist");
    let ids: Vec<&str> = page.items.iter().map(|s| s.draft_id.as_str()).collect();
    assert!(ids.contains(&res_a.draft_id.as_str()));
    assert!(ids.contains(&res_b.draft_id.as_str()));

    // note mapping untouched by checkpoint writes
    let db = a.db();
    let rev: i64 = db
        .query_row(
            "SELECT local_revision FROM notes WHERE relative_path='two.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        rev, 0,
        "checkpoint writes must not change the note revision"
    );
    let ops: i64 = db
        .query_row("SELECT COUNT(*) FROM operations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ops, 1, "operations journal must stay untouched");
}

// ----------------------------------------------------------------------------
// 4. baseline preserved when the disk file later changes or is deleted;
//    read still verifies integrity before exposure
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_baseline_survives_disk_change_and_delete() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "gone.md", "body");
    let res = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "gone.md",
            "earlier draft",
            rev,
            &hash,
        ))
        .unwrap_or_else(|e| panic!("RED: checkpoint write failed {e:?}"));

    let db_bytes_before = { fs::read(tv.path().join(".opennote").join("state.sqlite")).unwrap() };

    // external rewrite then delete of the note file: drafts must remain readable
    fs::write(tv.path().join("gone.md"), b"external rewrite bytes").unwrap();
    fs::remove_file(tv.path().join("gone.md")).unwrap();

    let reopened = VaultStorage::open(tv.path()).unwrap();
    let draft = reopened
        .read_draft(&res.draft_id)
        .unwrap_or_else(|e| panic!("RED: read after delete failed {e:?}"));
    assert_eq!(draft.source, "earlier draft");
    assert_eq!(draft.base_revision, rev);
    assert_eq!(draft.base_content_hash, hash);

    let page = reopened
        .list_drafts(Some(&note_id), None, Some(50))
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].draft_id, res.draft_id);

    // notes table row must be untouched by checkpoint operations
    let db = tv.db();
    let mapped_hash: String = db
        .query_row(
            "SELECT content_hash FROM notes WHERE relative_path='gone.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        mapped_hash, hash,
        "authoritative mapping must not absorb the external change"
    );
    let _ = db_bytes_before;
}

// ----------------------------------------------------------------------------
// 5. invalid identifiers/paths/hashes/bounds refuse typed, before any write
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_invalid_inputs_typed_refusal() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "valid.md", "body");

    // non-canonical (uppercase) UUID operationId
    let mut req = checkpoint_request(&note_id, "valid.md", "s", rev, &hash);
    req.operation_id = op_id().to_uppercase();
    let err = tv.storage.write_checkpoint(req).unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // unknown noteId -> ObjectNotFound
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(&op_id(), "valid.md", "s", rev, &hash))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::ObjectNotFound);

    // path diverging from the authoritative note mapping
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(&note_id, "other.md", "s", rev, &hash))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // traversal path shape
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(&note_id, "../evil.md", "s", rev, &hash))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // malformed baseline hash (not 64 lowercase hex)
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id, "valid.md", "s", rev, "DEADBEEF",
        ))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // numeric bound: baseRevision beyond SQLite i64 range
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "valid.md",
            "s",
            (i64::MAX as u64) + 1,
            &hash,
        ))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // read-side guards: unknown draft id, non-uuid draft id
    let err = tv.storage.read_draft(&op_id()).unwrap_err();
    assert_drafts(&err, ErrorCode::ObjectNotFound);
    let err = tv.storage.read_draft("not-a-uuid").unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // none of the failures above wrote a draft row
    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        0
    );
}

// ----------------------------------------------------------------------------
// 6. list: sequence DESC, metadata-only summaries, stable before-cursor
//    pagination, default 50 / capped at 200
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_list_desc_metadata_only_pagination() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "paged.md", "body");

    let mut seqs = Vec::new();
    for i in 0..5 {
        let res = tv
            .storage
            .write_checkpoint(checkpoint_request(
                &note_id,
                "paged.md",
                &format!("draft {i}"),
                rev,
                &hash,
            ))
            .unwrap_or_else(|e| panic!("RED setup: checkpoint {i} failed {e:?}"));
        seqs.push((res.draft_id.clone(), res.sequence));
    }

    // default (None) limit returns the full 5-item list, newest first
    let page = tv.storage.list_drafts(None, None, None).unwrap();
    assert_eq!(page.items.len(), 5);
    assert_eq!(
        page.items[0].draft_id, seqs[4].0,
        "descending by DB sequence"
    );
    assert_eq!(page.items[4].sequence, seqs[0].1);
    for w in page.items.windows(2) {
        assert!(w[0].sequence > w[1].sequence, "strictly descending");
    }
    assert_eq!(page.next_before_sequence, None, "final page has no cursor");
    // metadata-only summaries carry no source, but do carry the mapping fields
    assert_eq!(page.items[0].relative_path, "paged.md");
    assert_eq!(page.items[0].source_hash, sha256_hex("draft 4"));

    // pages of two are stable: [5,4] cursor 4 -> [3,2] cursor 2 -> [1] no cursor
    let p1 = tv.storage.list_drafts(None, None, Some(2)).unwrap();
    assert_eq!(
        p1.items.iter().map(|s| s.sequence).collect::<Vec<_>>(),
        vec![seqs[4].1, seqs[3].1]
    );
    assert_eq!(p1.next_before_sequence, Some(seqs[3].1));
    let p2 = tv
        .storage
        .list_drafts(None, p1.next_before_sequence, Some(2))
        .unwrap();
    assert_eq!(
        p2.items.iter().map(|s| s.sequence).collect::<Vec<_>>(),
        vec![seqs[2].1, seqs[1].1]
    );
    assert_eq!(p2.next_before_sequence, Some(seqs[1].1));
    let p3 = tv
        .storage
        .list_drafts(None, p2.next_before_sequence, Some(2))
        .unwrap();
    assert_eq!(
        p3.items.iter().map(|s| s.sequence).collect::<Vec<_>>(),
        vec![seqs[0].1]
    );
    assert_eq!(p3.next_before_sequence, None);

    // note filter only matches that note's drafts
    let filtered = tv
        .storage
        .list_drafts(Some(&op_id()), None, Some(50))
        .unwrap();
    assert!(
        filtered.items.is_empty(),
        "unknown note filter = empty page, not error"
    );

    // limit must be validated and capped
    let err = tv.storage.list_drafts(None, None, Some(0)).unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);
    let capped = tv
        .storage
        .list_drafts(None, None, Some(usize::MAX))
        .unwrap();
    assert_eq!(
        capped.items.len(),
        5,
        "capped to max 200, still returns all rows"
    );
}

// ----------------------------------------------------------------------------
// 7. corrupted stored rows are refused typed on read AND list
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_metadata_tamper_refuses_read_and_list() {
    let variants = [
        "UPDATE draft_checkpoints SET request_digest = 'cd'",
        "UPDATE draft_checkpoints SET source_hash = 'ab'",
        "UPDATE draft_checkpoints SET relative_path = '../evil.md'",
        "UPDATE draft_checkpoints SET base_content_hash = 'zz'",
        "UPDATE draft_checkpoints SET saved_at = 'yesterday'",
        // positive but beyond the safe integer bound
        "UPDATE draft_checkpoints SET base_revision = 5000000000000000000",
        "UPDATE draft_checkpoints SET draft_id = 'not-a-uuid'",
    ];
    for sql in variants {
        let mut tv = TestVault::new();
        let (note_id, rev, hash) = seed_note(&mut tv.storage, "corrupt.md", "body");
        let res = tv
            .storage
            .write_checkpoint(checkpoint_request(
                &note_id,
                "corrupt.md",
                "draft text",
                rev,
                &hash,
            ))
            .unwrap_or_else(|e| panic!("RED setup: write failed {e:?}"));
        tv.db()
            .execute(sql, [])
            .unwrap_or_else(|e| panic!("RED setup tamper failed: {e:?}"));

        // metadata tamper must refuse the whole page: metadata-only listing
        // refuses too, it never trusts the row it cannot fully verify
        let list_err = tv.storage.list_drafts(None, None, Some(50)).unwrap_err();
        assert_drafts(&list_err, ErrorCode::IoDenied);

        // body read refuses as well (except draft_id tamper, where the row has
        // no known id anymore — covered by the list refusal above)
        if !sql.contains("draft_id") {
            let err = tv.storage.read_draft(&res.draft_id).unwrap_err();
            assert_drafts(&err, ErrorCode::IoDenied);
        }
        let db = tv.db();
        assert_eq!(
            draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
            1
        );
    }
}

/// Baseline metadata tampering with a VALID hash must also be bound by the
/// stored digest (same-id replay with a changed baseContentHash is a typed
/// mismatch, not a silent replay).
#[test]
fn checkpoint_base_hash_only_mismatch_typed_and_tamper_detected() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "baseline.md", "body");
    let req = checkpoint_request(&note_id, "baseline.md", "payload", rev, &hash);
    tv.storage.write_checkpoint(req.clone()).unwrap();

    // identical request except baseContentHash (still 64 lowercase hex)
    let mut other = req.clone();
    other.base_content_hash = sha256_hex("different baseline");
    let err = tv.storage.write_checkpoint(other).unwrap_err();
    assert_drafts(&err, ErrorCode::IdempotencyMismatch);

    // stored baseline hash tampered to another VALID hex: the metadata digest
    // binds both content hashes, so exposure must refuse
    let res = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "baseline.md",
            "payload2",
            rev,
            &hash,
        ))
        .unwrap();
    let tampered = sha256_hex("tampered baseline");
    tv.db()
        .execute(
            "UPDATE draft_checkpoints SET base_content_hash = ?1",
            [tampered],
        )
        .unwrap();
    let err = tv.storage.read_draft(&res.draft_id).unwrap_err();
    assert_drafts(&err, ErrorCode::IoDenied);
    let err = tv.storage.list_drafts(None, None, Some(50)).unwrap_err();
    assert_drafts(&err, ErrorCode::IoDenied);
}

/// Body-only tampering cannot be detected by the metadata-only listing (the
/// list MUST still succeed with intact metadata); the full read verifies the
/// stored source bytes against source_hash and refuses.
#[test]
fn checkpoint_body_tamper_list_metadata_ok_read_refuses() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "bodyonly.md", "body");
    let res = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "bodyonly.md",
            "original draft",
            rev,
            &hash,
        ))
        .unwrap_or_else(|e| panic!("RED setup: write failed {e:?}"));

    tv.db()
        .execute("UPDATE draft_checkpoints SET source = 'tampered body'", [])
        .unwrap();

    // metadata-only list canNOT inspect the body: it must not fake an empty
    // refusal for an unverifiable body; metadata passes and the page serves
    let page = tv.storage.list_drafts(None, None, Some(50)).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].source_hash, sha256_hex("original draft"));

    // the full read verifies the stored source and refuses typed
    let err = tv.storage.read_draft(&res.draft_id).unwrap_err();
    assert_drafts(&err, ErrorCode::IoDenied);

    // page cursors still stable after the refusal: the single-row page has no
    // cursor; paging before its own sequence is empty
    assert_eq!(page.next_before_sequence, None);
    let p2 = tv
        .storage
        .list_drafts(None, Some(page.items[0].sequence), Some(50))
        .unwrap();
    assert!(p2.items.is_empty());
}

/// Caller path must match the authoritative mapping EXACTLY (no silent
/// backslash canonicalization); revisions and cursor values must stay within
/// the IPC-safe 2^53-1 bound.
#[test]
fn checkpoint_exact_canonical_path_and_numeric_bounds() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "dir/shape.md", "body");

    // backslash spelling of the same mapped path: NOT a match
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "dir\\shape.md",
            "p",
            rev,
            &hash,
        ))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // boundary: MAX_SAFE_INTEGER accepted
    let ck = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "dir/shape.md",
            "p",
            9_007_199_254_740_991,
            &hash,
        ))
        .unwrap_or_else(|e| panic!("RED: safe boundary rejected {e:?}"));
    assert_eq!(ck.sequence, 1);

    // beyond the safe integer bound: typed rejection, no row
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "dir/shape.md",
            "p",
            9_007_199_254_740_992,
            &hash,
        ))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    // same bound applies to the pagination cursor
    let err = tv
        .storage
        .list_drafts(None, Some(9_007_199_254_740_992), Some(50))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        1
    );
}

/// A held writer gate (another instance/process) surfaces the existing
/// retryable FILE_LOCKED; releasing it lets the SAME connection succeed.
#[test]
fn checkpoint_file_locked_retryable_then_same_connection_succeeds() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "lock.md", "body");

    let lock_path = tv.path().join(".opennote").join("writer.lock");
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    holder.try_lock().expect("holder acquires OS lock");

    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(&note_id, "lock.md", "p", rev, &hash))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::FileLocked);
    assert!(err.retryable, "FILE_LOCKED must be retryable");
    assert_eq!(
        {
            let db = tv.db();
            draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints")
        },
        0
    );

    drop(holder);
    let ck = tv
        .storage
        .write_checkpoint(checkpoint_request(&note_id, "lock.md", "p", rev, &hash))
        .unwrap_or_else(|e| panic!("RED: same-connection retry after release failed {e:?}"));
    assert_eq!(ck.sequence, 1);
}

// ----------------------------------------------------------------------------
// 8. deterministic WriteFailure failpoint: typed failure, transaction rolled
//    back, retry with same id/payload succeeds afterwards
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_write_failure_rolls_back_and_retry_replays() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "rollback.md", "body");

    tv.storage.push_failpoint(Failpoint::WriteFailure);
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "rollback.md",
            "unsaved draft",
            rev,
            &hash,
        ))
        .unwrap_err();
    assert!(
        matches!(
            err.code,
            ErrorCode::DiskFull | ErrorCode::IoDenied | ErrorCode::FileLocked
        ),
        "expected injected typed IO failure, got {:?}",
        err.code
    );

    // rolled back: no partial row
    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        0
    );
    drop(db);

    // same request retained: retry with the SAME operation id keeps failing
    // while the failpoint is queued (deterministic), original id/payload intact
    let req = checkpoint_request(&note_id, "rollback.md", "unsaved draft", rev, &hash);
    tv.storage.push_failpoint(Failpoint::WriteFailure);
    let err2 = tv.storage.write_checkpoint(req.clone()).unwrap_err();
    assert!(matches!(
        err2.code,
        ErrorCode::DiskFull | ErrorCode::IoDenied | ErrorCode::FileLocked
    ));
    drop(tv.db());

    // without the failpoint: the retained original request eventually
    // succeeds once and replays identically forever after
    let ok = tv.storage.write_checkpoint(req.clone()).unwrap();
    let again = tv.storage.write_checkpoint(req.clone()).unwrap();
    assert_eq!(again, ok);
    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        1
    );
}

// ----------------------------------------------------------------------------
// 9. additive migration: opening adds draft_checkpoints, keeps notes/operations
//    rows and keeps both still writable afterwards
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_migration_is_additive_preserve_all_rows() {
    let mut tv = TestVault::new();
    let (note_id, rev, _hash) = seed_note(&mut tv.storage, "m.md", "v1");
    let save = tv
        .storage
        .save(note_id.as_str(), "v2", rev, &op_id())
        .unwrap_or_else(|e| panic!("RED setup: save failed {e:?}"));
    let op1 = op_id();
    let _ = tv.storage.save(note_id.as_str(), "v2", rev, &op1).err(); // replay of the same request? distinct id same payload IS distinct op

    // findings/counters captured after reopen (which runs the new migration)
    let reopened = VaultStorage::open(tv.path())
        .unwrap_or_else(|e| panic!("RED reopen after migration: {e:?}"));
    let db = tv.db();
    let notes: i64 = db
        .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(notes, 1, "notes row preserved");
    let local_rev: i64 = db
        .query_row(
            "SELECT local_revision FROM notes WHERE relative_path='m.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        local_rev as u64, save.local_revision,
        "note revision preserved"
    );
    let ops: i64 = db
        .query_row(
            "SELECT COUNT(DISTINCT operation_id) FROM operations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ops >= 2, "both operation rows preserved, got {ops}");
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        0
    );

    // both surfaces still writable after the migration
    let ck = reopened
        .write_checkpoint(checkpoint_request(
            &note_id,
            "m.md",
            "after migration",
            save.local_revision,
            &save.content_hash,
        ))
        .unwrap_or_else(|e| panic!("RED: write after migration failed {e:?}"));
    let doc = reopened
        .read_draft(&ck.draft_id)
        .unwrap_or_else(|e| panic!("RED: read after migration failed {e:?}"));
    assert_eq!(doc.source, "after migration");
}

// ----------------------------------------------------------------------------
// 10. newer schema: no conn/writable, all three surfaces refuse typed with
//     zero mutation of sentinel state bytes
// ----------------------------------------------------------------------------

#[test]
fn checkpoint_newer_schema_no_mutation_typed() {
    let dir = TempDir::new().unwrap();
    let note = dir.path().join("note.md");
    fs::write(&note, b"hello").unwrap();
    let app = dir.path().join(".opennote");
    fs::create_dir_all(&app).unwrap();
    let vault_json = format!(
        "{{\"vaultId\":\"{}\",\"schemaVersion\":999,\"displayName\":\"Future\",\"syncEnabled\":false,\"writable\":true}}",
        Uuid::new_v4()
    );
    fs::write(app.join("vault.json"), vault_json.as_bytes()).unwrap();
    let sentinel = b"sentinel state bytes for draft zero mutation";
    fs::write(app.join("state.sqlite"), sentinel).unwrap();

    let storage = VaultStorage::open(dir.path()).unwrap();
    assert!(!storage.vault().writable);

    let list_id = op_id();
    let err = storage
        .write_checkpoint(checkpoint_request(
            &list_id,
            "note.md",
            "draft",
            0,
            &sha256_hex("hello"),
        ))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    let err = storage.list_drafts(None, None, Some(50)).unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    let err = storage.read_draft(&op_id()).unwrap_err();
    assert_drafts(&err, ErrorCode::InvalidInput);

    assert_eq!(fs::read(app.join("state.sqlite")).unwrap(), sentinel);
    assert_eq!(fs::read(&note).unwrap(), b"hello");
}

// ----------------------------------------------------------------------------
// 11. replay must verify the STORED body, not only the metadata digest;
//     an externally corrupted source column refuses typed and stays untouched
// ----------------------------------------------------------------------------

/// Same operationId + same raw request replay must never ACK a receipt whose
/// stored source bytes no longer hash to the stored (digest-bound) source_hash.
#[test]
fn checkpoint_replay_refuses_externally_corrupted_stored_source() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "replay.md", "body");
    let req = checkpoint_request(&note_id, "replay.md", "replay payload", rev, &hash);
    let first = tv
        .storage
        .write_checkpoint(req.clone())
        .unwrap_or_else(|e| panic!("RED setup: write failed {e:?}"));

    // mutate ONLY the stored source column through an independent test DB
    let tampered = "externally corrupted bytes";
    let _ = sha256_hex(tampered); // metadata (incl. source_hash) is left intact
    tv.db()
        .execute(
            "UPDATE draft_checkpoints SET source = ?1",
            rusqlite::params![tampered],
        )
        .unwrap_or_else(|e| panic!("RED setup: tamper failed {e:?}"));

    // replay of the SAME raw request/opId: typed refusal, never a receipt
    let err = tv.storage.write_checkpoint(req.clone()).unwrap_err();
    assert_drafts(&err, ErrorCode::IoDenied);

    // corrupted row remains untouched: exactly one row, tampered bytes still
    // stored (no silent repair/rewrite), metadata still digest-consistent
    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        1
    );
    let stored: String = db
        .query_row(
            "SELECT source FROM draft_checkpoints WHERE draft_id = ?1",
            [&first.draft_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, tampered);
    drop(db);

    // lists stay metadata-only: intact metadata must still serve the page
    let page = tv
        .storage
        .list_drafts(Some(&note_id), None, Some(50))
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].source_hash, sha256_hex("replay payload"));

    // full read refuses too (unchanged contract)
    let err = tv.storage.read_draft(&first.draft_id).unwrap_err();
    assert_drafts(&err, ErrorCode::IoDenied);
}

// ----------------------------------------------------------------------------
// 12. a DB-assigned AUTOINCREMENT sequence beyond MAX_SAFE_INTEGER must be
//     refused BEFORE commit (RAII rollback); never clamped/renumbered/ACKed
// ----------------------------------------------------------------------------

/// An externally corrupted sqlite_sequence pushing the next rowid beyond the
/// IPC-safe 2^53-1 bound must fail typed with the previous rows unchanged
/// (also: no sequence beyond the bound in any result) and — after the test
/// fixture sequence is restored — the same connection keeps working.
#[test]
fn checkpoint_sequence_above_safe_bound_typed_fail_before_commit() {
    let mut tv = TestVault::new();
    let (note_id, rev, hash) = seed_note(&mut tv.storage, "seq.md", "body");
    let _first = tv
        .storage
        .write_checkpoint(checkpoint_request(&note_id, "seq.md", "seed", rev, &hash))
        .unwrap_or_else(|e| panic!("RED setup: write failed {e:?}"));

    // externally push the AUTOINCREMENT counter to the exact safe boundary
    {
        let db = tv.db();
        db.execute(
            "UPDATE sqlite_sequence SET seq = 9007199254740991 WHERE name = 'draft_checkpoints'",
            [],
        )
        .unwrap_or_else(|e| panic!("RED setup: sequence tamper failed {e:?}"));
        let seq: i64 = db
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'draft_checkpoints'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seq, 9_007_199_254_740_991);
    }

    // next write needs rowid 9_007_199_254_740_992: typed refusal, and the
    // over-bound row must never be committed or ACKed
    let err = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "seq.md",
            "over the edge",
            rev,
            &hash,
        ))
        .unwrap_err();
    assert_drafts(&err, ErrorCode::IoDenied);

    // row count and previous rows unchanged; sequence column of the fixture
    // row untouched (no clamp/delete/renumber)
    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        1
    );
    let (seed_seq, seed_src): (i64, String) = db
        .query_row(
            "SELECT sequence, source FROM draft_checkpoints WHERE draft_id IS NOT NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(seed_seq, 1);
    assert_eq!(seed_src, "seed");
    let seq_after_fail: i64 = db
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'draft_checkpoints'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    assert_eq!(
        seq_after_fail, 9_007_199_254_740_991,
        "fixture sequence must stay the tampered fixture (refused bump rolled back), not the refused rowid"
    );

    // restore ONLY the test fixture sequence; the refuse-enabling tamper stays
    // a local test artifact, then the SAME connection is used again
    {
        let db = tv.db();
        db.execute(
            "UPDATE sqlite_sequence SET seq = 1 WHERE name = 'draft_checkpoints'",
            [],
        )
        .unwrap();
    }
    let ok = tv
        .storage
        .write_checkpoint(checkpoint_request(
            &note_id,
            "seq.md",
            "after restore",
            rev,
            &hash,
        ))
        .unwrap_or_else(|e| panic!("RED: connection unusable after refusal {e:?}"));
    assert_eq!(ok.sequence, 2);
    assert!(ok.sequence as u64 <= 9_007_199_254_740_991);
    let db = tv.db();
    assert_eq!(
        draft_count(&db, "SELECT COUNT(*) FROM draft_checkpoints"),
        2
    );
    let seed_again: String = db
        .query_row(
            "SELECT source FROM draft_checkpoints WHERE sequence = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(seed_again, "seed");
}
