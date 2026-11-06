use crate::{
    commit,
    errors::{err, SeaRes},
    gate, identity, ops, pathcheck, scan, state, Failpoint, RecoverOutcome, VaultStorage,
};
use onote_domain::{ErrorCode, NoteDocument, TrashEntry, TrashRecord, TrashRestoreResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs};

const MAX_REV: u64 = 9_007_199_254_740_990;
fn invalid() -> onote_domain::ServiceError {
    err(ErrorCode::IoDenied, "error.trashCorrupt", &[])
}
fn conflict() -> onote_domain::ServiceError {
    err(
        ErrorCode::VersionConflict,
        "error.trashBaselineChanged",
        &[],
    )
}
fn missing() -> onote_domain::ServiceError {
    err(ErrorCode::ObjectNotFound, "error.trashMissing", &[])
}
fn unique_note(conn: &Connection, note_id: &str) -> SeaRes<state::NoteRecord> {
    let notes = state::load_notes(conn)?;
    let mut matching = notes.into_iter().filter(|note| note.note_id == note_id);
    let record = matching.next().ok_or_else(missing)?;
    if matching.next().is_some() {
        return Err(err(
            ErrorCode::RescanRequired,
            "error.duplicateNoteMapping",
            &[],
        ));
    }
    Ok(record)
}
fn digest<T: Serialize>(value: &T) -> SeaRes<String> {
    Ok(scan::hex_sha256(
        &serde_json::to_vec(value).map_err(|_| invalid())?,
    ))
}

pub(crate) fn init(conn: &Connection) -> SeaRes<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS trash_actions(operation_id TEXT PRIMARY KEY, status TEXT NOT NULL, payload TEXT NOT NULL, checksum TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS trash_action_status ON trash_actions(status);
        CREATE TABLE IF NOT EXISTS trash_items(object_id TEXT PRIMARY KEY, note_id TEXT NOT NULL, status TEXT NOT NULL, payload TEXT NOT NULL, checksum TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS trash_tombstones(note_id TEXT PRIMARY KEY, object_id TEXT NOT NULL, payload TEXT NOT NULL, checksum TEXT NOT NULL);")
        .map_err(crate::errors::database)
}

fn validate_entry(entry: &TrashEntry) -> SeaRes<()> {
    if !commit::is_canonical_uuid(&entry.object_id)
        || !commit::is_canonical_uuid(&entry.note_id)
        || pathcheck::validate_new_note_path(&entry.original_path)? != entry.original_path
        || entry.content_hash.len() != 64
        || !entry
            .content_hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || entry.deleted_revision == 0
        || entry.deleted_revision > MAX_REV
        || !(1..=3650).contains(&entry.retention_days)
        || chrono::DateTime::parse_from_rfc3339(&entry.deleted_at).is_err()
    {
        return Err(invalid());
    }
    Ok(())
}

pub fn validate_trash_record(record: &TrashRecord, vault_id: &str) -> SeaRes<()> {
    validate_entry(&record.entry)?;
    let note = &record.document;
    if note.vault_id != vault_id
        || note.note_id != record.entry.note_id
        || note.relative_path != record.entry.original_path
        || note.local_revision.checked_add(1) != Some(record.entry.deleted_revision)
        || note.content_hash != record.entry.content_hash
        || scan::hex_sha256(note.source.as_bytes()) != note.content_hash
        || note.read_only
    {
        return Err(invalid());
    }
    match identity::parse_frontmatter(&note.source) {
        identity::Frontmatter::NoBlock
        | identity::Frontmatter::WellFormed {
            id: identity::BlockId::None,
            ..
        } => Ok(()),
        identity::Frontmatter::WellFormed {
            id: identity::BlockId::Valid(id),
            ..
        } if id == note.note_id => Ok(()),
        _ => Err(invalid()),
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Action {
    operation_id: String,
    vault_id: String,
    status: String,
    kind: String,
    entry: TrashEntry,
    expected_revision: u64,
    target_path: Option<String>,
    content_hash: String,
    request_digest: String,
}
impl Action {
    fn request_digest(&self) -> SeaRes<String> {
        digest(&(
            &self.kind,
            &self.entry.note_id,
            &self.entry.object_id,
            self.expected_revision,
            &self.target_path,
            &self.entry.content_hash,
        ))
    }
    fn validate(&self) -> SeaRes<()> {
        validate_entry(&self.entry)?;
        if !commit::is_canonical_uuid(&self.operation_id)
            || !commit::is_canonical_uuid(&self.vault_id)
            || !matches!(
                self.status.as_str(),
                "prepared" | "committed" | "cancelling" | "cancelled"
            )
            || self.request_digest != self.request_digest()?
        {
            return Err(invalid());
        }
        match self.kind.as_str() {
            "delete"
                if self.entry.object_id == self.operation_id
                    && self.expected_revision.checked_add(1)
                        == Some(self.entry.deleted_revision)
                    && self.target_path.is_none()
                    && self.content_hash == self.entry.content_hash =>
            {
                Ok(())
            }
            "restore"
                if self.expected_revision == self.entry.deleted_revision
                    && self.target_path.as_deref().is_some_and(|p| {
                        pathcheck::validate_new_note_path(p).is_ok_and(|canonical| canonical == p)
                    })
                    && self.content_hash.len() == 64
                    && self
                        .content_hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
            {
                Ok(())
            }
            "purge"
                if self.expected_revision == self.entry.deleted_revision
                    && self.target_path.is_none()
                    && self.content_hash == self.entry.content_hash =>
            {
                Ok(())
            }
            _ => Err(invalid()),
        }
    }
    fn aux_path(&self) -> String {
        let path = self
            .target_path
            .as_deref()
            .unwrap_or(&self.entry.original_path);
        format!("{path}.onote-trash-{}", self.operation_id)
    }
    fn restore_result(&self) -> TrashRestoreResult {
        TrashRestoreResult {
            note_id: self.entry.note_id.clone(),
            relative_path: self.target_path.clone().unwrap_or_default(),
            local_revision: self.entry.deleted_revision + 1,
            content_hash: self.content_hash.clone(),
        }
    }
}

fn encode<T: Serialize>(value: &T) -> SeaRes<(String, String)> {
    let payload = serde_json::to_string(value).map_err(|_| invalid())?;
    Ok((payload.clone(), scan::hex_sha256(payload.as_bytes())))
}
fn decode<T: serde::de::DeserializeOwned>(payload: &str, checksum: &str) -> SeaRes<T> {
    if scan::hex_sha256(payload.as_bytes()) != checksum {
        return Err(invalid());
    }
    serde_json::from_str(payload).map_err(|_| invalid())
}
fn find(conn: &Connection, id: &str) -> SeaRes<Option<Action>> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT status,payload,checksum FROM trash_actions WHERE operation_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(crate::errors::database)?;
    row.map(|(status, payload, checksum)| {
        let action: Action = decode(&payload, &checksum)?;
        action.validate()?;
        if action.operation_id != id || action.status != status {
            return Err(invalid());
        }
        Ok(action)
    })
    .transpose()
}
fn put(conn: &Connection, action: &Action, insert: bool) -> SeaRes<()> {
    action.validate()?;
    let (payload, checksum) = encode(action)?;
    let sql = if insert {
        "INSERT INTO trash_actions(operation_id,status,payload,checksum) VALUES (?1,?2,?3,?4)"
    } else {
        "UPDATE trash_actions SET status=?2,payload=?3,checksum=?4 WHERE operation_id=?1"
    };
    let changed = conn
        .execute(
            sql,
            params![action.operation_id, action.status, payload, checksum],
        )
        .map_err(crate::errors::database)?;
    if changed != 1 {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) fn refuse_operation_reuse(conn: &Connection, id: &str) -> SeaRes<()> {
    crate::snapshot::refuse_operation_reuse(conn, id)?;
    crate::rename::refuse_operation_reuse(conn, id)?;
    crate::tag_batches::refuse_operation_reuse(conn, id)?;
    if find(conn, id)?.is_some() {
        return Err(err(
            ErrorCode::IdempotencyMismatch,
            "error.idempotencyMismatch",
            &[],
        ));
    }
    Ok(())
}
fn actions(conn: &Connection) -> SeaRes<Vec<Action>> {
    let mut stmt = conn
        .prepare("SELECT operation_id FROM trash_actions WHERE status IN ('prepared','cancelling') ORDER BY rowid")
        .map_err(crate::errors::database)?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(crate::errors::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::errors::database)?;
    ids.iter()
        .map(|id| find(conn, id)?.ok_or_else(invalid))
        .collect()
}
pub(crate) fn protected(conn: &Connection) -> SeaRes<(HashSet<String>, HashSet<String>)> {
    let mut paths = HashSet::new();
    let mut ids = HashSet::new();
    for action in actions(conn)? {
        paths.insert(action.entry.original_path);
        if let Some(path) = action.target_path {
            paths.insert(path);
        }
        ids.insert(action.entry.note_id);
    }
    Ok((paths, ids))
}
pub(crate) fn guard_pending(conn: &Connection, note_id: Option<&str>, path: &str) -> SeaRes<()> {
    crate::rename::guard_pending(conn)?;
    let (paths, ids) = protected(conn)?;
    if paths
        .iter()
        .any(|candidate| pathcheck::same_windows_path(candidate, path))
        || note_id.is_some_and(|id| ids.contains(id))
    {
        return Err(conflict());
    }
    Ok(())
}

fn item(conn: &Connection, id: &str, require_active: bool) -> SeaRes<TrashEntry> {
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT note_id,status,payload,checksum FROM trash_items WHERE object_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(crate::errors::database)?;
    let Some((note_id, status, payload, checksum)) = row else {
        return Err(missing());
    };
    let entry: TrashEntry = decode(&payload, &checksum)?;
    validate_entry(&entry)?;
    if entry.object_id != id
        || entry.note_id != note_id
        || !matches!(status.as_str(), "active" | "restored" | "purged")
    {
        return Err(invalid());
    }
    if require_active && status != "active" {
        return Err(missing());
    }
    Ok(entry)
}
pub(crate) fn tombstones(conn: &Connection) -> SeaRes<Vec<TrashEntry>> {
    let mut stmt = conn
        .prepare("SELECT note_id,object_id,payload,checksum FROM trash_tombstones")
        .map_err(crate::errors::database)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(crate::errors::database)?;
    rows.map(|row| {
        let (note_id, id, payload, checksum) = row.map_err(crate::errors::database)?;
        let entry: TrashEntry = decode(&payload, &checksum)?;
        validate_entry(&entry)?;
        if entry.note_id != note_id || entry.object_id != id {
            return Err(invalid());
        }
        Ok(entry)
    })
    .collect()
}

impl VaultStorage {
    pub fn list_trash_operations(&self) -> SeaRes<Vec<onote_domain::TrashOperation>> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))?;
        actions(conn)?
            .into_iter()
            .map(|action| {
                if action.vault_id != self.vault_info.vault_id {
                    return Err(invalid());
                }
                Ok(onote_domain::TrashOperation {
                    operation_id: action.operation_id,
                    kind: action.kind,
                    phase: action.status,
                    entry: action.entry,
                    target_path: action.target_path,
                })
            })
            .collect()
    }

    pub fn resume_trash(
        &mut self,
        operation_id: &str,
    ) -> SeaRes<onote_domain::TrashOperationResult> {
        self.trash_operation_start(operation_id)?;
        self.trash_ready()?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        let conn = self.require_writable()?;
        let mut action = find(conn, operation_id)?.ok_or_else(missing)?;
        if matches!(action.status.as_str(), "cancelled" | "cancelling") {
            return Err(conflict());
        }
        self.execute_trash(&mut action, false)?;
        *self.notes.borrow_mut() = state::load_notes(conn)?;
        let restored = (action.kind == "restore").then(|| action.restore_result());
        Ok(onote_domain::TrashOperationResult {
            kind: action.kind,
            entry: action.entry,
            restored,
        })
    }

    pub fn trash_entry(&self, object_id: &str) -> SeaRes<TrashEntry> {
        Self::require_operation_id(object_id)?;
        item(
            self.conn
                .as_ref()
                .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))?,
            object_id,
            true,
        )
    }
    fn trash_ready(&self) -> SeaRes<&dyn crate::TrashArchive> {
        if !self.vault_info.writable || self.vault_info.sync_enabled {
            return Err(conflict());
        }
        self.trash_archive
            .as_deref()
            .ok_or_else(|| err(ErrorCode::IoDenied, "error.trashUnavailable", &[]))
    }
    fn trash_operation_start(&self, id: &str) -> SeaRes<()> {
        Self::require_operation_id(id)?;
        let conn = self.require_writable()?;
        crate::snapshot::refuse_operation_reuse(conn, id)?;
        crate::rename::refuse_operation_reuse(conn, id)?;
        crate::tag_batches::refuse_operation_reuse(conn, id)?;
        if ops::find_by_operation_id(conn, id)?.is_some()
            || crate::drafts::find_by_draft_id(conn, id)?.is_some()
        {
            return Err(err(
                ErrorCode::IdempotencyMismatch,
                "error.idempotencyMismatch",
                &[],
            ));
        }
        if !find(conn, id)?
            .is_some_and(|action| matches!(action.status.as_str(), "committed" | "cancelled"))
            && self.take_failpoint(Failpoint::WriteFailure)
        {
            return Err(err(
                ErrorCode::DiskFull,
                "error.simulatedTrashWriteFailure",
                &[],
            ));
        }
        Ok(())
    }
    pub fn list_trash(&self) -> SeaRes<Vec<TrashEntry>> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| err(ErrorCode::InvalidInput, "error.schemaReadOnly", &[]))?;
        let mut stmt = conn
            .prepare("SELECT object_id FROM trash_items WHERE status='active' ORDER BY rowid DESC")
            .map_err(crate::errors::database)?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(crate::errors::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(crate::errors::database)?;
        ids.iter().map(|id| item(conn, id, true)).collect()
    }
    pub fn delete_note(
        &mut self,
        note_id: &str,
        expected_revision: u64,
        expected_hash: &str,
        operation_id: &str,
    ) -> SeaRes<TrashEntry> {
        self.trash_operation_start(operation_id)?;
        self.trash_ready()?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        let conn = self.require_writable()?;
        let mut action = if let Some(action) = find(conn, operation_id)? {
            if action.kind != "delete"
                || action.entry.note_id != note_id
                || action.expected_revision != expected_revision
                || action.entry.content_hash != expected_hash
            {
                return Err(err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[],
                ));
            }
            action
        } else {
            let record = unique_note(conn, note_id)?;
            guard_pending(conn, Some(note_id), &record.relative_path)?;
            if ops::list_prepared(conn)?
                .iter()
                .any(|op| op.note_id == note_id || op.note_relative == record.relative_path)
            {
                return Err(conflict());
            }
            let source = self.load_source(&record.relative_path)?;
            if record.read_only
                || record.local_revision != expected_revision
                || record.content_hash != expected_hash
                || scan::hex_sha256(source.as_bytes()) != expected_hash
                || expected_revision >= MAX_REV
            {
                return Err(conflict());
            }
            let entry = TrashEntry {
                object_id: operation_id.into(),
                note_id: note_id.into(),
                original_path: record.relative_path.clone(),
                content_hash: record.content_hash.clone(),
                deleted_revision: record.local_revision + 1,
                deleted_at: chrono::Utc::now().to_rfc3339(),
                retention_days: 30,
            };
            let document = NoteDocument {
                note_id: note_id.into(),
                vault_id: self.vault_info.vault_id.clone(),
                relative_path: record.relative_path,
                source,
                local_revision: record.local_revision,
                content_hash: record.content_hash,
                read_only: false,
            };
            let archived = match self.trash_ready()?.read_deleted(operation_id) {
                Ok(existing) => {
                    validate_trash_record(&existing, &self.vault_info.vault_id)?;
                    if existing.document != document || existing.entry.object_id != operation_id {
                        return Err(err(
                            ErrorCode::IdempotencyMismatch,
                            "error.idempotencyMismatch",
                            &[],
                        ));
                    }
                    existing
                }
                Err(e)
                    if matches!(
                        e.code,
                        ErrorCode::ObjectNotFound | ErrorCode::HistoryMissing
                    ) =>
                {
                    TrashRecord { entry, document }
                }
                Err(e) => return Err(e),
            };
            let entry = archived.entry.clone();
            validate_trash_record(&archived, &self.vault_info.vault_id)?;
            self.revision_archive
                .as_deref()
                .ok_or_else(invalid)?
                .force_preserve(&archived.document)?;
            self.trash_ready()?.capture_deleted(&archived)?;
            let mut action = Action {
                operation_id: operation_id.into(),
                vault_id: self.vault_info.vault_id.clone(),
                status: "prepared".into(),
                kind: "delete".into(),
                entry,
                expected_revision,
                target_path: None,
                content_hash: expected_hash.into(),
                request_digest: String::new(),
            };
            action.request_digest = action.request_digest()?;
            put(conn, &action, true)?;
            action
        };
        self.execute_trash(&mut action, false)?;
        *self.notes.borrow_mut() = state::load_notes(conn)?;
        Ok(action.entry)
    }
    pub fn restore_trash(
        &mut self,
        object_id: &str,
        target_path: &str,
        expected_revision: u64,
        operation_id: &str,
    ) -> SeaRes<TrashRestoreResult> {
        self.trash_operation_start(operation_id)?;
        self.trash_ready()?;
        Self::require_operation_id(object_id)?;
        let canonical = pathcheck::validate_new_note_path(target_path)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        let conn = self.require_writable()?;
        let mut action = if let Some(action) = find(conn, operation_id)? {
            if action.kind != "restore"
                || action.entry.object_id != object_id
                || action.expected_revision != expected_revision
                || action.target_path.as_deref() != Some(canonical.as_str())
            {
                return Err(err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[],
                ));
            }
            action
        } else {
            let entry = item(conn, object_id, true)?;
            if entry.deleted_revision != expected_revision {
                return Err(conflict());
            }
            guard_pending(conn, Some(&entry.note_id), &canonical)?;
            self.require_tombstone(&entry, None)?;
            self.restore_destination(&canonical)?;
            let record = self.trash_ready()?.read_deleted(object_id)?;
            validate_trash_record(&record, &self.vault_info.vault_id)?;
            if record.entry != entry {
                return Err(invalid());
            }
            let composed = identity::compose_for_save(
                &record.document.source,
                &entry.note_id,
                &record.document.source,
            )?;
            let mut action = Action {
                operation_id: operation_id.into(),
                vault_id: self.vault_info.vault_id.clone(),
                status: "prepared".into(),
                kind: "restore".into(),
                entry,
                expected_revision,
                target_path: Some(canonical),
                content_hash: scan::hex_sha256(composed.as_bytes()),
                request_digest: String::new(),
            };
            action.request_digest = action.request_digest()?;
            put(conn, &action, true)?;
            action
        };
        self.execute_trash(&mut action, false)?;
        *self.notes.borrow_mut() = state::load_notes(conn)?;
        Ok(action.restore_result())
    }
    pub fn purge_trash(
        &mut self,
        object_id: &str,
        expected_revision: u64,
        operation_id: &str,
    ) -> SeaRes<()> {
        self.trash_operation_start(operation_id)?;
        self.trash_ready()?;
        Self::require_operation_id(object_id)?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        let conn = self.require_writable()?;
        let mut action = if let Some(action) = find(conn, operation_id)? {
            if action.kind != "purge"
                || action.entry.object_id != object_id
                || action.expected_revision != expected_revision
            {
                return Err(err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                    &[],
                ));
            }
            action
        } else {
            let entry = item(conn, object_id, true)?;
            if entry.deleted_revision != expected_revision {
                return Err(conflict());
            }
            guard_pending(conn, Some(&entry.note_id), &entry.original_path)?;
            let mut action = Action {
                operation_id: operation_id.into(),
                vault_id: self.vault_info.vault_id.clone(),
                status: "prepared".into(),
                kind: "purge".into(),
                content_hash: entry.content_hash.clone(),
                entry,
                expected_revision,
                target_path: None,
                request_digest: String::new(),
            };
            action.request_digest = action.request_digest()?;
            put(conn, &action, true)?;
            action
        };
        self.execute_trash(&mut action, false)
    }
    fn require_tombstone(&self, entry: &TrashEntry, own_target: Option<&str>) -> SeaRes<()> {
        if !tombstones(self.require_writable()?)?
            .iter()
            .any(|e| e == entry)
            || state::load_notes(self.require_writable()?)?
                .iter()
                .any(|n| n.note_id == entry.note_id)
        {
            return Err(conflict());
        }
        if scan::scan_vault(&self.root)?.notes.iter().any(|n| {
            n.embedded_id.as_deref() == Some(entry.note_id.as_str())
                && own_target != Some(n.relative_path.as_str())
        }) {
            return Err(conflict());
        }
        Ok(())
    }
    fn restore_destination(&self, path: &str) -> SeaRes<()> {
        if state::load_notes(self.require_writable()?)?
            .iter()
            .any(|n| pathcheck::same_windows_path(&n.relative_path, path))
        {
            return Err(err(
                ErrorCode::PathCollision,
                "error.noteAlreadyExists",
                &[],
            ));
        }
        match fs::symlink_metadata(self.root.join(path)) {
            Ok(_) => Err(err(
                ErrorCode::PathCollision,
                "error.noteAlreadyExists",
                &[],
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(crate::errors::io(&e, "error.trashTargetDenied")),
        }
    }
    fn execute_trash(&self, action: &mut Action, recovery: bool) -> SeaRes<()> {
        action.validate()?;
        if action.vault_id != self.vault_info.vault_id {
            return Err(invalid());
        }
        if action.status == "committed" {
            self.verify_completed_trash(action)?;
            return Ok(());
        }
        if action.status == "cancelled" {
            return Err(err(
                ErrorCode::VersionConflict,
                "error.operationCancelled",
                &[],
            ));
        }
        if action.status == "cancelling" {
            return match self.finish_trash_cancellation(action)? {
                onote_domain::TrashCancellation::AlreadyApplied => Ok(()),
                onote_domain::TrashCancellation::Cancelled => Err(err(
                    ErrorCode::VersionConflict,
                    "error.operationCancelled",
                    &[],
                )),
            };
        }
        let archive = self.trash_ready()?;
        if action.kind == "purge" {
            if item(self.require_writable()?, &action.entry.object_id, true)? != action.entry {
                return Err(conflict());
            }
            if self.take_failpoint(Failpoint::AfterPrepared) {
                return Err(err(
                    ErrorCode::IoDenied,
                    "error.simulatedTrashInterrupt",
                    &[],
                ));
            }
            archive.purge_deleted(&action.entry)?;
            if self.take_failpoint(Failpoint::AfterReplace) {
                return Err(err(
                    ErrorCode::IoDenied,
                    "error.simulatedTrashInterrupt",
                    &[],
                ));
            }
            return self.finalize_trash(action);
        }
        let record = archive.read_deleted(&action.entry.object_id)?;
        validate_trash_record(&record, &self.vault_info.vault_id)?;
        if record.entry != action.entry {
            return Err(invalid());
        }
        if action.kind == "delete" {
            self.revision_archive
                .as_deref()
                .ok_or_else(invalid)?
                .force_preserve(&record.document)?;
        }
        let aux = action.aux_path();
        if action.kind == "delete" {
            let current = unique_note(self.require_writable()?, &action.entry.note_id)?;
            if current.relative_path != action.entry.original_path
                || current.local_revision != action.expected_revision
                || current.content_hash != action.entry.content_hash
            {
                return Err(conflict());
            }
            let stage = state::guarded_read(&self.root, &aux);
            match stage {
                Ok(bytes) if scan::hex_sha256(&bytes) == action.entry.content_hash => {
                    match fs::symlink_metadata(self.root.join(&action.entry.original_path)) {
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                        _ => return Err(conflict()),
                    }
                }
                Err(e) if e.code == ErrorCode::ObjectNotFound => {
                    if recovery {
                        return Err(conflict());
                    }
                    if self.take_failpoint(Failpoint::AfterPrepared)
                        || self.take_failpoint(Failpoint::BeforeReplace)
                    {
                        return Err(err(
                            ErrorCode::IoDenied,
                            "error.simulatedTrashInterrupt",
                            &[],
                        ));
                    }
                    if scan::hex_sha256(&state::guarded_read(
                        &self.root,
                        &action.entry.original_path,
                    )?) != action.entry.content_hash
                    {
                        return Err(conflict());
                    }
                    state::ensure_writable_dir_parents(
                        &self.root,
                        self.root.join(&aux).parent().ok_or_else(invalid)?,
                    )?;
                    if fs::symlink_metadata(self.root.join(&aux)).is_ok() {
                        return Err(invalid());
                    }
                    fs::rename(
                        self.root.join(&action.entry.original_path),
                        self.root.join(&aux),
                    )
                    .map_err(|e| crate::errors::io(&e, "error.trashMoveDenied"))?;
                    if scan::hex_sha256(&state::guarded_read(&self.root, &aux)?)
                        != action.entry.content_hash
                    {
                        return Err(conflict());
                    }
                }
                _ => return Err(invalid()),
            }
        } else {
            self.require_tombstone(&action.entry, action.target_path.as_deref())?;
            let target = action.target_path.as_deref().ok_or_else(invalid)?;
            let composed = identity::compose_for_save(
                &record.document.source,
                &record.document.note_id,
                &record.document.source,
            )?;
            if scan::hex_sha256(composed.as_bytes()) != action.content_hash {
                return Err(invalid());
            }
            match state::guarded_read(&self.root, target) {
                Ok(bytes) if scan::hex_sha256(&bytes) == action.content_hash => (),
                Ok(_) => {
                    return Err(err(
                        ErrorCode::PathCollision,
                        "error.noteAlreadyExists",
                        &[],
                    ))
                }
                Err(e) if e.code == ErrorCode::ObjectNotFound => {
                    if recovery {
                        return Err(conflict());
                    }
                    self.restore_destination(target)?;
                    state::ensure_writable_dir_parents(
                        &self.root,
                        self.root.join(target).parent().ok_or_else(invalid)?,
                    )?;
                    commit::ensure_aux(
                        &self.root,
                        &aux,
                        composed.as_bytes(),
                        &action.content_hash,
                        "error.trashRestoreStage",
                    )?;
                    if self.take_failpoint(Failpoint::AfterPrepared)
                        || self.take_failpoint(Failpoint::BeforeReplace)
                    {
                        return Err(err(
                            ErrorCode::IoDenied,
                            "error.simulatedTrashInterrupt",
                            &[],
                        ));
                    }
                    fs::hard_link(self.root.join(&aux), self.root.join(target)).map_err(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            err(ErrorCode::PathCollision, "error.noteAlreadyExists", &[])
                        } else {
                            crate::errors::io(&e, "error.trashRestoreDenied")
                        }
                    })?;
                }
                Err(e) => return Err(e),
            }
            if scan::hex_sha256(&state::guarded_read(&self.root, target)?) != action.content_hash {
                return Err(conflict());
            }
        }
        if self.take_failpoint(Failpoint::AfterReplace)
            || self.take_failpoint(Failpoint::BeforeDbCommit)
        {
            return Err(err(
                ErrorCode::IoDenied,
                "error.simulatedTrashInterrupt",
                &[],
            ));
        }
        self.finalize_trash(action)?;
        let hash = if action.kind == "delete" {
            &action.entry.content_hash
        } else {
            &action.content_hash
        };
        if state::guarded_read(&self.root, &aux)
            .is_ok_and(|bytes| scan::hex_sha256(&bytes) == *hash)
        {
            let _ = fs::remove_file(self.root.join(aux));
        }
        Ok(())
    }
    pub fn cancel_trash(&mut self, operation_id: &str) -> SeaRes<onote_domain::TrashCancellation> {
        self.trash_operation_start(operation_id)?;
        self.trash_ready()?;
        let gate_arc = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate_arc)?;
        let conn = self.require_writable()?;
        let mut action = find(conn, operation_id)?.ok_or_else(missing)?;
        if action.vault_id != self.vault_info.vault_id {
            return Err(invalid());
        }
        if action.status == "committed" {
            self.verify_completed_trash(&action)?;
            return Ok(onote_domain::TrashCancellation::AlreadyApplied);
        }
        if action.status == "cancelled" {
            return Ok(onote_domain::TrashCancellation::Cancelled);
        }
        action.status = "cancelling".into();
        put(conn, &action, false)?;
        let result = self.finish_trash_cancellation(&mut action)?;
        *self.notes.borrow_mut() = state::load_notes(conn)?;
        Ok(result)
    }

    fn finish_trash_cancellation(
        &self,
        action: &mut Action,
    ) -> SeaRes<onote_domain::TrashCancellation> {
        let archive = self.trash_ready()?;
        let aux = action.aux_path();
        match action.kind.as_str() {
            "delete" => {
                let record = unique_note(self.require_writable()?, &action.entry.note_id)?;
                if record.relative_path != action.entry.original_path
                    || record.content_hash != action.entry.content_hash
                    || record.local_revision != action.expected_revision
                {
                    return Err(conflict());
                }
                match state::guarded_read(&self.root, &action.entry.original_path) {
                    Ok(bytes) if scan::hex_sha256(&bytes) == action.entry.content_hash => (),
                    Ok(_) => return Err(conflict()),
                    Err(e) if e.code == ErrorCode::ObjectNotFound => {
                        let saved = archive.read_deleted(&action.entry.object_id)?;
                        validate_trash_record(&saved, &self.vault_info.vault_id)?;
                        if saved.entry != action.entry
                            || scan::hex_sha256(&state::guarded_read(&self.root, &aux)?)
                                != action.entry.content_hash
                        {
                            return Err(invalid());
                        }
                        state::ensure_writable_dir_parents(
                            &self.root,
                            self.root
                                .join(&action.entry.original_path)
                                .parent()
                                .ok_or_else(invalid)?,
                        )?;
                        fs::hard_link(
                            self.root.join(&aux),
                            self.root.join(&action.entry.original_path),
                        )
                        .map_err(|e| crate::errors::io(&e, "error.trashCancelDenied"))?;
                        if scan::hex_sha256(&state::guarded_read(
                            &self.root,
                            &action.entry.original_path,
                        )?) != action.entry.content_hash
                        {
                            return Err(conflict());
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            "restore" => {
                self.require_tombstone(&action.entry, action.target_path.as_deref())?;
                match state::guarded_read(
                    &self.root,
                    action.target_path.as_deref().ok_or_else(invalid)?,
                ) {
                    Ok(bytes) if scan::hex_sha256(&bytes) == action.content_hash => {
                        let saved = archive.read_deleted(&action.entry.object_id)?;
                        validate_trash_record(&saved, &self.vault_info.vault_id)?;
                        if saved.entry != action.entry {
                            return Err(invalid());
                        }
                        let composed = identity::compose_for_save(
                            &saved.document.source,
                            &saved.document.note_id,
                            &saved.document.source,
                        )?;
                        if scan::hex_sha256(composed.as_bytes()) != action.content_hash {
                            return Err(invalid());
                        }
                        self.finalize_trash(action)?;
                        return Ok(onote_domain::TrashCancellation::AlreadyApplied);
                    }
                    Ok(_) => return Err(conflict()),
                    Err(e) if e.code == ErrorCode::ObjectNotFound => (),
                    Err(e) => return Err(e),
                }
            }
            "purge" => match archive.read_deleted(&action.entry.object_id) {
                Ok(record) if record.entry == action.entry => (),
                Ok(_) => return Err(invalid()),
                Err(e)
                    if matches!(
                        e.code,
                        ErrorCode::ObjectNotFound | ErrorCode::HistoryMissing
                    ) =>
                {
                    self.finalize_trash(action)?;
                    return Ok(onote_domain::TrashCancellation::AlreadyApplied);
                }
                Err(e) => return Err(e),
            },
            _ => return Err(invalid()),
        }
        if self.take_failpoint(Failpoint::BeforeDbCommit) {
            return Err(err(
                ErrorCode::IoDenied,
                "error.simulatedTrashCancelInterrupt",
                &[],
            ));
        }
        action.status = "cancelled".into();
        put(self.require_writable()?, action, false)?;
        // Cancellation retains auxiliaries: matching bytes alone cannot prove creation ownership.
        Ok(onote_domain::TrashCancellation::Cancelled)
    }

    fn verify_completed_trash(&self, action: &Action) -> SeaRes<()> {
        let conn = self.require_writable()?;
        if item(conn, &action.entry.object_id, false)? != action.entry {
            return Err(invalid());
        }
        if action.kind == "restore"
            && !state::load_notes(conn)?.iter().any(|n| {
                n.note_id == action.entry.note_id
                    && n.local_revision > action.entry.deleted_revision
            })
            && !tombstones(conn)?.iter().any(|entry| {
                entry.note_id == action.entry.note_id
                    && entry.deleted_revision > action.entry.deleted_revision
            })
        {
            return Err(invalid());
        }
        if action.kind == "purge" {
            let status: String = conn
                .query_row(
                    "SELECT status FROM trash_items WHERE object_id=?1",
                    [&action.entry.object_id],
                    |r| r.get(0),
                )
                .map_err(crate::errors::database)?;
            if status != "purged" {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn finalize_trash(&self, action: &mut Action) -> SeaRes<()> {
        let conn = self.require_writable()?;
        let (payload, checksum) = encode(&action.entry)?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(crate::errors::database)?;
        let previous = action.status.clone();
        action.status = "committed".into();
        let result = (|| -> SeaRes<()> {
            match action.kind.as_str() {
                "delete" => {
                    conn.execute("INSERT INTO trash_items(object_id,note_id,status,payload,checksum) VALUES (?1,?2,'active',?3,?4)",params![action.entry.object_id,action.entry.note_id,payload,checksum]).map_err(crate::errors::database)?;
                    conn.execute("INSERT INTO trash_tombstones(note_id,object_id,payload,checksum) VALUES (?1,?2,?3,?4)",params![action.entry.note_id,action.entry.object_id,payload,checksum]).map_err(crate::errors::database)?;
                    if conn
                        .execute(
                            "DELETE FROM notes WHERE note_id=?1",
                            [&action.entry.note_id],
                        )
                        .map_err(crate::errors::database)?
                        != 1
                    {
                        return Err(conflict());
                    }
                }
                "restore" => {
                    let result = action.restore_result();
                    conn.execute("INSERT INTO notes(relative_path,note_id,content_hash,local_revision,read_only,id_embedded) VALUES (?1,?2,?3,?4,0,1)",params![result.relative_path,result.note_id,result.content_hash,result.local_revision as i64]).map_err(crate::errors::database)?;
                    if conn
                        .execute(
                            "DELETE FROM trash_tombstones WHERE note_id=?1 AND object_id=?2",
                            params![action.entry.note_id, action.entry.object_id],
                        )
                        .map_err(crate::errors::database)?
                        != 1
                    {
                        return Err(conflict());
                    }
                    conn.execute(
                        "UPDATE trash_items SET status='restored' WHERE object_id=?1",
                        [&action.entry.object_id],
                    )
                    .map_err(crate::errors::database)?;
                }
                "purge" => {
                    conn.execute(
                        "UPDATE trash_items SET status='purged' WHERE object_id=?1",
                        [&action.entry.object_id],
                    )
                    .map_err(crate::errors::database)?;
                }
                _ => return Err(invalid()),
            }
            put(conn, action, false)?;
            conn.execute_batch("COMMIT")
                .map_err(crate::errors::database)
        })();
        if result.is_err() {
            let _ = conn.execute_batch("ROLLBACK");
            action.status = previous;
        }
        result
    }
    pub(crate) fn recover_trash(&self) -> SeaRes<RecoverOutcome> {
        let mut outcome = RecoverOutcome {
            applied: Vec::new(),
            pending: Vec::new(),
        };
        for mut action in actions(self.require_writable()?)? {
            if self.execute_trash(&mut action, true).is_ok() && action.status == "committed" {
                outcome.applied.push(action.entry.note_id);
            } else if action.status != "cancelled" {
                outcome.pending.push(action.entry.note_id);
            }
        }
        Ok(outcome)
    }
}
