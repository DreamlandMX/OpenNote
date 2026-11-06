use crate::{
    commit,
    errors::{err, SeaRes},
    gate, ops, scan, Failpoint, VaultStorage,
};
use onote_domain::{ErrorCode, SaveResult, TagBatchItemResult, TagBatchResult, TagWritePlan};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::PathBuf};

fn invalid() -> onote_domain::ServiceError {
    err(ErrorCode::IoDenied, "error.tagBatchInvalid", &[])
}
fn mismatch() -> onote_domain::ServiceError {
    err(
        ErrorCode::IdempotencyMismatch,
        "error.idempotencyMismatch",
        &[],
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    operation_id: String,
    root: PathBuf,
    plan: TagWritePlan,
    children: Vec<String>,
    cancelled: bool,
}

pub(crate) fn init(conn: &Connection) -> SeaRes<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS tag_batches(operation_id TEXT PRIMARY KEY, payload TEXT NOT NULL, checksum TEXT NOT NULL, cancelled INTEGER NOT NULL, cancellation_hash TEXT NOT NULL);")
        .map_err(crate::errors::database)
}
pub(crate) fn refuse_operation_reuse(conn: &Connection, id: &str) -> SeaRes<()> {
    if conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tag_batches WHERE operation_id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(crate::errors::database)?
    {
        return Err(mismatch());
    }
    Ok(())
}
fn validate(plan: &TagWritePlan) -> SeaRes<()> {
    if !commit::is_canonical_uuid(&plan.plan_id)
        || !commit::is_canonical_uuid(&plan.vault_id)
        || !(1..=64).contains(&plan.old_tag.chars().count())
        || plan.old_tag.trim() != plan.old_tag
        || plan.new_tag.as_ref().is_some_and(|tag| {
            tag.trim() != tag || !(1..=64).contains(&tag.chars().count()) || tag == &plan.old_tag
        })
        || (plan.new_tag.is_none() && plan.is_merge)
        || plan.items.is_empty()
    {
        return Err(invalid());
    }
    let mut ids = HashSet::new();
    let mut paths = Vec::new();
    for item in &plan.items {
        if !commit::is_canonical_uuid(&item.note_id)
            || !ids.insert(&item.note_id)
            || crate::validate_new_note_path(&item.relative_path)? != item.relative_path
            || paths
                .iter()
                .any(|p: &&str| crate::compare_windows_paths(p, &item.relative_path).is_eq())
            || item.expected_revision >= 9_007_199_254_740_991
            || item.content_hash.len() != 64
            || !item
                .content_hash
                .bytes()
                .all(|b| matches!(b,b'0'..=b'9'|b'a'..=b'f'))
        {
            return Err(invalid());
        }
        paths.push(item.relative_path.as_str());
    }
    Ok(())
}
fn load(conn: &Connection, id: &str) -> SeaRes<Option<Request>> {
    let row:Option<(String,String,i64,String)>=conn.query_row("SELECT payload,checksum,cancelled,cancellation_hash FROM tag_batches WHERE operation_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(crate::errors::database)?;
    let Some((payload, checksum, cancelled, cancellation_hash)) = row else {
        return Ok(None);
    };
    if scan::hex_sha256(payload.as_bytes()) != checksum {
        return Err(invalid());
    }
    let mut request: Request = serde_json::from_str(&payload).map_err(|_| invalid())?;
    validate(&request.plan)?;
    if request.cancelled
        || !matches!(cancelled, 0 | 1)
        || cancellation_hash != cancel_hash(id, cancelled)
    {
        return Err(invalid());
    }
    request.cancelled = cancelled == 1;
    let mut ids = HashSet::new();
    if request.version != 1
        || request.operation_id != id
        || !commit::is_canonical_uuid(id)
        || !request.root.is_absolute()
        || request.children.len() != request.plan.items.len()
        || request
            .children
            .iter()
            .any(|child| !commit::is_canonical_uuid(child) || child == id || !ids.insert(child))
    {
        return Err(invalid());
    }
    Ok(Some(request))
}
fn cancel_hash(id: &str, cancelled: i64) -> String {
    scan::hex_sha256(format!("tag-batch-cancel-v1:{id}:{cancelled}").as_bytes())
}
fn store(conn: &Connection, request: &Request) -> SeaRes<()> {
    let payload = serde_json::to_string(request).map_err(|_| invalid())?;
    let checksum = scan::hex_sha256(payload.as_bytes());
    conn.execute("INSERT INTO tag_batches(operation_id,payload,checksum,cancelled,cancellation_hash) VALUES (?1,?2,?3,0,?4)",
        rusqlite::params![request.operation_id,payload,checksum,cancel_hash(&request.operation_id,0)]).map_err(crate::errors::database)?;
    Ok(())
}
fn control(conn: &Connection, id: &str, expected_checksum: &str) -> SeaRes<bool> {
    let (checksum, cancelled, cancellation_hash): (String, i64, String) = conn
        .query_row(
            "SELECT checksum,cancelled,cancellation_hash FROM tag_batches WHERE operation_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(crate::errors::database)?;
    if checksum != expected_checksum
        || !matches!(cancelled, 0 | 1)
        || cancellation_hash != cancel_hash(id, cancelled)
    {
        return Err(invalid());
    }
    Ok(cancelled == 1)
}
fn context(request: &Request, index: usize) -> String {
    let item = &request.plan.items[index];
    format!(
        "tag-update:{}:{}:{}",
        request.operation_id, item.content_hash, item.relative_path
    )
}
fn item_result(conn: &Connection, request: &Request, index: usize) -> SeaRes<TagBatchItemResult> {
    let item = &request.plan.items[index];
    let mut saved = None;
    if let Some(operation) = ops::find_by_operation_id(conn, &request.children[index])? {
        if !commit::validate_record(&operation)
            || operation.kind != "save"
            || operation.note_id != item.note_id
            || operation.note_relative != item.relative_path
            || operation.old_revision != Some(item.expected_revision)
            || operation.old_hash.as_deref() != Some(item.content_hash.as_str())
            || operation.request_digest
                != ops::digest(
                    &context(request, index),
                    &item.relative_path,
                    &item.source,
                    Some(item.expected_revision),
                    Some(&item.note_id),
                )
        {
            return Err(invalid());
        }
        if operation.status == "committed" {
            saved = Some(SaveResult {
                note_id: item.note_id.clone(),
                local_revision: operation.new_revision.ok_or_else(invalid)?,
                content_hash: operation.new_hash,
            });
        } else if operation.status != "prepared" {
            return Err(invalid());
        }
    }
    Ok(TagBatchItemResult {
        note_id: item.note_id.clone(),
        relative_path: item.relative_path.clone(),
        result: saved,
        error: None,
    })
}
fn result(conn: &Connection, request: &Request) -> SeaRes<TagBatchResult> {
    let items = (0..request.plan.items.len())
        .map(|index| item_result(conn, request, index))
        .collect::<SeaRes<Vec<_>>>()?;
    Ok(TagBatchResult {
        operation_id: request.operation_id.clone(),
        old_tag: request.plan.old_tag.clone(),
        new_tag: request.plan.new_tag.clone(),
        is_merge: request.plan.is_merge,
        completed: items.iter().all(|i| i.result.is_some()),
        cancelled: request.cancelled,
        items,
    })
}

impl VaultStorage {
    fn tag_request(&self, id: &str) -> SeaRes<Request> {
        Self::require_operation_id(id)?;
        let conn = self.require_writable()?;
        let gate = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate)?;
        let request = load(conn, id)?
            .ok_or_else(|| err(ErrorCode::ObjectNotFound, "error.tagBatchMissing", &[]))?;
        if request.root != fs::canonicalize(&self.root).map_err(|_| invalid())?
            || request.plan.vault_id != self.vault_info.vault_id
        {
            return Err(mismatch());
        }
        Ok(request)
    }
    pub fn apply_tag_update(
        &mut self,
        plan: &TagWritePlan,
        operation_id: &str,
    ) -> SeaRes<TagBatchResult> {
        self.apply_tag_update_with_cancel(plan, operation_id, || false)
    }
    pub fn apply_tag_update_with_cancel(
        &mut self,
        plan: &TagWritePlan,
        operation_id: &str,
        cancel: impl FnMut() -> bool,
    ) -> SeaRes<TagBatchResult> {
        Self::require_operation_id(operation_id)?;
        validate(plan)?;
        let conn = self.require_writable()?;
        let exists = {
            let gate = gate::acquire(&self.root)?;
            let _guard = gate::lock(&gate)?;
            load(conn, operation_id)?.is_some()
        };
        if exists {
            if self.tag_request(operation_id)?.plan != *plan {
                return Err(mismatch());
            }
        } else {
            self.with_history_write(|| {
                if plan.vault_id != self.vault_info.vault_id {
                    return Err(mismatch());
                }
                // A second caller may have published the same parent while this caller waited.
                if let Some(request) = load(conn, operation_id)? {
                    if request.plan != *plan {
                        return Err(mismatch());
                    }
                    return Ok(());
                }
                crate::trash::refuse_operation_reuse(conn, operation_id)?;
                if ops::find_by_operation_id(conn, operation_id)?.is_some()
                    || conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM draft_checkpoints WHERE draft_id=?1)",
                            [operation_id],
                            |r| r.get::<_, bool>(0),
                        )
                        .map_err(crate::errors::database)?
                {
                    return Err(mismatch());
                }
                let children = plan
                    .items
                    .iter()
                    .map(|_| uuid::Uuid::new_v4().to_string())
                    .collect();
                store(
                    conn,
                    &Request {
                        version: 1,
                        operation_id: operation_id.into(),
                        root: fs::canonicalize(&self.root).map_err(|_| invalid())?,
                        plan: plan.clone(),
                        children,
                        cancelled: false,
                    },
                )
            })?;
        }
        self.resume_tag_update_with_cancel(operation_id, cancel)
    }
    pub fn resume_tag_update(&mut self, operation_id: &str) -> SeaRes<TagBatchResult> {
        self.resume_tag_update_with_cancel(operation_id, || false)
    }
    pub fn resume_tag_update_with_cancel(
        &mut self,
        operation_id: &str,
        mut cancel: impl FnMut() -> bool,
    ) -> SeaRes<TagBatchResult> {
        let mut request = self.tag_request(operation_id)?;
        let was_cancelled = request.cancelled;
        request.cancelled = false;
        let checksum = scan::hex_sha256(&serde_json::to_vec(&request).map_err(|_| invalid())?);
        request.cancelled = was_cancelled;
        let mut failures = Vec::new();
        for (index, item) in request.plan.items.iter().enumerate() {
            if cancel() {
                self.cancel_tag_update(operation_id)?;
                break;
            }
            let existing = {
                let conn = self.require_writable()?;
                let gate = gate::acquire(&self.root)?;
                let _guard = gate::lock(&gate)?;
                if control(conn, operation_id, &checksum)? {
                    break;
                }
                item_result(conn, &request, index)?
            };
            if existing.result.is_some() {
                continue;
            }
            let saved = self.save_with_context(
                &item.note_id,
                &item.source,
                item.expected_revision,
                &request.children[index],
                &context(&request, index),
                Some((&item.relative_path, &item.content_hash)),
            );
            if let Err(mut error) = saved {
                error.operation_id = Some(operation_id.into());
                failures.push((index, error));
            } else if self.take_failpoint(Failpoint::TagBatchItemCommitted) {
                break;
            }
        }
        let request = self.tag_request(operation_id)?;
        let conn = self.require_writable()?;
        let gate = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate)?;
        let mut outcome = result(conn, &request)?;
        for (index, error) in failures {
            if outcome.items[index].result.is_none() {
                outcome.items[index].error = Some(error)
            }
        }
        Ok(outcome)
    }
    pub fn cancel_tag_update(&self, operation_id: &str) -> SeaRes<TagBatchResult> {
        self.tag_request(operation_id)?;
        self.with_history_write(|| {
            let conn = self.require_writable()?;
            let mut request = load(conn, operation_id)?.ok_or_else(invalid)?;
            if !result(conn, &request)?.completed {
                request.cancelled = true;
                conn.execute(
                    "UPDATE tag_batches SET cancelled=1,cancellation_hash=?2 WHERE operation_id=?1",
                    rusqlite::params![operation_id, cancel_hash(operation_id, 1)],
                )
                .map_err(crate::errors::database)?;
            }
            result(conn, &request)
        })
    }
    pub fn list_tag_updates(&self) -> SeaRes<Vec<TagBatchResult>> {
        let conn = self.require_writable()?;
        let gate = gate::acquire(&self.root)?;
        let _guard = gate::lock(&gate)?;
        let mut statement = conn
            .prepare("SELECT operation_id FROM tag_batches ORDER BY rowid DESC")
            .map_err(crate::errors::database)?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(crate::errors::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(crate::errors::database)?;
        let root = fs::canonicalize(&self.root).map_err(|_| invalid())?;
        let mut batches = Vec::new();
        for id in ids {
            let request = load(conn, &id)?.ok_or_else(invalid)?;
            // Receipts restored into a different root are historical, never resumable here.
            if request.root != root || request.plan.vault_id != self.vault_info.vault_id {
                continue;
            }
            let batch = result(conn, &request)?;
            if !batch.completed || batches.len() < 20 {
                batches.push(batch)
            }
        }
        Ok(batches)
    }
}
