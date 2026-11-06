use crate::{
    commit,
    errors::{err, SeaRes},
    gate, ops, pathcheck, scan, state, Failpoint, VaultStorage,
};
use onote_domain::{ErrorCode, RenameItemResult, RenamePlan, RenameResult};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs};

fn conflict() -> onote_domain::ServiceError {
    err(
        ErrorCode::VersionConflict,
        "error.renameBaselineChanged",
        &[],
    )
}
fn corrupt() -> onote_domain::ServiceError {
    err(ErrorCode::IoDenied, "error.renameJournalInvalid", &[])
}
fn mismatch() -> onote_domain::ServiceError {
    err(
        ErrorCode::IdempotencyMismatch,
        "error.idempotencyMismatch",
        &[],
    )
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    baseline: onote_domain::RenameBaseline,
    destination: String,
    before: String,
    after: String,
    embedded: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    operation_id: String,
    root: std::path::PathBuf,
    plan: RenamePlan,
    expected_revision: u64,
    items: Vec<Item>,
    completed: usize,
    publishing: bool,
}

pub(crate) fn init(conn: &Connection) -> SeaRes<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS rename_actions(operation_id TEXT PRIMARY KEY, status TEXT NOT NULL, payload TEXT NOT NULL, checksum TEXT NOT NULL);")
        .map_err(crate::errors::database)
}
fn validate_plan(plan: &RenamePlan) -> SeaRes<()> {
    if !commit::is_canonical_uuid(&plan.plan_id)
        || !commit::is_canonical_uuid(&plan.vault_id)
        || !commit::is_canonical_uuid(&plan.note_id)
        || !plan.keeps_identity
        || plan.from_path == plan.to_path
        || pathcheck::validate_new_note_path(&plan.from_path)? != plan.from_path
        || pathcheck::validate_new_note_path(&plan.to_path)? != plan.to_path
    {
        return Err(corrupt());
    }
    let mut ids = HashSet::new();
    let mut paths = Vec::new();
    for b in &plan.expected_hashes {
        if !commit::is_canonical_uuid(&b.note_id)
            || !ids.insert(&b.note_id)
            || b.expected_revision >= 9_007_199_254_740_991
            || pathcheck::validate_new_note_path(&b.relative_path)? != b.relative_path
            || paths
                .iter()
                .any(|p: &&str| pathcheck::same_windows_path(p, &b.relative_path))
            || b.content_hash.len() != 64
            || !b
                .content_hash
                .bytes()
                .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(corrupt());
        }
        paths.push(b.relative_path.as_str());
        if b.note_id == plan.note_id {
            if b.relative_path != plan.from_path {
                return Err(corrupt());
            }
        } else if !plan.edits.iter().any(|e| e.note_id == b.note_id) {
            return Err(corrupt());
        }
    }
    if !ids.contains(&plan.note_id) {
        return Err(corrupt());
    }
    for edit in &plan.edits {
        if !plan
            .expected_hashes
            .iter()
            .any(|b| b.note_id == edit.note_id && b.relative_path == edit.relative_path)
        {
            return Err(corrupt());
        }
    }
    Ok(())
}
fn rewrite(plan: &RenamePlan, id: &str, source: &str) -> SeaRes<String> {
    let mut edits = plan
        .edits
        .iter()
        .filter(|e| e.note_id == id)
        .collect::<Vec<_>>();
    edits.sort_by_key(|e| e.offset);
    let mut last = 0;
    for edit in &edits {
        let end = edit.offset.checked_add(edit.length).ok_or_else(corrupt)?;
        if edit.offset < last || source.get(edit.offset..end) != Some(edit.before.as_str()) {
            return Err(conflict());
        }
        last = end;
    }
    let mut result = source.to_owned();
    for edit in edits.into_iter().rev() {
        result.replace_range(edit.offset..edit.offset + edit.length, &edit.after);
    }
    let identity = |text: &str| match crate::identity::parse_frontmatter(text) {
        crate::identity::Frontmatter::NoBlock
        | crate::identity::Frontmatter::WellFormed {
            id: crate::identity::BlockId::None,
            ..
        } => Ok(None),
        crate::identity::Frontmatter::WellFormed {
            id: crate::identity::BlockId::Valid(value),
            ..
        } if value == id => Ok(Some(value)),
        _ => Err(corrupt()),
    };
    if identity(source)? != identity(&result)? {
        return Err(corrupt());
    }
    Ok(result)
}
fn load(conn: &Connection, id: &str) -> SeaRes<Option<Action>> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT status,payload,checksum FROM rename_actions WHERE operation_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(crate::errors::database)?;
    let Some((status, payload, checksum)) = row else {
        return Ok(None);
    };
    if scan::hex_sha256(payload.as_bytes()) != checksum {
        return Err(corrupt());
    }
    let a: Action = serde_json::from_str(&payload).map_err(|_| corrupt())?;
    validate_plan(&a.plan)?;
    if a.operation_id != id
        || !commit::is_canonical_uuid(id)
        || !a.root.is_absolute()
        || a.items.len() != a.plan.expected_hashes.len()
        || !a
            .plan
            .expected_hashes
            .iter()
            .any(|b| b.note_id == a.plan.note_id && b.expected_revision == a.expected_revision)
        || a.completed > a.items.len()
        || (a.completed == a.items.len() && a.publishing)
        || status
            != if a.completed == a.items.len() {
                "committed"
            } else {
                "prepared"
            }
    {
        return Err(corrupt());
    }
    for (i, b) in a.items.iter().zip(&a.plan.expected_hashes) {
        if &i.baseline != b
            || scan::hex_sha256(i.before.as_bytes()) != b.content_hash
            || i.after != rewrite(&a.plan, &b.note_id, &i.before)?
            || &i.destination
                != if b.note_id == a.plan.note_id {
                    &a.plan.to_path
                } else {
                    &b.relative_path
                }
        {
            return Err(corrupt());
        }
    }
    Ok(Some(a))
}
fn write(conn: &Connection, a: &Action, fresh: bool) -> SeaRes<()> {
    let payload = serde_json::to_string(a).map_err(|_| corrupt())?;
    let checksum = scan::hex_sha256(payload.as_bytes());
    let status = if a.completed == a.items.len() {
        "committed"
    } else {
        "prepared"
    };
    if fresh {
        conn.execute(
            "INSERT INTO rename_actions(operation_id,status,payload,checksum) VALUES (?1,?2,?3,?4)",
            rusqlite::params![a.operation_id, status, payload, checksum],
        )
    } else {
        conn.execute(
            "UPDATE rename_actions SET status=?2,payload=?3,checksum=?4 WHERE operation_id=?1",
            rusqlite::params![a.operation_id, status, payload, checksum],
        )
    }
    .map_err(crate::errors::database)?;
    Ok(())
}
pub(crate) fn pending(conn: &Connection) -> SeaRes<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT operation_id FROM rename_actions WHERE status='prepared' ORDER BY rowid")
        .map_err(crate::errors::database)?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(crate::errors::database)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(crate::errors::database)
}
pub(crate) fn reconcile_findings(
    conn: &Connection,
    root: &std::path::Path,
    vault_id: &str,
    notes: &[scan::ScannedNote],
    findings: &mut Vec<onote_domain::VaultFinding>,
) -> SeaRes<()> {
    for id in pending(conn)? {
        let a = load(conn, &id)?.ok_or_else(corrupt)?;
        if a.root != fs::canonicalize(root).map_err(|_| corrupt())? || a.plan.vault_id != vault_id {
            return Err(corrupt());
        }
        let duplicates = notes
            .iter()
            .filter(|n| n.embedded_id.as_deref() == Some(&a.plan.note_id))
            .collect::<Vec<_>>();
        if a.publishing
            && a.items
                .get(a.completed)
                .is_some_and(|i| i.baseline.note_id == a.plan.note_id)
            && duplicates.len() == 2
            && duplicates.iter().all(|n| {
                (n.relative_path == a.plan.from_path || n.relative_path == a.plan.to_path)
                    && a.items.iter().any(|i| {
                        i.baseline.note_id == a.plan.note_id
                            && (n.content_hash == i.baseline.content_hash
                                || n.content_hash == scan::hex_sha256(i.after.as_bytes()))
                    })
            })
        {
            findings.retain(|f| {
                f.code != "DuplicateNoteId"
                    || !duplicates
                        .iter()
                        .any(|n| f.target.as_deref() == Some(&n.relative_path))
            });
        }
    }
    Ok(())
}

pub(crate) fn guard_pending(conn: &Connection) -> SeaRes<()> {
    if !pending(conn)?.is_empty() {
        return Err(conflict());
    }
    Ok(())
}
pub(crate) fn refuse_operation_reuse(conn: &Connection, id: &str) -> SeaRes<()> {
    if conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rename_actions WHERE operation_id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(crate::errors::database)?
    {
        return Err(mismatch());
    }
    Ok(())
}
fn disk(root: &std::path::Path, path: &str) -> SeaRes<Option<String>> {
    match state::guarded_read(root, path) {
        Ok(b) => Ok(Some(scan::hex_sha256(&b))),
        Err(e) if e.code == ErrorCode::ObjectNotFound => Ok(None),
        Err(e) => Err(e),
    }
}
impl Action {
    fn result(&self) -> RenameResult {
        RenameResult {
            commit_id: self.operation_id.clone(),
            completed: self.completed == self.items.len(),
            items: self
                .items
                .iter()
                .take(self.completed)
                .map(|i| RenameItemResult {
                    note_id: i.baseline.note_id.clone(),
                    relative_path: i.destination.clone(),
                    local_revision: i.baseline.expected_revision + 1,
                    content_hash: scan::hex_sha256(i.after.as_bytes()),
                })
                .collect(),
        }
    }
}

impl VaultStorage {
    pub fn rename(
        &self,
        plan: &RenamePlan,
        expected_revision: u64,
        operation_id: &str,
    ) -> SeaRes<RenameResult> {
        Self::require_operation_id(operation_id)?;
        validate_plan(plan)?;
        let conn = self.require_writable()?;
        let gate = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate)?;
        if plan.vault_id != self.vault_info.vault_id {
            return Err(mismatch());
        }
        let root = fs::canonicalize(&self.root).map_err(|_| corrupt())?;
        let mut action = if let Some(a) = load(conn, operation_id)? {
            if a.plan != *plan || a.expected_revision != expected_revision || a.root != root {
                return Err(mismatch());
            }
            if a.completed == a.items.len() {
                return Ok(a.result());
            }
            a
        } else {
            guard_pending(conn)?;
            crate::trash::refuse_operation_reuse(conn, operation_id)?;
            if ops::find_by_operation_id(conn, operation_id)?.is_some() {
                return Err(mismatch());
            }
            if !ops::list_prepared(conn)?.is_empty() || !crate::trash::protected(conn)?.0.is_empty()
            {
                return Err(conflict());
            }
            if !self.vault_info.writable {
                return Err(conflict());
            }
            let notes = state::load_notes(conn)?;
            let mut items = Vec::new();
            for b in &plan.expected_hashes {
                let rows = notes
                    .iter()
                    .filter(|n| n.note_id == b.note_id)
                    .collect::<Vec<_>>();
                if rows.len() != 1 {
                    return Err(conflict());
                }
                let row = rows[0];
                if row.read_only
                    || row.relative_path != b.relative_path
                    || row.local_revision != b.expected_revision
                    || row.content_hash != b.content_hash
                    || (b.note_id == plan.note_id && expected_revision != b.expected_revision)
                {
                    return Err(conflict());
                }
                let before = Self::read_source_snapshot(&root, &b.relative_path)?;
                if scan::hex_sha256(before.as_bytes()) != b.content_hash {
                    return Err(conflict());
                }
                items.push(Item {
                    baseline: b.clone(),
                    destination: if b.note_id == plan.note_id {
                        plan.to_path.clone()
                    } else {
                        b.relative_path.clone()
                    },
                    after: rewrite(plan, &b.note_id, &before)?,
                    before,
                    embedded: row.id_embedded,
                });
            }
            // Refuse case collisions including directories and files not in the catalog.
            self.rename_destination(plan)?;
            let a = Action {
                operation_id: operation_id.into(),
                root,
                plan: plan.clone(),
                expected_revision,
                items,
                completed: 0,
                publishing: false,
            };
            write(conn, &a, true)?;
            a
        };
        self.execute_rename(conn, &mut action)
            .map_err(|mut error| {
                error.operation_id = Some(operation_id.to_owned());
                error
                    .params
                    .insert("completedItems".into(), action.completed.to_string());
                error
                    .params
                    .insert("totalItems".into(), action.items.len().to_string());
                error
            })?;
        Ok(action.result())
    }

    fn rename_destination(&self, plan: &RenamePlan) -> SeaRes<()> {
        let target = self.root.join(&plan.to_path);
        // Validate ancestors without creating them during a preview/baseline refusal.
        let mut current = self.root.clone();
        let parts = plan.to_path.split('/').collect::<Vec<_>>();
        for (index, part) in parts.iter().enumerate() {
            if current.is_dir() {
                for entry in fs::read_dir(&current).map_err(|_| corrupt())? {
                    let entry = entry.map_err(|_| corrupt())?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if pathcheck::same_windows_path(&name, part) {
                        let rel = entry
                            .path()
                            .strip_prefix(&self.root)
                            .map_err(|_| corrupt())?
                            .to_string_lossy()
                            .replace('\\', "/");
                        if scan::is_reparse(&entry.path())
                            || (index == parts.len() - 1
                                && !pathcheck::same_windows_path(&rel, &plan.from_path))
                            || (index < parts.len() - 1
                                && (name != *part || !entry.path().is_dir()))
                        {
                            return Err(err(
                                ErrorCode::PathCollision,
                                "error.noteAlreadyExists",
                                &[],
                            ));
                        }
                    }
                }
            }
            current.push(part);
        }
        if target == self.root.join(&plan.from_path) {
            return Err(conflict());
        }
        Ok(())
    }

    fn execute_rename(&self, conn: &Connection, a: &mut Action) -> SeaRes<()> {
        if !self.vault_info.writable
            || fs::canonicalize(&self.root).map_err(|_| corrupt())? != a.root
            || a.plan.vault_id != self.vault_info.vault_id
        {
            return Err(conflict());
        }
        let meta: serde_json::Value =
            serde_json::from_slice(&state::guarded_read(&self.root, ".opennote/vault.json")?)
                .map_err(|_| corrupt())?;
        if meta["vaultId"].as_str() != Some(&a.plan.vault_id)
            || meta["schemaVersion"].as_u64() != Some(self.vault_info.schema_version as u64)
            || meta["writable"].as_bool() != Some(true)
        {
            return Err(conflict());
        }
        // Check every baseline before resuming any remaining item, including already applied files.
        let rows = state::load_notes(conn)?;
        for (index, i) in a.items.iter().enumerate() {
            let old = disk(&self.root, &i.baseline.relative_path)?;
            let new = disk(&self.root, &i.destination)?;
            let new_hash = scan::hex_sha256(i.after.as_bytes());
            let moved = i.destination != i.baseline.relative_path;
            let same = pathcheck::same_windows_path(&i.destination, &i.baseline.relative_path);
            let publishing = index == a.completed && a.publishing;
            let backed_up = if publishing {
                let old_copy = format!(".opennote/rename-{}-{}.old", a.operation_id, index);
                disk(&self.root, &old_copy)?.as_ref() == Some(&i.baseline.content_hash)
            } else {
                false
            };
            let on_disk = if index < a.completed {
                new.as_ref() == Some(&new_hash) && (!moved || same || old.is_none())
            } else if !moved {
                old.as_ref() == Some(&i.baseline.content_hash)
                    || (publishing && backed_up && old.as_ref() == Some(&new_hash))
            } else {
                (old.as_ref() == Some(&i.baseline.content_hash)
                    || (publishing && backed_up && old.is_none())
                    || (publishing && backed_up && same && old.as_ref() == Some(&new_hash)))
                    && (new.is_none()
                        || (publishing && backed_up && new.as_ref() == Some(&new_hash))
                        || (same && new.as_ref() == Some(&i.baseline.content_hash)))
            };
            let matching = rows
                .iter()
                .filter(|n| n.note_id == i.baseline.note_id)
                .collect::<Vec<_>>();
            if !on_disk || matching.len() != 1 || matching[0].read_only {
                return Err(conflict());
            }
            let n = matching[0];
            let baseline = n.relative_path == i.baseline.relative_path
                && n.local_revision == i.baseline.expected_revision
                && n.content_hash == i.baseline.content_hash;
            let final_row = n.relative_path == i.destination
                && n.local_revision == i.baseline.expected_revision + 1
                && n.content_hash == new_hash;
            if !(if index < a.completed {
                final_row
            } else {
                baseline
            }) {
                return Err(conflict());
            }
        }
        if self.take_failpoint(Failpoint::WriteFailure) {
            return Err(err(ErrorCode::DiskFull, "error.simulatedWriteFailure", &[]));
        }
        if self.take_failpoint(Failpoint::AfterPrepared) {
            return Err(corrupt());
        }
        while a.completed < a.items.len() {
            let i = a.items[a.completed].clone();
            let new_hash = scan::hex_sha256(i.after.as_bytes());
            let stage = format!(
                "{}.onote-rename-{}-{}",
                i.destination, a.operation_id, a.completed
            );
            let old_copy = format!(".opennote/rename-{}-{}.old", a.operation_id, a.completed);
            state::ensure_writable_dir_parents(
                &self.root,
                self.root
                    .join(&i.destination)
                    .parent()
                    .ok_or_else(corrupt)?,
            )?;
            commit::ensure_aux(
                &self.root,
                &old_copy,
                i.before.as_bytes(),
                &i.baseline.content_hash,
                "error.renameJournalInvalid",
            )?;
            if let Some(archive) = &self.revision_archive {
                archive.force_preserve(&onote_domain::NoteDocument {
                    note_id: i.baseline.note_id.clone(),
                    vault_id: a.plan.vault_id.clone(),
                    relative_path: i.baseline.relative_path.clone(),
                    source: i.before.clone(),
                    local_revision: i.baseline.expected_revision,
                    content_hash: i.baseline.content_hash.clone(),
                    read_only: false,
                })?;
            }
            commit::ensure_aux(
                &self.root,
                &stage,
                i.after.as_bytes(),
                &new_hash,
                "error.renameJournalInvalid",
            )?;
            if self.take_failpoint(Failpoint::BeforeReplace) {
                return Err(corrupt());
            }
            let moved = i.destination != i.baseline.relative_path;
            let same = pathcheck::same_windows_path(&i.destination, &i.baseline.relative_path);
            if !a.publishing {
                a.publishing = true;
                write(conn, a, false)?;
            }
            if moved {
                if same && self.root.join(&i.baseline.relative_path).exists() {
                    let hash = disk(&self.root, &i.baseline.relative_path)?;
                    if hash.as_ref() != Some(&i.baseline.content_hash)
                        && hash.as_ref() != Some(&new_hash)
                    {
                        return Err(conflict());
                    }
                    fs::remove_file(self.root.join(&i.baseline.relative_path))
                        .map_err(|_| corrupt())?;
                }
                if same && self.take_failpoint(Failpoint::RenameSourceRemoved) {
                    return Err(corrupt());
                }
                if disk(&self.root, &i.destination)?.is_none() {
                    fs::hard_link(self.root.join(&stage), self.root.join(&i.destination))
                        .map_err(|_| conflict())?;
                } else if disk(&self.root, &i.destination)?.as_ref() != Some(&new_hash) {
                    return Err(conflict());
                }
                if self.take_failpoint(Failpoint::RenamePublished) {
                    return Err(corrupt());
                }
                if !same && self.root.join(&i.baseline.relative_path).exists() {
                    if disk(&self.root, &i.baseline.relative_path)?.as_ref()
                        != Some(&i.baseline.content_hash)
                    {
                        return Err(conflict());
                    }
                    fs::remove_file(self.root.join(&i.baseline.relative_path))
                        .map_err(|_| corrupt())?;
                }
            } else if disk(&self.root, &i.destination)?.as_ref() != Some(&new_hash) {
                if disk(&self.root, &i.destination)?.as_ref() != Some(&i.baseline.content_hash) {
                    return Err(conflict());
                }
                fs::rename(self.root.join(&stage), self.root.join(&i.destination))
                    .map_err(|_| corrupt())?;
            }
            if self.take_failpoint(Failpoint::AfterReplace)
                || self.take_failpoint(Failpoint::BeforeDbCommit)
            {
                return Err(corrupt());
            }
            if disk(&self.root, &i.destination)?.as_ref() != Some(&new_hash) {
                return Err(conflict());
            }
            let mut next = a.clone();
            next.completed += 1;
            next.publishing = false;
            let tx = conn
                .unchecked_transaction()
                .map_err(crate::errors::database)?;
            tx.execute("DELETE FROM notes WHERE note_id=?1", [&i.baseline.note_id])
                .map_err(crate::errors::database)?;
            state::persist_notes(
                &tx,
                &[state::NoteRecord {
                    note_id: i.baseline.note_id.clone(),
                    relative_path: i.destination.clone(),
                    content_hash: new_hash,
                    local_revision: i.baseline.expected_revision + 1,
                    read_only: false,
                    id_embedded: i.embedded,
                }],
            )?;
            write(&tx, &next, false)?;
            tx.commit().map_err(crate::errors::database)?;
            *a = next;
            *self.notes.borrow_mut() = state::load_notes(conn)?;
            if disk(&self.root, &stage)?.as_deref()
                == Some(scan::hex_sha256(i.after.as_bytes()).as_str())
            {
                fs::remove_file(self.root.join(&stage)).map_err(|_| corrupt())?;
            }
            if self.take_failpoint(Failpoint::RenameItemCommitted) {
                return Err(corrupt());
            }
        }
        Ok(())
    }

    pub(crate) fn recover_renames(&self, conn: &Connection) -> SeaRes<crate::RecoverOutcome> {
        let mut outcome = crate::RecoverOutcome {
            applied: Vec::new(),
            pending: Vec::new(),
        };
        for id in pending(conn)? {
            match load(conn, &id) {
                Ok(Some(mut a)) => {
                    if self.execute_rename(conn, &mut a).is_ok() {
                        outcome.applied.push(a.plan.note_id);
                    } else {
                        outcome.pending.push(a.plan.note_id);
                    }
                }
                _ => outcome.pending.push(id),
            }
        }
        Ok(outcome)
    }
}
