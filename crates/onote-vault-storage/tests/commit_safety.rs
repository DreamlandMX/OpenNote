use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use uuid::Uuid;

use onote_domain::{ErrorCode, NoteDocument, ServiceError};
use onote_vault_storage::{Failpoint, VaultStorage};

// ----------------------------------------------------------------------------
// focused commit-safety regressions (aux ownership, guarded finalize,
// committed replay, target races, tamper-proof journal cleanup)
// ----------------------------------------------------------------------------

fn op_id() -> String {
    Uuid::new_v4().to_string()
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Crash-injection surfaces a real typed IO failure, never success.
fn assert_injected_io_error(code: &str, err: &ServiceError) {
    assert!(
        matches!(
            err.code,
            ErrorCode::DiskFull | ErrorCode::IoDenied | ErrorCode::FileLocked
        ),
        "{code}: expected typed injected IO error, got {:?} ({:?})",
        err.code,
        err.message_key
    );
}

fn find_stage(dir: &Path, note_relative: &str) -> Option<PathBuf> {
    let mut found = None;
    for entry in fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{note_relative}.onote-stage-")) {
            found = Some(entry.path());
        }
    }
    found
}

fn open_state(dir: &TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join(".opennote").join("state.sqlite")).unwrap()
}

// ----------------------------------------------------------------------------
// 1. preexisting aux sentinel/link is never truncated, followed or replaced
// ----------------------------------------------------------------------------

/// A preexisting plain file (sentinel) at the aux stage name is refused
/// without truncation and without touching the target or the journal.
#[test]
fn preexisting_stage_sentinel_never_truncated() {
    let dir = TempDir::new().unwrap();
    let op = op_id();
    let stage_name = format!("s.md.onote-stage-{op}");
    fs::write(dir.path().join(&stage_name), b"sentinel-stage").unwrap();

    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let err = storage.create_note("s.md", "body", &op).unwrap_err();
    assert!(
        matches!(err.code, ErrorCode::IoDenied | ErrorCode::DiskFull),
        "{:?} {:?}",
        err.code,
        err.message_key
    );

    assert_eq!(
        read_bytes(&dir.path().join(&stage_name)),
        b"sentinel-stage",
        "preexisting sentinel at the stage name must never be truncated"
    );
    assert!(
        !dir.path().join("s.md").exists(),
        "note target must not be created from a refused staging path"
    );
    let count: i64 = open_state(&dir)
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE note_relative = 's.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "no journal row may be written from foreign staging"
    );
}

/// A reparse point (junction) at the aux stage name is refused without
/// following it; the outside sentinel stays intact, the link stays put.
#[cfg(windows)]
#[test]
fn preexisting_stage_junction_never_followed() {
    let outside = TempDir::new().unwrap();
    let sub = outside.path().join("link-target");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("outside-secret.md"), b"outside secret").unwrap();

    let dir = TempDir::new().unwrap();
    let op = op_id();
    let stage_name = format!("j.md.onote-stage-{op}");
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(dir.path().join(&stage_name))
        .arg(&sub)
        .status()
        .unwrap();
    assert!(status.success(), "stage junction creation failed");

    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let err = storage.create_note("j.md", "body", &op).unwrap_err();
    assert_eq!(err.code, ErrorCode::IoDenied, "{:?}", err.message_key);

    assert_eq!(
        read_bytes(&sub.join("outside-secret.md")),
        b"outside secret",
        "junction target must never be written through"
    );
    assert!(
        dir.path().join(&stage_name).is_dir(),
        "the junction itself must not be deleted or replaced by the refusal"
    );
    assert!(!dir.path().join("j.md").exists());
}

/// A preexisting plain sentinel at the old-snapshot name is refused without
/// truncation; the save target and the journal staging stay untouched.
#[test]
fn preexisting_old_backup_sentinel_never_truncated() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("w.md", "v0", &op_id()).unwrap();
    let baseline = read_bytes(&dir.path().join("w.md"));

    let op = op_id();
    let old_name = format!("w.md.onote-old-{op}");
    fs::write(dir.path().join(&old_name), b"sentinel-old").unwrap();

    let err = storage
        .save(note.note_id.as_str(), "v1", note.local_revision, &op)
        .unwrap_err();
    assert!(
        matches!(err.code, ErrorCode::IoDenied | ErrorCode::DiskFull),
        "old-snapshot sentinel must be refused, got {:?} {:?}",
        err.code,
        err.message_key
    );
    assert_eq!(
        read_bytes(&dir.path().join(&old_name)),
        b"sentinel-old",
        "preexisting sentinel at the old name must never be truncated"
    );
    assert_eq!(
        read_bytes(&dir.path().join("w.md")),
        baseline,
        "save target must stay untouched through the refusal"
    );
    let count: i64 = open_state(&dir)
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE note_relative = 'w.md' AND status = 'prepared'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "no prepared row may record a stolen old slot");
}

// ----------------------------------------------------------------------------
// 2. prepared same-op retry after replacement: one exact revision, the
//    original backup is never rewritten with the new bytes
// ----------------------------------------------------------------------------

#[test]
fn prepared_retry_after_replace_one_revision_and_original_backup() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("r.md", "v0", &op_id()).unwrap();
    let baseline_hash = note.content_hash.clone();
    let baseline_bytes = read_bytes(&dir.path().join("r.md"));

    let v1 = note.source.replace("v0", "v1");
    let op1 = op_id();
    let backup = dir.path().join(format!("r.md.onote-old-{op1}"));

    // first attempt: crash after replace (target has the new bytes, DB not yet)
    storage.push_failpoint(Failpoint::AfterReplace);
    let err = storage
        .save(note.note_id.as_str(), &v1, note.local_revision, &op1)
        .unwrap_err();
    assert_injected_io_error("AfterReplace", &err);
    assert_eq!(read_bytes(&dir.path().join("r.md")), v1.as_bytes());
    assert!(backup.exists(), "crash keeps the prepared backup");
    assert_eq!(
        sha256_hex(&String::from_utf8(read_bytes(&backup)).unwrap()),
        baseline_hash,
        "the first attempt's backup holds the ORIGINAL bytes"
    );

    // resume the same prepared operation with an injected crash before the
    // DB finalize: nothing may be rewritten, the backup must stay original.
    storage.push_failpoint(Failpoint::BeforeDbCommit);
    let err = storage
        .save(note.note_id.as_str(), &v1, note.local_revision, &op1)
        .unwrap_err();
    assert_injected_io_error("resume BeforeDbCommit", &err);
    assert_eq!(
        read_bytes(&backup),
        baseline_bytes,
        "resume must never rewrite the old backup with new bytes"
    );
    assert_eq!(
        read_bytes(&dir.path().join("r.md")),
        v1.as_bytes(),
        "replaced target stays intact through the resume crash"
    );

    let (rev, hash): (i64, String) = open_state(&dir)
        .query_row(
            "SELECT local_revision, content_hash FROM notes WHERE relative_path = 'r.md'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rev, 0, "no double-apply before the finalize");
    assert_eq!(hash, baseline_hash);
    let status: String = open_state(&dir)
        .query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [op1.clone()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "prepared", "the slot must stay pending on crash");

    // clean resume of the same operation: exactly one committed revision
    let retried = storage
        .save(note.note_id.as_str(), &v1, note.local_revision, &op1)
        .unwrap_or_else(|e| panic!("clean resume failed {e:?}"));
    assert_eq!(retried.local_revision, note.local_revision + 1);
    assert_eq!(retried.content_hash, sha256_hex(&v1));
    assert!(
        !backup.exists(),
        "finalized slot cleans the original backup"
    );
    let ops_rows: i64 = open_state(&dir)
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE note_relative = 'r.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ops_rows, 2, "one create + one save, nothing applied twice");
    let (rev, hash): (i64, String) = open_state(&dir)
        .query_row(
            "SELECT local_revision, content_hash FROM notes WHERE relative_path = 'r.md'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rev, 1, "exactly one committed revision after all retries");
    assert_eq!(hash, sha256_hex(&v1));
}

// ----------------------------------------------------------------------------
// 3. committed replay returns the original result before any fresh disk read,
//    even when the note file was removed afterwards
// ----------------------------------------------------------------------------

#[test]
fn committed_replay_replays_original_result_after_file_removed() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("gone.md", "v0", &op_id()).unwrap();

    let op1 = op_id();
    let s1 = storage
        .save(note.note_id.as_str(), "v1", note.local_revision, &op1)
        .unwrap();
    let s2 = storage
        .save(note.note_id.as_str(), "v2", s1.local_revision, &op_id())
        .unwrap();
    assert_ne!(s2.local_revision, s1.local_revision);

    // the note file disappears entirely after the successful operations
    fs::remove_file(dir.path().join("gone.md")).unwrap();

    // the committed old request still replays the ORIGINAL result
    let retry = storage
        .save(note.note_id.as_str(), "v1", note.local_revision, &op1)
        .unwrap_or_else(|e| panic!("committed replay must not fail on removed file {e:?}"));
    assert_eq!(retry, s1, "same op must replay the original revision/hash");
    drop(storage);

    // unchanged after reopen, against the removed file
    let mut reopened = VaultStorage::open(dir.path()).unwrap();
    let retry2 = reopened
        .save(note.note_id.as_str(), "v1", note.local_revision, &op1)
        .unwrap_or_else(|e| panic!("reopened replay failed {e:?}"));
    assert_eq!(retry2, s1);

    // a changed payload under the same operation id still mismatches
    let err = reopened
        .save(note.note_id.as_str(), "OTHER", note.local_revision, &op1)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::IdempotencyMismatch);
}

// ----------------------------------------------------------------------------
// 4. target appeared/changed between stage and replace (deterministic
//    failpoint + external mutation, safe refusal, later completion)
// ----------------------------------------------------------------------------

#[test]
fn target_changed_between_stage_and_replace_save_refuses_and_preserves() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("t.md", "v0", &op_id()).unwrap();
    let baseline = read_bytes(&dir.path().join("t.md"));

    let v1 = note.source.replace("v0", "v1");
    let op1 = op_id();
    let stage = dir.path().join(format!("t.md.onote-stage-{op1}"));
    let backup = dir.path().join(format!("t.md.onote-old-{op1}"));

    storage.push_failpoint(Failpoint::BeforeReplace);
    let err = storage
        .save(note.note_id.as_str(), &v1, note.local_revision, &op1)
        .unwrap_err();
    assert_injected_io_error("BeforeReplace", &err);
    assert!(stage.exists(), "stage retained at the interrupt");
    assert!(backup.exists(), "backup retained at the interrupt");
    assert_eq!(read_bytes(&backup), baseline, "backup holds original bytes");

    // external change lands between "stage" and "replace"
    fs::write(dir.path().join("t.md"), b"external-writer").unwrap();

    // the retry must refuse typed, preserve everything, never replace
    let err = storage
        .save(note.note_id.as_str(), &v1, note.local_revision, &op1)
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::VersionConflict,
        "{:?} {:?}",
        err.code,
        err.message_key
    );
    assert_eq!(
        read_bytes(&dir.path().join("t.md")),
        b"external-writer",
        "the external bytes were never replaced by the retry"
    );
    assert!(stage.exists(), "stage preserved on refusal");
    assert_eq!(read_bytes(&stage), v1.as_bytes(), "stage bytes preserved");
    assert!(backup.exists(), "backup preserved on refusal");
    assert_eq!(read_bytes(&backup), baseline, "backup untouched by refusal");

    let status: String = open_state(&dir)
        .query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [op1.clone()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "prepared", "journal stays pending after refusal");

    // once the external writer reverts to the baseline bytes, the same
    // prepared operation completes safely (validated stage+backup reuse)
    fs::write(dir.path().join("t.md"), &baseline).unwrap();
    let saved = storage
        .save(note.note_id.as_str(), &v1, note.local_revision, &op1)
        .unwrap_or_else(|e| panic!("retry after external revert failed {e:?}"));
    assert_eq!(saved.local_revision, note.local_revision + 1);
    assert_eq!(read_bytes(&dir.path().join("t.md")), v1.as_bytes());
    assert!(!stage.exists() && !backup.exists());
}

#[test]
fn target_appeared_between_stage_and_publish_create_refuses_and_preserves() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();

    let op = op_id();
    let source = "# fresh created";
    storage.push_failpoint(Failpoint::BeforeReplace);
    let err = storage.create_note("c4.md", source, &op).unwrap_err();
    assert_injected_io_error("create BeforeReplace", &err);
    let stage = find_stage(dir.path(), "c4.md").expect("stage retained at the interrupt");

    // a concurrent destination appears after staging
    fs::write(dir.path().join("c4.md"), b"concurrent creator").unwrap();

    let err = storage.create_note("c4.md", source, &op).unwrap_err();
    assert_eq!(err.code, ErrorCode::PathCollision, "{:?}", err.message_key);
    assert_eq!(
        read_bytes(&dir.path().join("c4.md")),
        b"concurrent creator",
        "the concurrently appeared destination was never replaced"
    );
    assert!(stage.exists(), "stage preserved on refusal");
    let status: String = open_state(&dir)
        .query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [op.clone()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "prepared");

    // once the intruder is gone the same operation completes
    fs::remove_file(dir.path().join("c4.md")).unwrap();
    let note = storage
        .create_note("c4.md", source, &op)
        .unwrap_or_else(|e| panic!("create after intruder removed failed {e:?}"));
    assert!(note.source.contains(source));
    assert_eq!(
        read_bytes(&dir.path().join("c4.md")),
        note.source.as_bytes()
    );
    assert!(
        find_stage(dir.path(), "c4.md").is_none(),
        "publish leaves no stage link behind"
    );
}

// ----------------------------------------------------------------------------
// 5. tampered journal rows: traversal/ownership tampering must refuse
//    recovery and cleanup without touching any file by the attacker names
//    and without regressing the authoritative revision
// ----------------------------------------------------------------------------

fn sql_tamper(db_path: &Path, rowid: i64, sql_set: &str) {
    rusqlite::Connection::open(db_path)
        .unwrap()
        .execute_batch(&format!(
            "UPDATE operations SET {sql_set} WHERE rowid = {rowid};"
        ))
        .unwrap();
}

fn sql_restore(db_path: &Path, rowid: i64, original: &str) {
    rusqlite::Connection::open(db_path)
        .unwrap()
        .execute_batch(&format!(
            "UPDATE operations SET {original} WHERE rowid = {rowid};"
        ))
        .unwrap();
}

fn assert_tamper_refused(
    dir: &TempDir,
    outside: &TempDir,
    note: &NoteDocument,
    baseline: &[u8],
    baseline_hash: &str,
    stage_path: &Path,
    backup_path: &Path,
    stage_bytes: &[u8],
    backup_bytes: &[u8],
    label: &str,
) {
    let reopened = VaultStorage::open(dir.path()).unwrap();
    let outcome = reopened.recover().unwrap();
    assert!(
        outcome.applied.iter().all(|i| *i != note.note_id),
        "{label}: tampered row must never be applied ({outcome:?})"
    );
    assert!(
        outcome.applied.is_empty() && outcome.pending.len() == 1,
        "{label}: exactly the tampered row stays pending, nothing applied ({outcome:?})"
    );

    assert_eq!(
        read_bytes(stage_path),
        stage_bytes,
        "{label}: stage file preserved under tampering"
    );
    assert_eq!(
        read_bytes(backup_path),
        backup_bytes,
        "{label}: backup file preserved under tampering"
    );
    assert_eq!(
        read_bytes(&dir.path().join("sentinel.dat")),
        b"inside-sentinel",
        "{label}: in-root sentinel untouched"
    );
    assert_eq!(
        read_bytes(&outside.path().join("outside-evidence.md")),
        b"outside-sentinel",
        "{label}: outside sentinel untouched"
    );
    assert_eq!(
        read_bytes(&dir.path().join("n.md")),
        baseline,
        "{label}: note bytes untouched during refusal"
    );
    let (rev, hash): (i64, String) = open_state(dir)
        .query_row(
            "SELECT local_revision, content_hash FROM notes WHERE relative_path = 'n.md'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (rev, hash.as_str()),
        (0, baseline_hash),
        "{label}: revision must never regress during tamper refusal"
    );
}

#[test]
fn tampered_journal_row_preserved_and_cleanups_refused() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("n.md", "v0", &op_id()).unwrap();
    let baseline = read_bytes(&dir.path().join("n.md"));
    let baseline_hash = note.content_hash.clone();

    // a legitimately prepared save (never replaced: AfterPrepared crash)
    let draft = note.source.replace("v0", "v1");
    let op1 = op_id();
    storage.push_failpoint(Failpoint::AfterPrepared);
    let err = storage
        .save(note.note_id.as_str(), &draft, note.local_revision, &op1)
        .unwrap_err();
    assert_injected_io_error("setup prepared row", &err);
    drop(storage);

    let db_path = dir.path().join(".opennote").join("state.sqlite");
    let stage_path = dir.path().join(format!("n.md.onote-stage-{op1}"));
    let backup_path = dir.path().join(format!("n.md.onote-old-{op1}"));
    assert!(stage_path.exists() && backup_path.exists());
    let stage_bytes = read_bytes(&stage_path);
    let backup_bytes = read_bytes(&backup_path);

    // an unrelated in-root sentinel (the natural deletion target of an
    // unvalidated stage_relative) and an outside traversal sentinel
    fs::write(dir.path().join("sentinel.dat"), b"inside-sentinel").unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(
        outside.path().join("outside-evidence.md"),
        b"outside-sentinel",
    )
    .unwrap();

    // rowid is the only stable tamper key
    let rowid: i64 = open_state(&dir)
        .query_row(
            "SELECT rowid FROM operations WHERE operation_id = ?1",
            [op1.clone()],
            |r| r.get(0),
        )
        .unwrap();
    let original = format!(
        "kind = '{k}', note_id = '{nid}', note_relative = 'n.md', old_revision = 0, \
         new_revision = 1, old_hash = '{oh}', new_hash = '{nh}', \
         stage_relative = 'n.md.onote-stage-{op1}', old_relative = 'n.md.onote-old-{op1}'",
        k = "save",
        nid = note.note_id,
        oh = baseline_hash,
        nh = sha256_hex(&draft),
    );
    let assert_case = |label: &str| {
        assert_tamper_refused(
            &dir,
            &outside,
            &note,
            &baseline,
            &baseline_hash,
            &stage_path,
            &backup_path,
            &stage_bytes,
            &backup_bytes,
            label,
        );
    };

    // traversal in the note path (both slash forms) and aux-path traversal
    for traversal in ["../outside-evidence.md", "..\\outside-evidence.md"] {
        sql_tamper(&db_path, rowid, &format!("note_relative = '{traversal}'"));
        assert_case(&format!("note_relative={traversal}"));
    }
    sql_tamper(
        &db_path,
        rowid,
        "stage_relative = '..\\outside-evidence.md'",
    );
    assert_case("stage_relative traversal");
    sql_tamper(&db_path, rowid, "old_relative = '.\\outside-evidence.md'");
    assert_case("old_relative traversal");

    // aux names aimed at real app/inside files must not be followed
    sql_tamper(&db_path, rowid, "stage_relative = '.opennote/vault.json'");
    assert_case("stage_relative attacks vault.json");
    sql_tamper(&db_path, rowid, "stage_relative = 'sentinel.dat'");
    assert_case("stage_relative steals sentinel.dat");
    sql_tamper(&db_path, rowid, "old_relative = 'sentinel.dat'");
    assert_case("old_relative steals sentinel.dat");

    // kind / uuid / hash-syntax / revision-regression tampering
    sql_tamper(&db_path, rowid, "kind = 'delete'");
    assert_case("kind=delete");
    sql_tamper(&db_path, rowid, "note_id = 'not-a-uuid'");
    assert_case("note_id not a uuid");
    sql_tamper(&db_path, rowid, "new_hash = 'zz'");
    assert_case("new_hash not a hash");
    sql_tamper(&db_path, rowid, "kind = 'create', old_relative = NULL");
    assert_case("kind=create keeps save transition");
    sql_tamper(&db_path, rowid, "new_revision = 0");
    assert_case("save row regressed to revision 0");

    // restore the original row: the slot is a normal prepared row again
    sql_restore(&db_path, rowid, &original);
    {
        let reopened = VaultStorage::open(dir.path()).unwrap();
        let outcome = reopened.recover().unwrap();
        assert!(
            outcome.pending.iter().any(|i| *i == note.note_id),
            "restored row stays pending while the target was never replaced: {outcome:?}"
        );
        assert_eq!(read_bytes(&stage_path), stage_bytes);
        assert_eq!(read_bytes(&backup_path), backup_bytes);
    }

    // normal life continues: the same request resumes and commits rev1
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let saved = storage
        .save(note.note_id.as_str(), &draft, note.local_revision, &op1)
        .unwrap_or_else(|e| panic!("resume after restore failed {e:?}"));
    assert_eq!(saved.local_revision, 1);
    assert_eq!(saved.content_hash, sha256_hex(&draft));
    assert!(!stage_path.exists() && !backup_path.exists());
    let (rev2, hash2): (i64, String) = open_state(&dir)
        .query_row(
            "SELECT local_revision, content_hash FROM notes WHERE relative_path = 'n.md'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rev2, 1);
    assert_eq!(hash2, sha256_hex(&draft));
    assert_eq!(read_bytes(&dir.path().join("n.md")), draft.as_bytes());
}

// ----------------------------------------------------------------------------
// 6. cleanup is content-gated: a foreign sentinel that replaced a prepared
//    operation's old/stage aux before recovery is preserved, never deleted;
//    the identical-bytes fresh save branch never removes preexisting
//    nonce-named sentinels either
// ----------------------------------------------------------------------------

#[test]
fn recovery_preserves_sentinels_replacing_prepared_old_and_stage_aux() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("n.md", "v0", &op_id()).unwrap();

    // prepared save that already replaced the target (AfterReplace crash):
    // the DB baseline is untouched, the stage name was consumed by the
    // rename, the old backup exists on disk
    let draft = note.source.replace("v0", "v1");
    let op1 = op_id();
    storage.push_failpoint(Failpoint::AfterReplace);
    let err = storage
        .save(note.note_id.as_str(), &draft, note.local_revision, &op1)
        .unwrap_err();
    assert_injected_io_error("setup AfterReplace", &err);
    drop(storage);

    let stage_path = dir.path().join(format!("n.md.onote-stage-{op1}"));
    let backup_path = dir.path().join(format!("n.md.onote-old-{op1}"));
    assert!(!stage_path.exists(), "stage renamed onto the target");
    assert!(backup_path.exists(), "backup kept by the crash");

    // a foreign process replaces the structurally valid aux names with its
    // own sentinels just before recovery
    fs::write(&stage_path, b"foreign-stage-sentinel").unwrap();
    fs::write(&backup_path, b"foreign-backup-sentinel").unwrap();

    let reopened = VaultStorage::open(dir.path()).unwrap();
    let outcome = reopened.recover().unwrap();
    assert!(
        outcome.applied.iter().any(|i| *i == note.note_id) && outcome.pending.is_empty(),
        "the replaced save row must be applied ({outcome:?})"
    );
    assert_eq!(
        read_bytes(&stage_path),
        b"foreign-stage-sentinel",
        "sentinel at the stage name must survive the finalized cleanup"
    );
    assert_eq!(
        read_bytes(&backup_path),
        b"foreign-backup-sentinel",
        "sentinel at the old name must survive the finalized cleanup"
    );
    assert_eq!(read_bytes(&dir.path().join("n.md")), draft.as_bytes());
    let status: String = open_state(&dir)
        .query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [op1.clone()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "committed",
        "finalize happened while sentinels stayed"
    );
    let (rev, hash): (i64, String) = open_state(&dir)
        .query_row(
            "SELECT local_revision, content_hash FROM notes WHERE relative_path = 'n.md'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rev, 1);
    assert_eq!(hash, sha256_hex(&draft));
}

#[test]
fn identical_bytes_fresh_save_preserves_preexisting_nonce_sentinels() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("w.md", "v0", &op_id()).unwrap();
    let baseline = read_bytes(&dir.path().join("w.md"));

    // a fresh operation whose derived nonce names already carry foreign
    // sentinel bytes (identical-bytes save never writes staging, and the
    // stale same-content file of the same op would legitimately be deleted)
    let op = op_id();
    let stage_name = format!("w.md.onote-stage-{op}");
    let backup_name = format!("w.md.onote-old-{op}");
    fs::write(dir.path().join(&stage_name), b"sentinel-stage").unwrap();
    fs::write(dir.path().join(&backup_name), b"sentinel-old").unwrap();

    let saved = storage
        .save(
            note.note_id.as_str(),
            &note.source,
            note.local_revision,
            &op,
        )
        .unwrap_or_else(|e| panic!("identical-bytes save failed {e:?}"));
    assert_eq!(saved.local_revision, note.local_revision + 1);
    assert_eq!(
        saved.content_hash, note.content_hash,
        "identical bytes keep the exact content hash"
    );
    assert_eq!(
        read_bytes(&dir.path().join(&stage_name)),
        b"sentinel-stage",
        "preexisting stage sentinel must never be deleted by the cleanup"
    );
    assert_eq!(
        read_bytes(&dir.path().join(&backup_name)),
        b"sentinel-old",
        "preexisting old sentinel must never be deleted by the cleanup"
    );
    assert_eq!(
        read_bytes(&dir.path().join("w.md")),
        baseline,
        "identical-bytes save never rewrites the target bytes"
    );
    let status: String = open_state(&dir)
        .query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [op.clone()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "committed");
}

// ----------------------------------------------------------------------------
// 7. non-canonical UUID operation ids (urn:uuid:notation etc.) are refused
//    before ANY filesystem write, and journal rows carrying them are never
//    read, reused or deleted — no ADS/stream path is ever addressed
// ----------------------------------------------------------------------------

#[test]
fn noncanonical_uuid_operation_id_refused_before_any_filesystem_write() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();

    // the ADS base file a raw `urn:uuid:` operation id would have addressed
    let host = dir.path().join("s.md.onote-stage-urn");
    fs::write(&host, b"urn-host-sentinel").unwrap();

    let canonical = Uuid::new_v4().to_string();
    let candidates = [format!("urn:uuid:{canonical}"), format!("{{{canonical}}}")];
    for op_id in candidates.iter() {
        let err = storage.create_note("s.md", "body", op_id).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{op_id:?}");
        assert_eq!(err.message_key, "error.invalidOperationId", "{op_id:?}");
        assert!(
            !dir.path().join("s.md").exists(),
            "no note may be created through a non-canonical operation id"
        );
    }

    assert_eq!(
        read_bytes(&host),
        b"urn-host-sentinel",
        "preexisting sentinel at the ADS base name must stay untouched"
    );
    assert!(
        fs::metadata(
            dir.path()
                .join(format!("s.md.onote-stage-urn:uuid:{canonical}"))
        )
        .is_err()
            && fs::read(
                dir.path()
                    .join(format!("s.md.onote-stage-urn:uuid:{canonical}"))
            )
            .is_err(),
        "no ADS stream at the coloned name may have been created"
    );
    let count: i64 = open_state(&dir)
        .query_row("SELECT COUNT(*) FROM operations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "no journal row may reference a non-canonical id");
}

#[test]
fn seeded_noncanonical_operation_row_survives_recovery_untouched() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage.create_note("n.md", "v0", &op_id()).unwrap();

    // sentinel at the ADS base name of the would-be aux path
    let host = dir.path().join("s2.md.onote-stage-urn");
    fs::write(&host, b"urn-base-sentinel").unwrap();

    // a prior-era prepared row whose operationId was raw `urn:uuid:` and
    // whose aux names therefore interpolate into a Windows ADS path
    let urn_op = format!("urn:uuid:{}", Uuid::new_v4());
    let bogus_note_id = Uuid::new_v4().to_string();
    let digest = sha256_hex("seeded");
    rusqlite::Connection::open(dir.path().join(".opennote").join("state.sqlite"))
        .unwrap()
        .execute_batch(&format!(
            "INSERT INTO operations (operation_id, request_digest, note_id, status, kind, \
             note_relative, old_revision, new_revision, old_hash, new_hash, stage_relative, \
             old_relative, created_at) VALUES ('{urn_op}', '{digest}', '{bogus_note_id}', \
             'prepared', 'create', 's2.md', NULL, 0, NULL, '{}', \
             's2.md.onote-stage-urn:uuid:{}', NULL, '0');",
            sha256_hex("seeded-source"),
            &urn_op["urn:uuid:".len()..],
        ))
        .unwrap();

    drop(storage);
    let reopened = VaultStorage::open(dir.path()).unwrap();
    let outcome = reopened.recover().unwrap();
    assert!(
        outcome.applied.iter().all(|i| *i != bogus_note_id)
            && outcome.pending.iter().any(|i| *i == bogus_note_id),
        "the non-canonical row must stay pending without any disk action ({outcome:?})"
    );
    assert!(
        !dir.path().join("s2.md").exists(),
        "no artifact may be created for a non-canonical journal row"
    );
    assert_eq!(
        read_bytes(&host),
        b"urn-base-sentinel",
        "sentinel at the ADS base name must never be read, reused or deleted"
    );
    assert_eq!(read_bytes(&dir.path().join("n.md")), note.source.as_bytes());
    let status: String = open_state(&dir)
        .query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [urn_op],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "prepared", "seeded row stays pending verbatim");
}
