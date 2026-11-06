use std::path::Path;

use onote_domain::ErrorCode;

use crate::errors::{self, SeaRes};
use crate::ops::OperationRecord;
use crate::scan::hex_sha256;
use crate::state::{self, NoteRecord};

pub(crate) struct Core<'a> {
    pub root: &'a Path,
    pub conn: &'a rusqlite::Connection,
    pub vault_id: &'a str,
    pub archive: Option<&'a dyn crate::RevisionArchive>,
}

fn preserve_previous(core: &Core, plan: &CommitPlan, old_rel: Option<&str>) -> SeaRes<()> {
    let (Some(archive), Some(hash)) = (core.archive, plan.old_hash.as_deref()) else {
        return Ok(());
    };
    let revision = plan
        .new_revision
        .checked_sub(1)
        .ok_or_else(|| errors::err(ErrorCode::InvalidInput, "error.invalidRevision", &[]))?;
    for relative in old_rel
        .into_iter()
        .chain(std::iter::once(plan.relative.as_str()))
    {
        if let Ok(bytes) = state::guarded_read(core.root, relative) {
            if hex_sha256(&bytes) != hash {
                continue;
            }
            let source = String::from_utf8(bytes).map_err(|_| {
                errors::err(ErrorCode::IoDenied, "error.historyInvalidEncoding", &[])
            })?;
            return archive.preserve(&onote_domain::NoteDocument {
                note_id: plan.note_id.clone(),
                vault_id: core.vault_id.to_owned(),
                relative_path: plan.relative.clone(),
                source,
                local_revision: revision,
                content_hash: hash.to_owned(),
                read_only: false,
            });
        }
    }
    // A previously protected revision may outlive its temporary journal snapshot.
    archive.verify(&plan.note_id, revision, hash)
}

pub(crate) struct CommitPlan {
    pub kind: String,
    pub note_id: String,
    pub relative: String,
    pub composed: String,
    pub new_hash: String,
    pub old_hash: Option<String>,
    pub new_revision: u64,
    pub request_digest: String,
    /// True when this is a resume of an already-persisted prepared journal
    /// row: the journal must not be re-INSERTed (operation_id stays unique).
    pub resume_prepared: bool,
    /// Validated journal aux paths on resume (`resume_prepared` only): the
    /// ownership was checked by `validate_record` before any disk action.
    pub resume_stage: Option<String>,
    pub resume_old: Option<String>,
}

impl CommitPlan {
    /// Stage path: the journal's validated owned name on resume, otherwise
    /// the same-directory name derived from the operation UUID (never from
    /// a hash prefix, which was shared by concurrent-but-distinct ops).
    fn stage_rel(&self, operation_id: &str) -> String {
        match &self.resume_stage {
            Some(rel) => rel.clone(),
            None => format!("{}.onote-stage-{}", self.relative, operation_id),
        }
    }

    /// Old-snapshot path: journal-validated owned name on resume, otherwise
    /// the operation-UUID-derived same-directory name.
    fn old_rel(&self, operation_id: &str) -> Option<String> {
        match &self.resume_old {
            Some(rel) => Some(rel.clone()),
            None => self
                .old_hash
                .as_ref()
                .map(|_| format!("{}.onote-old-{}", self.relative, operation_id)),
        }
    }

    fn to_record(
        &self,
        operation_id: &str,
        stage_relative: String,
        old_relative: Option<String>,
    ) -> OperationRecord {
        OperationRecord {
            request_digest: self.request_digest.clone(),
            operation_id: operation_id.to_owned(),
            status: "prepared".into(),
            kind: self.kind.clone(),
            note_id: self.note_id.clone(),
            note_relative: self.relative.clone(),
            old_revision: self.new_revision.checked_sub(1),
            new_revision: Some(self.new_revision),
            old_hash: self.old_hash.clone(),
            new_hash: self.new_hash.clone(),
            stage_relative,
            old_relative,
            created_at: crate::ops::now_stamp(),
        }
    }
}

// ----------------------------------------------------------------------------
// Journal validation: rows coming back from state.sqlite are untrusted.
// Before recovery, resume or any cleanup touches them, every path/field must
// satisfy the exact ownership contract this crate would have written.
// ----------------------------------------------------------------------------

/// Only the exact canonical rendering this crate writes (lowercase
/// hyphenated, i.e. `Uuid::to_string()`) identifies a UUID: `parse_str`
/// alone would also accept `urn:uuid:…`, braced and simple spellings whose
/// raw interpolation into aux filenames contains colons — a parseable
/// Windows ADS path. Non-canonical forms are refused before any path or
/// filesystem use.
pub(crate) fn is_canonical_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).map_or(false, |u| u.to_string() == s)
}

pub(crate) fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hash8(h: &str) -> String {
    h.chars().take(8).collect()
}

/// A stored note path must be a safe relative Markdown path inside the vault
/// root and outside the `.opennote` app directory, in the exact canonical
/// form this crate writes (forward slashes, no backslashes).
fn valid_note_path(rel: &str) -> bool {
    if rel.is_empty() || rel.contains('\\') {
        return false;
    }
    crate::pathcheck::validate_new_note_path(rel).is_ok()
}

/// The only acceptable aux filenames: operation-UUID derived (current
/// format) or content-hash8 derived (legacy pre-hardening format). Every
/// other relative name — including traversal, ADS or reparse targets — is
/// not owner-checked and must never be read, reused or deleted.
fn accepted_stage_names(rel: &str, operation_id: &str, new_hash: &str) -> [String; 2] {
    [
        format!("{rel}.onote-stage-{operation_id}"),
        format!("{rel}.onote-stage-{}", hash8(new_hash)),
    ]
}

fn accepted_old_names(rel: &str, operation_id: &str, new_hash: &str) -> [String; 2] {
    [
        format!("{rel}.onote-old-{operation_id}"),
        format!("{rel}.onote-old-{}", hash8(new_hash)),
    ]
}

/// Full structural validation of a journal row: UUIDs, kind, hash syntax,
/// the create/save revision transition (create rev0, save old+1), a safe
/// note path outside `.opennote`, and aux paths that exactly match the
/// derived owned names. Invalid rows stay pending; nothing of theirs is
/// ever read, reused or deleted.
pub(crate) fn validate_record(rec: &OperationRecord) -> bool {
    if !is_canonical_uuid(&rec.operation_id) || !is_canonical_uuid(&rec.note_id) {
        return false;
    }
    if !is_hash(&rec.new_hash) || !valid_note_path(&rec.note_relative) {
        return false;
    }
    if !accepted_stage_names(&rec.note_relative, &rec.operation_id, &rec.new_hash)
        .contains(&rec.stage_relative)
    {
        return false;
    }
    match (rec.kind.as_str(), rec.old_hash.as_deref()) {
        ("create", None) => {
            rec.old_revision.is_none() && rec.new_revision == Some(0) && rec.old_relative.is_none()
        }
        ("save", Some(old_hash)) => {
            if !is_hash(old_hash) {
                return false;
            }
            rec.new_revision >= Some(1)
                && rec.old_revision == rec.new_revision.and_then(|r| r.checked_sub(1))
                && rec.old_relative.as_deref().is_some_and(|old_rel| {
                    accepted_old_names(&rec.note_relative, &rec.operation_id, &rec.new_hash)
                        .contains(&old_rel.to_owned())
                })
        }
        _ => false,
    }
}

// ----------------------------------------------------------------------------
// Durable staging: exclusive creation with exact-content reuse.
// ----------------------------------------------------------------------------

/// Create the aux copy only when nothing exists there yet; a preexisting
/// entry is reused ONLY when it is a plain file whose exact bytes hash to
/// `content_hash`. A reparse point, directory or foreign content is never
/// truncated, followed or replaced — the operation fails typed instead.
pub(crate) fn ensure_aux(
    root: &Path,
    rel_name: &str,
    bytes: &[u8],
    content_hash: &str,
    key: &str,
) -> SeaRes<()> {
    let path = root.join(rel_name);
    if hex_sha256(bytes) != content_hash {
        // The staged content must always be exactly the journal/payload
        // bytes this operation is committed to — never whatever the target
        // currently happens to hold.
        return Err(errors::err(ErrorCode::IoDenied, key, &[]));
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            std::io::Write::write_all(&mut file, bytes).map_err(|e| errors::io(&e, key))?;
            file.sync_all().map_err(|e| errors::io(&e, key))?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::symlink_metadata(&path).map_or(true, |m| m.is_dir())
                || crate::scan::is_reparse(&path)
            {
                return Err(errors::err(ErrorCode::IoDenied, key, &[]));
            }
            match std::fs::read(&path) {
                Ok(existing) if hex_sha256(&existing) == content_hash => Ok(()),
                _ => Err(errors::err(ErrorCode::IoDenied, key, &[])),
            }
        }
        Err(e) => Err(errors::io(&e, key)),
    }
}

/// The one finalization authorization rule, shared by fresh commits,
/// prepared resumes and recovery: the authoritative DB state must still be
/// the journal's own baseline (pre-operation revision/hash/note id), or the
/// create baseline legitimately absent, or already exactly this journal's
/// final state (an idempotent consume, never a rewrite). Used the same way
/// everywhere; the caller never re-implements it.
fn finalize_authorized(plan: &CommitPlan, db_row: &Option<NoteRecord>) -> bool {
    let baseline_matches = match (plan.old_hash.as_deref(), &db_row) {
        (Some(old_hash), Some(row)) => {
            Some(row.local_revision) == plan.new_revision.checked_sub(1)
                && old_hash == row.content_hash
                && row.note_id == plan.note_id
        }
        _ => false,
    };
    let create_absent = plan.old_hash.is_none() && db_row.is_none();
    let already_final = match &db_row {
        Some(row) => {
            row.note_id == plan.note_id
                && row.content_hash == plan.new_hash
                && row.local_revision == plan.new_revision
        }
        None => false,
    };
    baseline_matches || create_absent || already_final
}

/// Durable commit with crash boundaries. Auxiliary staging names are derived
/// from the operation UUID; everything the target gets replaced by or the
/// journal makes deletable from is owner-checked first. The target's fresh
/// state is classified before replacement and revalidated immediately
/// beforehand; create publishes through `fs::hard_link` (an atomic
/// no-replace std primitive on every platform), save through atomic rename,
/// so there is never a delete-rename gap and never a preexisting unrelated
/// file/link truncation.
pub(crate) fn execute_commit(
    core: &Core,
    plan: &CommitPlan,
    operation_id: &str,
    failpoints: &mut dyn FnMut(crate::Failpoint) -> bool,
) -> SeaRes<()> {
    // The plan itself is journal-grade data: one wrong path must never
    // reach disk operations.
    if !is_canonical_uuid(operation_id)
        || !is_canonical_uuid(&plan.note_id)
        || !is_hash(&plan.new_hash)
        || plan.old_hash.as_deref().is_some_and(|h| !is_hash(h))
        || !(plan.kind == "create" || plan.kind == "save")
        || !valid_note_path(&plan.relative)
        || plan.resume_prepared
            && ((plan.kind == "create" && (plan.resume_old.is_some() || plan.old_hash.is_some()))
                || plan.kind == "save" && (plan.resume_old.is_none() || plan.old_hash.is_none()))
    {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.pathEscape",
            &[],
        ));
    }

    let stage_rel = plan.stage_rel(operation_id);
    let old_rel = plan.old_rel(operation_id);
    let target = core.root.join(&plan.relative);

    // Fresh classification of the target, under the writer gate.
    let target_state = match state::guarded_read(core.root, &plan.relative) {
        Ok(bytes) => Some(hex_sha256(&bytes)),
        Err(e) if e.code == ErrorCode::ObjectNotFound => None,
        Err(e) => return Err(e),
    };

    // Authoritative DB row for the guarded-finalize decision (loaded fresh
    // under the gate, exactly once for this invocation).
    let db_row = state::load_notes(core.conn)?
        .into_iter()
        .find(|n| n.relative_path == plan.relative);

    // ---- branch: target already carries the journal's final bytes --------
    if target_state.as_deref() == Some(plan.new_hash.as_str()) {
        if !plan.resume_prepared {
            // Fresh attempt turning out already-final (identical-bytes
            // save): record the prepared journal first so a crash before
            // the finalize still leaves a resumable slot.
            let record = plan.to_record(operation_id, stage_rel.clone(), old_rel.clone());
            crate::ops::write_prepared(core.conn, &record)?;
        }
        if !finalize_authorized(plan, &db_row) {
            // Ambiguous or newer authoritative state: preserve everything,
            // refuse. The old backup is never rewritten with new bytes.
            return Err(errors::err(
                ErrorCode::VersionConflict,
                "error.externalChange",
                &[("relativePath", plan.relative.clone())],
            ));
        }
        if failpoints(crate::Failpoint::BeforeDbCommit) {
            return Err(errors::err(
                ErrorCode::IoDenied,
                "error.simulatedCrashBeforeDbCommit",
                &[],
            ));
        }
        let note = NoteRecord {
            note_id: plan.note_id.clone(),
            relative_path: plan.relative.clone(),
            content_hash: plan.new_hash.clone(),
            local_revision: plan.new_revision,
            read_only: false,
            id_embedded: true,
        };
        preserve_previous(core, plan, old_rel.as_deref())?;
        crate::ops::finalize_commit(core.conn, operation_id, &note)?;
        cleanup_aux(
            core.root,
            &stage_rel,
            old_rel.as_deref(),
            &plan.new_hash,
            plan.old_hash.as_deref(),
        );
        return Ok(());
    }

    // ---- branch: nothing on disk and a create is being published ---------
    if target_state.is_none() && plan.old_hash.is_none() {
        if let Some(parent_abs) = target.parent() {
            state::ensure_writable_dir_parents(core.root, parent_abs)?;
        }
        if failpoints(crate::Failpoint::WriteFailure) {
            return Err(errors::err(
                ErrorCode::DiskFull,
                "error.simulatedWriteFailure",
                &[],
            ));
        }
        // 1. full new bytes in the same directory, flushed
        ensure_aux(
            core.root,
            &stage_rel,
            plan.composed.as_bytes(),
            &plan.new_hash,
            "error.stageWrite",
        )?;
        // 3. durable prepared journal (hashes/revision/stage path) unless
        //    this is a resume whose row already exists
        if !plan.resume_prepared {
            let record = plan.to_record(operation_id, stage_rel.clone(), None);
            crate::ops::write_prepared(core.conn, &record)?;
        }
        // 4. crash boundary: after prepared
        if failpoints(crate::Failpoint::AfterPrepared) {
            return Err(errors::err(
                ErrorCode::IoDenied,
                "error.simulatedCrashAfterPrepared",
                &[],
            ));
        }
        // 5. atomic no-replace publish: the target name appears only through
        // `hard_link` from the validated staged bytes; on any platform it
        // fails when the destination already exists, so a concurrently
        // appeared file/link is reported typed and left untouched.
        if failpoints(crate::Failpoint::BeforeReplace) {
            return Err(errors::err(
                ErrorCode::IoDenied,
                "error.simulatedReplaceInterrupted",
                &[],
            ));
        }
        std::fs::hard_link(core.root.join(&stage_rel), &target).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                errors::err(
                    ErrorCode::PathCollision,
                    "error.noteAlreadyExists",
                    &[("relativePath", plan.relative.clone())],
                )
            } else {
                errors::io(&e, "error.createTargetDenied")
            }
        })?;
        // 6. crash boundary: after replace
        if failpoints(crate::Failpoint::AfterReplace) {
            return Err(errors::err(
                ErrorCode::IoDenied,
                "error.simulatedCrashAfterReplace",
                &[],
            ));
        }
        // 7. verify the target now carries the new bytes
        let now_bytes = state::guarded_read(core.root, &plan.relative)?;
        if hex_sha256(&now_bytes) != plan.new_hash {
            return Err(errors::err(
                ErrorCode::IoDenied,
                "error.postReplaceHashMismatch",
                &[],
            ));
        }
        // 8. crash boundary: before db commit
        if failpoints(crate::Failpoint::BeforeDbCommit) {
            return Err(errors::err(
                ErrorCode::IoDenied,
                "error.simulatedCrashBeforeDbCommit",
                &[],
            ));
        }
        let note = NoteRecord {
            note_id: plan.note_id.clone(),
            relative_path: plan.relative.clone(),
            content_hash: plan.new_hash.clone(),
            local_revision: plan.new_revision,
            read_only: false,
            id_embedded: true,
        };
        crate::ops::finalize_commit(core.conn, operation_id, &note)?;
        // The target shares the staged file's data (two link names): removing
        // the stage name — only while it still hashes to the staged bytes —
        // leaves the note file intact.
        cleanup_aux(core.root, &stage_rel, None, &plan.new_hash, None);
        return Ok(());
    }

    // ---- branch: replace old bytes with new (save) ------------------------
    let old_hash = match (target_state.as_deref(), plan.old_hash.as_deref()) {
        (Some(actual), Some(old)) if actual == old => old.to_owned(),
        // Anything else (newer/ambiguous external bytes, vanished target, or
        // a create plan that lost its absent target): preserve everything
        // and refuse.
        (Some(_), Some(_)) => {
            return Err(errors::err(
                ErrorCode::VersionConflict,
                "error.externalChange",
                &[("relativePath", plan.relative.clone())],
            ));
        }
        (None, Some(_)) => {
            return Err(errors::err(
                ErrorCode::VersionConflict,
                "error.externalChange",
                &[("relativePath", plan.relative.clone())],
            ));
        }
        (Some(_), None) => {
            return Err(errors::err(
                ErrorCode::PathCollision,
                "error.noteAlreadyExists",
                &[("relativePath", plan.relative.clone())],
            ));
        }
        (None, None) => {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.noteFileMissing",
                &[],
            ));
        }
    };

    if let Some(parent_abs) = target.parent() {
        state::ensure_writable_dir_parents(core.root, parent_abs)?;
    }

    if failpoints(crate::Failpoint::WriteFailure) {
        return Err(errors::err(
            ErrorCode::DiskFull,
            "error.simulatedWriteFailure",
            &[],
        ));
    }

    // 1. full new bytes in the same directory, flushed (exclusive create;
    //    a left-over stage of this exact operation with the exact bytes is
    //    reused, anything else is refused, never truncated)
    ensure_aux(
        core.root,
        &stage_rel,
        plan.composed.as_bytes(),
        &plan.new_hash,
        "error.stageWrite",
    )?;

    // 2. old snapshot copy, flushed before the journal; bytes are the
    //    baseline bytes this operation was prepared against (never the
    //    target's current content when it has already been replaced).
    if let Some(old_rel) = &old_rel {
        ensure_aux(
            core.root,
            old_rel,
            &state::guarded_read(core.root, &plan.relative)?,
            &old_hash,
            "error.oldSnapshotWrite",
        )?;
    }
    preserve_previous(core, plan, old_rel.as_deref())?;

    // 3. durable prepared journal (unless this is a resume whose row exists)
    if !plan.resume_prepared {
        let record = plan.to_record(operation_id, stage_rel.clone(), old_rel.clone());
        crate::ops::write_prepared(core.conn, &record)?;
    }

    // 4. crash boundary: after prepared
    if failpoints(crate::Failpoint::AfterPrepared) {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.simulatedCrashAfterPrepared",
            &[],
        ));
    }

    if failpoints(crate::Failpoint::BeforeReplace) {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.simulatedReplaceInterrupted",
            &[],
        ));
    }

    // 5. revalidate the target immediately before replacement after staging
    let pre_bytes = state::guarded_read(core.root, &plan.relative)?;
    if hex_sha256(&pre_bytes) != old_hash {
        return Err(errors::err(
            ErrorCode::VersionConflict,
            "error.externalChange",
            &[("relativePath", plan.relative.clone())],
        ));
    }

    // atomic replace: std rename = MoveFileExW REPLACE_EXISTING (Windows) /
    // rename(2) (Unix) — the target is never deleted-then-renamed.
    std::fs::rename(core.root.join(&stage_rel), &target)
        .map_err(|e| errors::io(&e, "error.replaceFailed"))?;

    // 6. crash boundary: after replace
    if failpoints(crate::Failpoint::AfterReplace) {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.simulatedCrashAfterReplace",
            &[],
        ));
    }

    // 7. verify the target now carries the new bytes
    let now_bytes = state::guarded_read(core.root, &plan.relative)?;
    if hex_sha256(&now_bytes) != plan.new_hash {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.postReplaceHashMismatch",
            &[],
        ));
    }

    // 8. crash boundary: before db commit
    if failpoints(crate::Failpoint::BeforeDbCommit) {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.simulatedCrashBeforeDbCommit",
            &[],
        ));
    }

    // 9. guarded finalize: only against the authoritative old baseline or
    //    the exact already-final row; otherwise preserve everything and
    //    refuse (the slot stays pending for recovery).
    if !finalize_authorized(plan, &db_row) {
        return Err(errors::err(
            ErrorCode::VersionConflict,
            "error.externalChange",
            &[("relativePath", plan.relative.clone())],
        ));
    }
    let note = NoteRecord {
        note_id: plan.note_id.clone(),
        relative_path: plan.relative.clone(),
        content_hash: plan.new_hash.clone(),
        local_revision: plan.new_revision,
        read_only: false,
        id_embedded: true,
    };
    crate::ops::finalize_commit(core.conn, operation_id, &note)?;

    cleanup_aux(
        core.root,
        &stage_rel,
        old_rel.as_deref(),
        &plan.new_hash,
        Some(old_hash.as_str()),
    );
    Ok(())
}

/// Best-effort cleanup of the two owned aux names after a successful
/// finalize. Every removal is content-gated: a name is removed only when it
/// is still a guarded safe plain file (no traversal, no ADS-safe-by-
/// construction, reparse parents/points and directories refused) whose
/// exact bytes hash to the expected stage/old content hash. Missing files
/// are already clean; a foreign sentinel that replaced an aux file, any
/// unreadable or unexpected entry is left untouched, never deleted —
/// shared by `execute_commit` and recovery so both finalize paths gate
/// identically. No journal traversal, no deletions by other names.
fn cleanup_aux(
    root: &Path,
    stage_rel: &str,
    old_rel: Option<&str>,
    stage_hash: &str,
    old_hash: Option<&str>,
) {
    cleanup_aux_file(root, stage_rel, stage_hash);
    if let (Some(old_rel), Some(old_hash)) = (old_rel, old_hash) {
        cleanup_aux_file(root, old_rel, old_hash);
    }
}

/// Remove one exact aux name under `root` only if it is a readable plain
/// file whose bytes hash exactly to `expected_hash` right now.
fn cleanup_aux_file(root: &Path, rel_name: &str, expected_hash: &str) {
    let rel = rel_name.replace('\\', "/");
    if rel.is_empty()
        || rel.starts_with('/')
        || rel
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
    {
        return;
    }
    let path = root.join(rel_name);
    if let Ok(bytes) = state::guarded_read(root, rel_name) {
        if hex_sha256(&bytes) == expected_hash && !crate::scan::is_reparse(&path) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Replay of prepared journal rows after restart. Rows are validated first
/// (paths/fields untrusted: invalid rows stay pending with nothing read,
/// nothing reused and nothing deleted). Replaced slots are finalized ONLY
/// when the authoritative DB baseline still matches the journal origin (the
/// pre-operation revision/hash) — or the create baseline is legitimately
/// absent — or the target note row already holds exactly this journal's
/// final note/revision/hash. Whenever the DB has moved on (later committed
/// revision, stale draft bytes reappearing, removed baseline), the slot
/// stays pending with journal, stage file and old backup untouched:
/// recovery never rolls back, never silently consumes ambiguous or newer
/// rows, never regresses the authoritative revision and never deletes files
/// from unchecked journal-relative paths.
pub(crate) fn perform_recovery(core: &Core) -> SeaRes<crate::RecoverOutcome> {
    let rows = crate::ops::list_prepared(core.conn)?;
    let mut applied = Vec::new();
    let mut pending = Vec::new();
    for rec in rows {
        if !validate_record(&rec) {
            pending.push(rec.note_id.clone());
            continue;
        }
        let target_hash = match state::guarded_read(core.root, &rec.note_relative) {
            Ok(bytes) => Some(hex_sha256(&bytes)),
            Err(e) if e.code == ErrorCode::ObjectNotFound => None,
            Err(e) => return Err(e),
        };
        if target_hash.as_deref() != Some(rec.new_hash.as_str()) {
            // never replaced (or ambiguous external content): keep the
            // original and the staged draft; the journal row stays pending.
            pending.push(rec.note_id.clone());
            continue;
        }

        // Replacement bytes are on disk. Consult the authoritative baseline
        // FRESHLY each iteration: earlier rows may have just been finalized
        // (chained pending operations apply in order).
        let db_row = state::load_notes(core.conn)?
            .into_iter()
            .find(|n| n.relative_path == rec.note_relative);

        let plan = CommitPlan {
            kind: rec.kind.clone(),
            note_id: rec.note_id.clone(),
            relative: rec.note_relative.clone(),
            composed: String::new(),
            new_hash: rec.new_hash.clone(),
            old_hash: rec.old_hash.clone(),
            new_revision: rec.new_revision.unwrap_or(0),
            request_digest: String::new(),
            resume_prepared: true,
            resume_stage: Some(rec.stage_relative.clone()),
            resume_old: rec.old_relative.clone(),
        };
        if !finalize_authorized(&plan, &db_row) {
            pending.push(rec.note_id.clone());
            continue;
        }
        if preserve_previous(core, &plan, rec.old_relative.as_deref()).is_err() {
            pending.push(rec.note_id.clone());
            continue;
        }

        // finalized: write the journal's committed revision.
        let note = NoteRecord {
            note_id: rec.note_id.clone(),
            relative_path: rec.note_relative.clone(),
            content_hash: rec.new_hash.clone(),
            local_revision: rec.new_revision.unwrap_or(0),
            read_only: false,
            id_embedded: true,
        };
        crate::ops::finalize_commit(core.conn, &rec.operation_id, &note)?;
        cleanup_aux(
            core.root,
            &rec.stage_relative,
            rec.old_relative.as_deref(),
            &rec.new_hash,
            rec.old_hash.as_deref(),
        );
        applied.push(rec.note_id.clone());
    }
    Ok(crate::RecoverOutcome { applied, pending })
}
