use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use uuid::Uuid;

use onote_domain::{ErrorCode, ServiceError};
use onote_vault_storage::{Failpoint, VaultStorage};

// ----------------------------------------------------------------------------
// helpers (mirrors the storage.rs style; self-contained per-test temp vaults)
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

/// Independent open of the on-disk state.sqlite for introspection.
struct Db {
    path: PathBuf,
}

impl Db {
    fn open(root: &Path) -> Db {
        Db {
            path: root.join(".opennote").join("state.sqlite"),
        }
    }

    fn count(&self, sql: &str, rel: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        conn.query_row(sql, [rel], |row| row.get(0)).unwrap()
    }

    fn notes_count(&self) -> i64 {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .unwrap()
    }

    fn notes_row(&self, relative: &str) -> Option<(i64, String, String)> {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        conn.query_row(
            "SELECT local_revision, content_hash, note_id FROM notes WHERE relative_path = ?1",
            [relative],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
    }

    fn prepared_count(&self, relative: &str) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM operations WHERE note_relative = ?1 AND status = 'prepared'",
            relative,
        )
    }

    fn op_status(&self, operation_id: &str) -> Option<String> {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        conn.query_row(
            "SELECT status FROM operations WHERE operation_id = ?1",
            [operation_id],
            |row| row.get(0),
        )
        .ok()
    }

    fn op_row(&self, operation_id: &str) -> (Option<i64>, Option<i64>, Option<String>, String) {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        conn.query_row(
            "SELECT old_revision, new_revision, old_hash, new_hash FROM operations WHERE operation_id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
    }
}

fn find_entry(dir: &Path, name_prefix: &str) -> Option<PathBuf> {
    let mut found = None;
    for entry in fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        if entry.file_name().to_string_lossy().starts_with(name_prefix) {
            found = Some(entry.path());
        }
    }
    found
}

// ----------------------------------------------------------------------------
// 1. exact revisions across crash / reopen / read / recover / reopen for both
//    post-replace failpoints: the pre-recover state must stay on the
//    authoritative prior DB baseline (revision, hash) with READONLY reads and
//    no provisional DB bump; only recover() advances it, exactly once.
// ----------------------------------------------------------------------------

#[test]
fn committed_by_recover_exact_revisions_after_replace_failpoints() {
    for failpoint in [Failpoint::AfterReplace, Failpoint::BeforeDbCommit] {
        let dir = TempDir::new().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
        let note = storage
            .create_note("cr.md", "# crash body v0", &op_id())
            .unwrap();
        assert_eq!(note.local_revision, 0);

        let v1 = note.source.replace("v0", "v1");
        let v1_bytes = v1.as_bytes().to_vec();
        let op_save = op_id();
        let old_backup = dir.path().join(format!("cr.md.onote-old-{op_save}"));
        let stage = dir.path().join(format!("cr.md.onote-stage-{op_save}"));

        storage.push_failpoint(failpoint);
        let err = storage
            .save(note.note_id.as_str(), &v1, note.local_revision, &op_save)
            .unwrap_err();
        assert_injected_io_error("replace-stage crash", &err);
        // replace already happened: full new bytes on disk
        assert_eq!(read_bytes(&dir.path().join("cr.md")), v1_bytes);

        // ---- reopen (no recover yet) --------------------------------------
        let mut reopened = VaultStorage::open(dir.path()).unwrap();
        let db = Db::open(dir.path());
        let summary = reopened
            .list()
            .unwrap()
            .into_iter()
            .find(|s| s.note_id == note.note_id)
            .unwrap();
        assert_eq!(
            summary.local_revision, 0,
            "provisional scan bump before recover must be suppressed"
        );
        assert_eq!(
            summary.content_hash, note.content_hash,
            "the authoritative prior DB baseline hash must be preserved"
        );
        assert_eq!(
            db.notes_row("cr.md").unwrap(),
            (0, note.content_hash.clone(), note.note_id.clone()),
            "DB baseline row must stay untouched by initialize"
        );
        assert_eq!(db.prepared_count("cr.md"), 1);
        assert!(
            old_backup.exists(),
            "old backup must be retained while pending"
        );
        assert!(!stage.exists(), "staged file was renamed over the target");

        // ---- read BEFORE recover: matching hash, READONLY, no DB bump -----
        let doc = reopened.read(note.note_id.as_str()).unwrap();
        assert_eq!(doc.source, v1, "read must return the actual disk bytes");
        assert_eq!(
            sha256_hex(&doc.source),
            doc.content_hash,
            "consistent read: contentHash matches returned source"
        );
        assert!(doc.read_only, "pending-path reads are READONLY");
        assert_eq!(doc.local_revision, 0, "no provisional revision bump");
        assert_eq!(
            db.notes_row("cr.md").unwrap(),
            (0, note.content_hash.clone(), note.note_id.clone()),
            "a read while pending must not write a DB revision bump"
        );

        // ---- recover: exactly one authoritative transition -----------------
        let outcome = reopened.recover().unwrap();
        assert!(
            outcome.applied.iter().any(|id| *id == note.note_id),
            "journal replay must finalize the replaced commit: {:?}",
            outcome
        );
        assert!(outcome.pending.is_empty());
        assert_eq!(
            db.notes_row("cr.md").unwrap(),
            (1, sha256_hex(&v1), note.note_id.clone()),
            "recovery finalizes the exact journal revision, once"
        );
        let doc2 = reopened.read(note.note_id.as_str()).unwrap();
        assert_eq!(doc2.local_revision, 1);
        assert_eq!(doc2.source, v1);
        assert!(!doc2.read_only, "recovered note is writable again");
        assert_eq!(
            reopened
                .list()
                .unwrap()
                .into_iter()
                .find(|s| s.note_id == note.note_id)
                .unwrap()
                .local_revision,
            1
        );
        assert!(!old_backup.exists(), "applied slots clean the old backup");

        // ---- normal lifecycle resumes at the recovered revision ------------
        let v2 = v1.replace("v1", "v2");
        let saved = reopened
            .save(note.note_id.as_str(), &v2, doc2.local_revision, &op_id())
            .unwrap();
        assert_eq!(saved.local_revision, 2);
        assert_eq!(db.prepared_count("cr.md"), 0);

        // ---- reopen again: exact stable state ------------------------------
        let final_storage = VaultStorage::open(dir.path()).unwrap();
        let final_doc = final_storage.read(note.note_id.as_str()).unwrap();
        assert_eq!(final_doc.local_revision, 2);
        assert!(!final_doc.read_only);
        assert_eq!(final_doc.source, v2);
        assert_eq!(sha256_hex(&final_doc.source), final_doc.content_hash);
        assert_eq!(
            db.notes_row("cr.md").unwrap(),
            (2, sha256_hex(&v2), note.note_id.clone()),
        );
        assert_eq!(db.prepared_count("cr.md"), 0);
    }
}

// ----------------------------------------------------------------------------
// 2. stale pending draft bytes reappearing after a newer successful commit:
//    recovery must stay pending, never roll back or silently consume the
//    newer authoritative revision, and keep journal + stage + old backup.
// ----------------------------------------------------------------------------

#[test]
fn stale_pending_draft_republished_after_newer_commit_never_regresses() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage
        .create_note("st.md", "# stale v0", &op_id())
        .unwrap();
    let base_hash = note.content_hash.clone();

    let draft = note.source.replace("v0", "stale draft");
    let op_a = op_id();
    storage.push_failpoint(Failpoint::AfterPrepared);
    let err = storage
        .save(note.note_id.as_str(), &draft, note.local_revision, &op_a)
        .unwrap_err();
    assert_injected_io_error("AfterPrepared", &err);

    let stage_path = dir.path().join(format!("st.md.onote-stage-{op_a}"));
    let old_path = dir.path().join(format!("st.md.onote-old-{op_a}"));

    // reopen WITHOUT recover: baseline preserved, op still prepared
    let mut reopened = VaultStorage::open(dir.path()).unwrap();
    let db = Db::open(dir.path());
    let summary = reopened
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.note_id == note.note_id)
        .unwrap();
    assert_eq!(summary.local_revision, 0);
    assert_eq!(summary.content_hash, base_hash, "baseline hash preserved");
    assert_eq!(
        db.op_row(&op_a),
        (
            Some(0),
            Some(1),
            Some(base_hash.clone()),
            sha256_hex(&draft)
        )
    );
    assert!(stage_path.exists(), "stage retained");
    assert_eq!(read_bytes(&stage_path), draft.as_bytes());
    assert_eq!(
        read_bytes(&old_path),
        note.source.as_bytes(),
        "old backup = original bytes"
    );

    // recover keeps it pending (target untouched), everything retained
    let outcome = reopened.recover().unwrap();
    assert!(outcome.pending.iter().any(|id| *id == note.note_id));
    assert!(outcome.applied.is_empty());
    assert_eq!(db.op_status(&op_a).as_deref(), Some("prepared"));
    assert_eq!(
        db.notes_row("st.md").unwrap(),
        (0, base_hash.clone(), note.note_id.clone()),
    );
    let doc = reopened.read(note.note_id.as_str()).unwrap();
    assert!(doc.read_only);
    assert_eq!(doc.local_revision, 0);
    assert_eq!(sha256_hex(&doc.source), doc.content_hash);

    // a NEWER legitimate commit on top of the baseline: revision 1
    let newer = note.source.replace("v0", "newer committed");
    let op_b = op_id();
    let saved = reopened
        .save(note.note_id.as_str(), &newer, 0, &op_b)
        .unwrap();
    assert_eq!(saved.local_revision, 1);
    assert_eq!(
        db.notes_row("st.md").unwrap(),
        (1, sha256_hex(&newer), note.note_id.clone())
    );
    assert_eq!(db.op_status(&op_b).as_deref(), Some("committed"));

    // the stale draft bytes REAPPEAR on disk (same hash as the pending
    // journal's new_hash): recovery must NOT treat this as its own commit
    let draft_bytes = read_bytes(&stage_path);
    fs::write(dir.path().join("st.md"), &draft_bytes).unwrap();
    assert_eq!(
        sha256_hex(&draft),
        db.op_row(&op_a).3,
        "sanity: reimposed bytes match journal target hash"
    );
    assert_eq!(read_bytes(&dir.path().join("st.md")), draft_bytes);

    let outcome2 = reopened.recover().unwrap();
    assert!(
        outcome2.pending.iter().any(|id| *id == note.note_id),
        "ambiguous/newer rows must stay pending: {:?}",
        outcome2
    );
    // authoritative newer revision untouched by recovery
    assert_eq!(
        db.notes_row("st.md").unwrap(),
        (1, sha256_hex(&newer), note.note_id.clone()),
        "never roll back or silently consume the newer revision"
    );
    assert_eq!(
        db.op_status(&op_a).as_deref(),
        Some("prepared"),
        "journal preserved"
    );
    assert_eq!(db.op_status(&op_b).as_deref(), Some("committed"));
    assert!(stage_path.exists(), "stage retained on ambiguity");
    assert_eq!(
        read_bytes(&stage_path),
        draft_bytes,
        "stage bytes preserved"
    );
    assert!(old_path.exists(), "old backup retained on ambiguity");
    assert_eq!(
        read_bytes(&dir.path().join("st.md")),
        draft_bytes,
        "recovery must not rewrite file bytes either way"
    );

    // read while pending: READONLY on the authoritative NEWER revision (1)
    let doc = reopened.read(note.note_id.as_str()).unwrap();
    assert!(doc.read_only);
    assert_eq!(
        doc.local_revision, 1,
        "revision never regresses while pending"
    );
    assert_eq!(sha256_hex(&doc.source), doc.content_hash);
    assert_eq!(
        db.notes_row("st.md").unwrap().0,
        1,
        "no provisional read bump"
    );

    // reopen: pending keeps the authoritative baseline, no scan bump to 2
    let final_storage = VaultStorage::open(dir.path()).unwrap();
    let summary = final_storage
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.note_id == note.note_id)
        .unwrap();
    assert_eq!(summary.local_revision, 1);
    assert_eq!(
        summary.content_hash,
        sha256_hex(&newer),
        "authoritative hash stays on the newer commit"
    );
    let final_outcome = final_storage.recover().unwrap();
    assert!(final_outcome.pending.iter().any(|id| *id == note.note_id));
    assert_eq!(
        db.notes_row("st.md").unwrap(),
        (1, sha256_hex(&newer), note.note_id.clone()),
    );
}

// ----------------------------------------------------------------------------
// 3. already finalized exact same note/revision/hash: the prepared row is
//    safely recognized and consumed (marked committed) without rewriting the
//    authoritative row or bumping the revision.
// ----------------------------------------------------------------------------

#[test]
fn already_finalized_journal_consumed_without_revision_change() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage
        .create_note("af.md", "# final v0", &op_id())
        .unwrap();

    // op A: crash after prepared (bytes never replaced)
    let draft = note.source.replace("v0", "final v1");
    let op_a = op_id();
    storage.push_failpoint(Failpoint::AfterPrepared);
    let err = storage
        .save(note.note_id.as_str(), &draft, note.local_revision, &op_a)
        .unwrap_err();
    assert_injected_io_error("op A prepared crash", &err);

    // reopen + recover(op A): pending; baseline (0, original hash) intact
    let reopened = VaultStorage::open(dir.path()).unwrap();
    let db = Db::open(dir.path());
    let outcome = reopened.recover().unwrap();
    assert!(outcome.pending.iter().any(|id| *id == note.note_id));
    assert_eq!(db.op_status(&op_a).as_deref(), Some("prepared"));

    // op B: identical request bytes, distinct operation id, crash after replace
    let op_b = op_id();
    storage.push_failpoint(Failpoint::AfterReplace);
    let err = storage
        .save(note.note_id.as_str(), &draft, note.local_revision, &op_b)
        .unwrap_err();
    assert_injected_io_error("op B replace crash", &err);
    assert_eq!(read_bytes(&dir.path().join("af.md")), draft.as_bytes());
    assert_eq!(
        db.op_row(&op_b),
        (
            Some(0),
            Some(1),
            Some(note.content_hash.clone()),
            sha256_hex(&draft)
        )
    );

    // reopen + recover both:
    //  - op A finalizes legitimately (baseline still (0, original))
    //  - op B finds the row already exactly at its (note, rev, hash): consume
    let final_storage = VaultStorage::open(dir.path()).unwrap();
    let outcome = final_storage.recover().unwrap();
    assert!(outcome.applied.iter().any(|id| *id == note.note_id));
    assert!(outcome.pending.is_empty());
    assert_eq!(db.op_status(&op_a).as_deref(), Some("committed"));
    assert_eq!(db.op_status(&op_b).as_deref(), Some("committed"));
    assert_eq!(
        db.notes_row("af.md").unwrap(),
        (1, sha256_hex(&draft), note.note_id.clone()),
        "consume must not rewrite or bump the authoritative row"
    );

    let doc = final_storage.read(note.note_id.as_str()).unwrap();
    assert_eq!(doc.local_revision, 1);
    assert!(!doc.read_only);
    assert_eq!(doc.source, draft);
    assert_eq!(sha256_hex(&doc.source), doc.content_hash);

    // reopen stays exact
    let again = VaultStorage::open(dir.path()).unwrap();
    let doc = again.read(note.note_id.as_str()).unwrap();
    assert_eq!(doc.local_revision, 1);
    let outcome3 = again.recover().unwrap();
    assert!(outcome3.applied.is_empty() && outcome3.pending.is_empty());
    assert_eq!(db.notes_row("af.md").unwrap().0, 1);
}

// ----------------------------------------------------------------------------
// 4. pending CREATE must not acquire a competing provisional note mapping
//    from scan; it appears exactly when recovery finalizes it (file present
//    crash) or stays entirely unmapped with its stage retained.
// ----------------------------------------------------------------------------

#[test]
fn pending_create_has_no_provisional_mapping_before_recover() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();

    // part A: crash BeforeDbCommit — replace materialized the file, DB never did
    let op_c = op_id();
    storage.push_failpoint(Failpoint::BeforeDbCommit);
    let err = storage
        .create_note("pc.md", "# pending created", &op_c)
        .unwrap_err();
    assert_injected_io_error("create BeforeDbCommit", &err);
    assert!(
        dir.path().join("pc.md").exists(),
        "file replaced before db commit"
    );

    let reopened = VaultStorage::open(dir.path()).unwrap();
    let db = Db::open(dir.path());
    assert!(
        reopened
            .list()
            .unwrap()
            .iter()
            .all(|s| s.relative_path != "pc.md"),
        "pending CREATE must not appear via a provisional scan mapping"
    );
    assert_eq!(
        db.notes_count(),
        0,
        "no provisional notes row for pending create"
    );
    assert_eq!(db.prepared_count("pc.md"), 1);

    let outcome = reopened.recover().unwrap();
    assert!(!outcome.applied.is_empty(), "file-present create finalizes");
    assert!(outcome.pending.is_empty());
    assert_eq!(db.prepared_count("pc.md"), 0);
    let created = reopened.list().unwrap();
    assert_eq!(created.len(), 1);
    let summary = &created[0];
    assert_eq!(summary.relative_path, "pc.md");
    assert_eq!(summary.local_revision, 0);
    let doc = reopened.read(summary.note_id.as_str()).unwrap();
    assert!(doc.source.contains("opennote-id:"));
    assert!(!doc.read_only);
    assert_eq!(read_bytes(&dir.path().join("pc.md")), doc.source.as_bytes(),);
    // exact identical state after another reopen
    let again = VaultStorage::open(dir.path()).unwrap();
    assert_eq!(
        again.read(summary.note_id.as_str()).unwrap().local_revision,
        0
    );
    assert_eq!(db.notes_row("pc.md").unwrap().0, 0);

    // part B: crash AfterPrepared — file never existed, stays fully unmapped
    let mut storage = again;
    let op_c2 = op_id();
    storage.push_failpoint(Failpoint::AfterPrepared);
    let err = storage
        .create_note("pc2.md", "# still nowhere", &op_c2)
        .unwrap_err();
    assert_injected_io_error("create AfterPrepared", &err);
    assert!(!dir.path().join("pc2.md").exists());

    let after_reopen = VaultStorage::open(dir.path()).unwrap();
    let notes = after_reopen.list().unwrap();
    assert_eq!(notes.len(), 1, "only the recovered create is mapped");
    assert!(notes.iter().all(|s| s.relative_path != "pc2.md"));
    assert_eq!(
        db.notes_count(),
        1,
        "unrecovered create must never acquire a mapping from scan"
    );
    let outcome2 = after_reopen.recover().unwrap();
    assert!(outcome2.applied.is_empty());
    assert_eq!(outcome2.pending.len(), 1, "file-is-pending stays pending");
    assert!(
        find_entry(dir.path(), "pc2.md.onote-stage-").is_some(),
        "stage retained"
    );
    assert_eq!(db.notes_count(), 1);
    assert_eq!(after_reopen.list().unwrap().len(), 1);
}
