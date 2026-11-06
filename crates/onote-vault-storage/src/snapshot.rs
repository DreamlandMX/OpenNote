use crate::{
    errors::{self, SeaRes},
    gate, ops, scan, state, trash, VaultStorage,
};
use onote_domain::{ErrorCode, NoteSummary, SaveResult, Vault};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path},
};

impl VaultStorage {
    /// Only M08's quarantined new-directory restore calls this. The verified
    /// immutable snapshot protects the original revisions; file bytes stay intact.
    pub fn finalize_snapshot_restore(
        &mut self,
        expected: &[NoteSummary],
        manifest_hash: &str,
        operation_id: &str,
    ) -> SeaRes<Vec<SaveResult>> {
        Self::require_operation_id(operation_id)?;
        if manifest_hash.len() != 64
            || !manifest_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.restoreManifest",
                &[],
            ));
        }
        let conn = self.require_writable()?;
        let gate = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate)?;
        let mut expected = expected.to_vec();
        expected.sort_by(|a, b| a.note_id.cmp(&b.note_id));
        let mut ids = HashSet::new();
        let mut paths = HashSet::new();
        if expected.iter().any(|n| {
            !crate::commit::is_canonical_uuid(&n.note_id)
                || n.vault_id != self.vault_info.vault_id
                || n.local_revision >= 9_007_199_254_740_991
                || !ids.insert(n.note_id.clone())
                || !paths.insert(n.relative_path.clone())
        }) {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.restoreBaseline",
                &[],
            ));
        }
        let digest = scan::hex_sha256(
            &serde_json::to_vec(&(&self.vault_info.vault_id, manifest_hash, &expected))
                .map_err(|_| errors::err(ErrorCode::IoDenied, "error.restoreSerialize", &[]))?,
        );
        let existing:Option<(String,String,String,bool)>=conn.query_row("SELECT request_digest,result_payload,checksum,activated FROM snapshot_restores WHERE operation_id=?1",[operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(errors::database)?;
        let results = expected
            .iter()
            .map(|n| SaveResult {
                note_id: n.note_id.clone(),
                local_revision: n.local_revision + 1,
                content_hash: n.content_hash.clone(),
            })
            .collect::<Vec<_>>();
        if let Some((saved, payload, checksum, activated)) = existing {
            if saved != digest {
                return Err(errors::err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[],
                ));
            }
            let previous: Vec<SaveResult> = serde_json::from_str(&payload)
                .map_err(|_| errors::err(ErrorCode::IoDenied, "error.restoreCorrupt", &[]))?;
            if scan::hex_sha256(payload.as_bytes()) != checksum || previous != results {
                return Err(errors::err(
                    ErrorCode::IoDenied,
                    "error.restoreCorrupt",
                    &[],
                ));
            }
            if activated {
                return Ok(results);
            }
        } else {
            let meta: serde_json::Value =
                serde_json::from_slice(&state::guarded_read(&self.root, ".opennote/vault.json")?)
                    .map_err(|_| errors::err(ErrorCode::IoDenied, "error.restoreMetadata", &[]))?;
            if meta.get("writable").and_then(|v| v.as_bool()) != Some(false)
                || meta.get("syncEnabled").and_then(|v| v.as_bool()) != Some(false)
            {
                return Err(errors::err(
                    ErrorCode::InvalidInput,
                    "error.restoreNotQuarantined",
                    &[],
                ));
            }
            trash::refuse_operation_reuse(conn, operation_id)?;
            if ops::find_by_operation_id(conn, operation_id)?.is_some()
                || conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM draft_checkpoints WHERE draft_id=?1)",
                        [operation_id],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(errors::database)?
            {
                return Err(errors::err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[],
                ));
            }
            let (paths, ids) = trash::protected(conn)?;
            if !ops::list_prepared(conn)?.is_empty() || !paths.is_empty() || !ids.is_empty() {
                return Err(errors::err(
                    ErrorCode::RescanRequired,
                    "error.restorePending",
                    &[],
                ));
            }
            let records = state::load_notes(conn)?;
            if records.len() != expected.len() {
                return Err(errors::err(
                    ErrorCode::VersionConflict,
                    "error.restoreBaseline",
                    &[],
                ));
            }
            let by_id = records
                .iter()
                .map(|n| (n.note_id.as_str(), n))
                .collect::<HashMap<_, _>>();
            if by_id.len() != records.len() {
                return Err(errors::err(
                    ErrorCode::RescanRequired,
                    "error.restoreBaseline",
                    &[],
                ));
            }
            for note in &expected {
                let record = by_id.get(note.note_id.as_str()).ok_or_else(|| {
                    errors::err(ErrorCode::VersionConflict, "error.restoreBaseline", &[])
                })?;
                if record.relative_path != note.relative_path
                    || record.local_revision != note.local_revision
                    || record.content_hash != note.content_hash
                    || scan::hex_sha256(&state::guarded_read(&self.root, &note.relative_path)?)
                        != note.content_hash
                {
                    return Err(errors::err(
                        ErrorCode::VersionConflict,
                        "error.restoreBaseline",
                        &[],
                    ));
                }
            }
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .map_err(errors::database)?;
            for note in &expected {
                let changed=tx.execute("UPDATE notes SET local_revision=?1 WHERE note_id=?2 AND relative_path=?3 AND local_revision=?4 AND content_hash=?5",rusqlite::params![note.local_revision+1,note.note_id,note.relative_path,note.local_revision,note.content_hash]).map_err(errors::database)?;
                if changed != 1 {
                    return Err(errors::err(
                        ErrorCode::VersionConflict,
                        "error.restoreBaseline",
                        &[],
                    ));
                }
            }
            let payload = serde_json::to_string(&results)
                .map_err(|_| errors::err(ErrorCode::IoDenied, "error.restoreSerialize", &[]))?;
            tx.execute("INSERT INTO snapshot_restores(operation_id,request_digest,result_payload,checksum) VALUES(?1,?2,?3,?4)",rusqlite::params![operation_id,digest,payload,scan::hex_sha256(payload.as_bytes())]).map_err(errors::database)?;
            if self.take_failpoint(crate::Failpoint::BeforeDbCommit) {
                return Err(errors::err(
                    ErrorCode::IoDenied,
                    "error.injectedRestoreFailure",
                    &[],
                ));
            }
            tx.commit().map_err(errors::database)?;
        }
        state::patch_meta(
            &self.root.join(".opennote"),
            &[("syncEnabled", false.into()), ("writable", true.into())],
        )?;
        conn.execute(
            "UPDATE snapshot_restores SET activated=1 WHERE operation_id=?1",
            [operation_id],
        )
        .map_err(errors::database)?;
        *self.notes.borrow_mut() = state::load_notes(conn)?;
        self.vault_info.writable = true;
        self.vault_info.sync_enabled = false;
        Ok(results)
    }
    /// Export SQLite at a committed boundary and hold the writer gate while M08
    /// captures immutable files. The callback must not call storage operations.
    pub fn with_snapshot_state<T>(
        &self,
        database_target: &Path,
        capture: impl FnOnce(&Path, &Vault, &[NoteSummary]) -> SeaRes<T>,
    ) -> SeaRes<T> {
        let root =
            fs::canonicalize(&self.root).map_err(|e| errors::io(&e, "error.snapshotSource"))?;
        if !database_target.is_absolute()
            || database_target
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.snapshotTarget",
                &[],
            ));
        }
        let parent = database_target
            .parent()
            .ok_or_else(|| errors::err(ErrorCode::InvalidInput, "error.snapshotTarget", &[]))?;
        let mut ancestor = parent.to_path_buf();
        loop {
            if scan::is_reparse(&ancestor) {
                return Err(errors::err(ErrorCode::IoDenied, "error.pathReparse", &[]));
            }
            if !ancestor.pop() {
                break;
            }
        }
        let parent =
            fs::canonicalize(parent).map_err(|e| errors::io(&e, "error.snapshotTarget"))?;
        if parent.starts_with(&root) {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.snapshotRecursive",
                &[],
            ));
        }
        let target =
            parent.join(database_target.file_name().ok_or_else(|| {
                errors::err(ErrorCode::InvalidInput, "error.snapshotTarget", &[])
            })?);
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(errors::err(
                    ErrorCode::PathCollision,
                    "error.snapshotTargetExists",
                    &[],
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(errors::io(&e, "error.snapshotTarget")),
        }
        let gate = gate::acquire(&root)?;
        let _guard = gate::lock(&gate)?;
        let legacy;
        let conn = if self.conn.is_none() && matches!(self.vault_info.schema_version, 1 | 2) {
            // Preserve old journal tables without interpreting or migrating them.
            let current = state::legacy_open::open_if_incompatible(&root)?.ok_or_else(|| {
                errors::err(ErrorCode::VersionConflict, "error.snapshotChanged", &[])
            })?;
            if current.vault_info.vault_id != self.vault_info.vault_id
                || current.vault_info.schema_version != self.vault_info.schema_version
                || current
                    .vault_info
                    .findings
                    .iter()
                    .any(|f| f.code == "LegacyCatalogUnavailable" || f.code == "DuplicateNoteId")
            {
                return Err(errors::err(
                    ErrorCode::RescanRequired,
                    "error.snapshotRescan",
                    &[],
                ));
            }
            legacy = state::legacy_open::readonly_connection(&root)?;
            &legacy
        } else {
            let conn = self.require_writable()?;
            crate::rename::guard_pending(conn)?;
            let (paths, ids) = trash::protected(conn)?;
            if !ops::list_prepared(conn)?.is_empty() || !paths.is_empty() || !ids.is_empty() {
                return Err(errors::err(
                    ErrorCode::RescanRequired,
                    "error.snapshotPending",
                    &[],
                ));
            }
            conn
        };
        let records = state::load_notes(conn)?;
        let scanned = scan::scan_vault(&root)?;
        let mut seen = HashSet::new();
        if !scanned.findings.is_empty()
            || records.len() != scanned.notes.len()
            || records
                .iter()
                .any(|record| !seen.insert(record.note_id.clone()))
        {
            return Err(errors::err(
                ErrorCode::RescanRequired,
                "error.snapshotRescan",
                &[],
            ));
        }
        let by_path = scanned
            .notes
            .iter()
            .map(|note| (note.relative_path.as_str(), note))
            .collect::<HashMap<_, _>>();
        for record in &records {
            let current = by_path.get(record.relative_path.as_str()).ok_or_else(|| {
                errors::err(ErrorCode::RescanRequired, "error.snapshotRescan", &[])
            })?;
            if current.content_hash != record.content_hash
                || current
                    .embedded_id
                    .as_ref()
                    .is_some_and(|id| id != &record.note_id)
            {
                return Err(errors::err(
                    ErrorCode::VersionConflict,
                    "error.snapshotChanged",
                    &[],
                ));
            }
        }
        let target_text = target
            .to_str()
            .ok_or_else(|| errors::err(ErrorCode::InvalidInput, "error.snapshotTarget", &[]))?;
        // VACUUM INTO reads a consistent SQLite snapshot including WAL; copying
        // state.sqlite directly could silently lose durable journal/draft rows.
        conn.execute("VACUUM INTO ?1", [target_text])
            .map_err(errors::database)?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&target)
            .and_then(|file| file.sync_all())
            .map_err(|e| errors::io(&e, "error.snapshotFlush"))?;
        let notes = records
            .iter()
            .map(|record| state::summary(record, &self.vault_info.vault_id))
            .collect::<Vec<_>>();
        capture(&root, &self.vault_info, &notes)
    }
}

pub(crate) fn init(conn: &Connection) -> SeaRes<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS snapshot_restores(operation_id TEXT PRIMARY KEY,request_digest TEXT NOT NULL,result_payload TEXT NOT NULL,checksum TEXT NOT NULL,activated INTEGER NOT NULL DEFAULT 0);").map_err(errors::database)?;
    let columns = conn
        .prepare("PRAGMA table_info(snapshot_restores)")
        .map_err(errors::database)?
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(errors::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(errors::database)?;
    if !columns.iter().any(|name| name == "activated") {
        conn.execute_batch(
            "ALTER TABLE snapshot_restores ADD COLUMN activated INTEGER NOT NULL DEFAULT 0",
        )
        .map_err(errors::database)?;
    }
    Ok(())
}
pub(crate) fn refuse_operation_reuse(conn: &Connection, id: &str) -> SeaRes<()> {
    if conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM snapshot_restores WHERE operation_id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(errors::database)?
    {
        return Err(errors::err(
            ErrorCode::IdempotencyMismatch,
            "error.idempotencyMismatch",
            &[],
        ));
    }
    Ok(())
}
