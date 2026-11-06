use crate::{
    errors::{self, SeaRes},
    gate, scan, state, Failpoint, VaultStorage,
};
use onote_domain::{ErrorCode, NoteSummary, Vault};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaUpgradeResult {
    pub operation_id: String,
    pub manifest_hash: String,
    pub from_schema: u32,
    pub to_schema: u32,
    pub note_count: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    root: PathBuf,
    #[serde(default = "legacy_target", skip_serializing_if = "is_legacy_target")]
    to_schema: u32,
    vault: Vault,
    notes: Vec<NoteSummary>,
    metadata: Vec<u8>,
    operation_id: String,
    manifest_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    request: Request,
    checksum: String,
}

fn legacy_target() -> u32 {
    2
}
fn is_legacy_target(value: &u32) -> bool {
    *value == 2
}

fn failure() -> onote_domain::ServiceError {
    errors::err(ErrorCode::IoDenied, "error.upgradeInterrupted", &[])
}
fn mismatch() -> onote_domain::ServiceError {
    errors::err(
        ErrorCode::IdempotencyMismatch,
        "error.idempotencyMismatch",
        &[],
    )
}
fn bytes(request: &Request) -> SeaRes<Vec<u8>> {
    serde_json::to_vec(request).map_err(|_| failure())
}
fn directory(root: &Path, from_schema: u32) -> SeaRes<PathBuf> {
    let path = root.join(if from_schema == 1 {
        ".opennote/upgrade-v1-v2"
    } else {
        ".opennote/upgrade-v2-v3"
    });
    for parent in [root.join(".opennote"), path.clone()] {
        if scan::is_reparse(&parent) {
            return Err(failure());
        }
        match fs::symlink_metadata(parent) {
            Ok(meta) if !meta.is_dir() => return Err(failure()),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(failure()),
            _ => (),
        }
    }
    Ok(path)
}
fn read(root: &Path, path: &Path) -> SeaRes<Vec<u8>> {
    if scan::is_reparse(path) || fs::metadata(path).map_err(|_| failure())?.len() > 64 * 1024 * 1024
    {
        return Err(failure());
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| failure())?
        .to_str()
        .ok_or_else(failure)?
        .replace('\\', "/");
    state::guarded_read(root, &relative)
}
fn load(root: &Path, from_schema: u32) -> SeaRes<Option<Request>> {
    let path = directory(root, from_schema)?.join("request.json");
    if !path.exists() {
        return Ok(None);
    }
    let record: Record = serde_json::from_slice(&read(root, &path)?).map_err(|_| failure())?;
    if record.checksum != scan::hex_sha256(&bytes(&record.request)?)
        || record.request.vault.schema_version != from_schema
        || !matches!(
            (from_schema, record.request.to_schema),
            (1, 2) | (1, 3) | (2, 3)
        )
        || !crate::commit::is_canonical_uuid(&record.request.operation_id)
        || record.request.manifest_hash.len() != 64
        || !record
            .request
            .manifest_hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(failure());
    }
    Ok(Some(record.request))
}
fn complete(root: &Path, request: &Request) -> SeaRes<bool> {
    let path = directory(root, request.vault.schema_version)?.join("complete");
    if !path.exists() {
        return Ok(false);
    }
    if read(root, &path)? != scan::hex_sha256(&bytes(request)?).as_bytes() {
        return Err(failure());
    }
    Ok(true)
}
pub(crate) fn requires_readonly(root: &Path) -> SeaRes<bool> {
    let root = fs::canonicalize(root).map_err(|_| failure())?;
    for from in [1, 2] {
        if !directory(&root, from)?.exists() {
            continue;
        }
        match load(&root, from)? {
            Some(request) if complete(&root, &request)? => (),
            _ => return Ok(true),
        }
    }
    Ok(false)
}
fn publish(root: &Path, from_schema: u32, path: &Path, payload: &[u8]) -> SeaRes<()> {
    if payload.len() > 64 * 1024 * 1024 {
        return Err(failure());
    }
    let folder = directory(root, from_schema)?;
    fs::create_dir_all(&folder).map_err(|e| errors::io(&e, "error.upgradeInterrupted"))?;
    let stage = folder.join(format!(".stage-{}", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)
        .map_err(|e| errors::io(&e, "error.upgradeInterrupted"))?;
    file.write_all(payload)
        .and_then(|_| file.sync_all())
        .map_err(|e| errors::io(&e, "error.upgradeInterrupted"))?;
    drop(file);
    match fs::hard_link(&stage, path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && read(root, path)? == payload => {
            ()
        }
        Err(e) => return Err(errors::io(&e, "error.upgradeInterrupted")),
    }
    fs::remove_file(stage).map_err(|e| errors::io(&e, "error.upgradeInterrupted"))
}
fn result(request: &Request) -> SchemaUpgradeResult {
    SchemaUpgradeResult {
        operation_id: request.operation_id.clone(),
        manifest_hash: request.manifest_hash.clone(),
        from_schema: request.vault.schema_version,
        to_schema: request.to_schema,
        note_count: request.notes.len() as u64,
    }
}

impl VaultStorage {
    /// A pending M08 request may be abandoned only while this source is still
    /// schema 1 and no M02 migration request has been published.
    pub fn schema_upgrade_not_started(
        &self,
        operation_id: &str,
        manifest_hash: &str,
    ) -> SeaRes<bool> {
        Self::require_operation_id(operation_id)?;
        if manifest_hash.len() != 64
            || !manifest_hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(mismatch());
        }
        let root = fs::canonicalize(&self.root).map_err(|_| failure())?;
        let gate = gate::acquire(&root)?;
        let _guard = gate::lock(&gate)?;
        for from in [1, 2] {
            if let Some(request) = load(&root, from)? {
                if request.operation_id == operation_id {
                    if request.manifest_hash != manifest_hash
                        || request.vault.vault_id != self.vault_info.vault_id
                    {
                        return Err(mismatch());
                    }
                    return Ok(false);
                }
                if !complete(&root, &request)? {
                    return Err(mismatch());
                }
            }
        }
        let current: serde_json::Value =
            serde_json::from_slice(&state::guarded_read(&root, ".opennote/vault.json")?)
                .map_err(|_| failure())?;
        Ok(current.get("vaultId").and_then(|v| v.as_str())
            == Some(self.vault_info.vault_id.as_str())
            && current.get("schemaVersion").and_then(|v| v.as_u64())
                == Some(self.vault_info.schema_version as u64)
            && matches!(self.vault_info.schema_version, 1 | 2))
    }

    pub fn completed_schema_upgrade(
        &self,
        operation_id: &str,
        manifest_hash: &str,
    ) -> SeaRes<Option<SchemaUpgradeResult>> {
        Self::require_operation_id(operation_id)?;
        let root = fs::canonicalize(&self.root).map_err(|_| failure())?;
        let gate = gate::acquire(&root)?;
        let _guard = gate::lock(&gate)?;
        for from in [1, 2] {
            let Some(request) = load(&root, from)? else {
                continue;
            };
            if request.operation_id != operation_id {
                continue;
            }
            if request.manifest_hash != manifest_hash
                || request.vault.vault_id != self.vault_info.vault_id
            {
                return Err(mismatch());
            }
            return Ok(if complete(&root, &request)? {
                Some(result(&request))
            } else {
                None
            });
        }
        Ok(None)
    }
    /// M08 must verify the protected snapshot and source inside this callback.
    /// Resuming skips the original DB hash; non-DB files must still match.
    /// The callback holds the writer gate; do not call storage operations from it.
    /// M08 also owns backup retention and source-space checks before migration.
    pub fn apply_schema_upgrade(
        &self,
        database_target: &Path,
        operation_id: &str,
        manifest_hash: &str,
        verify: impl FnOnce(&Path, &Vault, &[NoteSummary], bool) -> SeaRes<()>,
    ) -> SeaRes<SchemaUpgradeResult> {
        self.apply_schema_upgrade_to(
            database_target,
            operation_id,
            manifest_hash,
            state::SCHEMA_VERSION,
            verify,
        )
    }

    pub fn apply_schema_upgrade_to(
        &self,
        database_target: &Path,
        operation_id: &str,
        manifest_hash: &str,
        to_schema: u32,
        verify: impl FnOnce(&Path, &Vault, &[NoteSummary], bool) -> SeaRes<()>,
    ) -> SeaRes<SchemaUpgradeResult> {
        Self::require_operation_id(operation_id)?;
        if manifest_hash.len() != 64
            || !manifest_hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(mismatch());
        }
        let root = fs::canonicalize(&self.root).map_err(|_| failure())?;
        let mut from_schema = self.vault_info.schema_version;
        for from in [1, 2] {
            if let Some(request) = load(&root, from)? {
                if request.operation_id == operation_id || !complete(&root, &request)? {
                    from_schema = from;
                    break;
                }
            }
        }
        if load(&root, from_schema)?.is_some() {
            let gate = gate::acquire(&root)?;
            let _guard = gate::lock(&gate)?;
            let request = load(&root, from_schema)?.ok_or_else(failure)?;
            if request.operation_id != operation_id
                || request.to_schema != to_schema
                || request.manifest_hash != manifest_hash
                || request.vault.vault_id != self.vault_info.vault_id
            {
                return Err(mismatch());
            }
            if complete(&root, &request)? {
                return Ok(result(&request));
            }
            if request.root != root {
                return Err(mismatch());
            }
            self.validate_upgrade_metadata(&root, &request)?;
            verify(&root, &request.vault, &request.notes, true)?;
            return self.finish_schema_upgrade(&root, &request);
        }
        self.with_snapshot_state(database_target, |root, vault, notes| {
            if !matches!((vault.schema_version, to_schema), (1, 2) | (1, 3) | (2, 3)) {
                return Err(failure());
            }
            if load(root, vault.schema_version)?.is_some() {
                return Err(mismatch());
            }
            verify(root, vault, notes, false)?;
            let conn = state::legacy_open::readonly_connection(root)?;
            for (table, column) in [
                ("operations", "operation_id"),
                ("draft_checkpoints", "draft_id"),
                ("trash_actions", "operation_id"),
                ("snapshot_restores", "operation_id"),
                ("rename_actions", "operation_id"),
                ("tag_batches", "operation_id"),
            ] {
                let exists: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                        [table],
                        |r| r.get(0),
                    )
                    .map_err(errors::database)?;
                if exists
                    && conn
                        .query_row(
                            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column}=?1)"),
                            [operation_id],
                            |r| r.get::<_, bool>(0),
                        )
                        .map_err(errors::database)?
                {
                    return Err(mismatch());
                }
            }
            let request = Request {
                root: root.into(),
                to_schema,
                vault: vault.clone(),
                notes: notes.to_vec(),
                metadata: state::guarded_read(root, ".opennote/vault.json")?,
                operation_id: operation_id.into(),
                manifest_hash: manifest_hash.into(),
            };
            let payload = serde_json::to_vec(&Record {
                checksum: scan::hex_sha256(&bytes(&request)?),
                request: request.clone(),
            })
            .map_err(|_| failure())?;
            publish(
                root,
                vault.schema_version,
                &directory(root, vault.schema_version)?.join("request.json"),
                &payload,
            )?;
            if self.take_failpoint(Failpoint::UpgradePrepared) {
                return Err(failure());
            }
            self.finish_schema_upgrade(root, &request)
        })
    }

    fn validate_upgrade_metadata(&self, root: &Path, request: &Request) -> SeaRes<()> {
        let current = state::guarded_read(root, ".opennote/vault.json")?;
        if current == request.metadata {
            return Ok(());
        }
        let mut expected: serde_json::Value =
            serde_json::from_slice(&request.metadata).map_err(|_| failure())?;
        expected["schemaVersion"] = request.to_schema.into();
        let value: serde_json::Value = serde_json::from_slice(&current).map_err(|_| failure())?;
        if value != expected {
            return Err(failure());
        }
        Ok(())
    }

    fn finish_schema_upgrade(&self, root: &Path, request: &Request) -> SeaRes<SchemaUpgradeResult> {
        self.validate_upgrade_metadata(root, request)?;
        // A missing authoritative database must never be replaced by an empty one.
        let original = state::legacy_open::readonly_connection(root)?;
        state::load_notes(&original)?;
        drop(original);
        let conn = state::open_db(&root.join(".opennote"))?;
        state::init_schema(&conn)?;
        let mut current = state::load_notes(&conn)?
            .iter()
            .map(|n| state::summary(n, &request.vault.vault_id))
            .collect::<Vec<_>>();
        let mut expected = request.notes.clone();
        current.sort_by(|a, b| a.note_id.cmp(&b.note_id));
        expected.sort_by(|a, b| a.note_id.cmp(&b.note_id));
        if current != expected {
            return Err(failure());
        }
        if self.take_failpoint(Failpoint::UpgradeDatabase) {
            return Err(failure());
        }
        state::patch_meta(
            &root.join(".opennote"),
            &[("schemaVersion", request.to_schema.into())],
        )?;
        if self.take_failpoint(Failpoint::UpgradeMetadata) {
            return Err(failure());
        }
        publish(
            root,
            request.vault.schema_version,
            &directory(root, request.vault.schema_version)?.join("complete"),
            scan::hex_sha256(&bytes(request)?).as_bytes(),
        )?;
        Ok(result(request))
    }
}
