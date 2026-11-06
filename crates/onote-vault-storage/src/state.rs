use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use onote_domain::{ErrorCode, NoteSummary, Vault};

use crate::errors::{self, SeaRes};
use crate::gate;
use crate::scan::{self, ScannedNote};

#[path = "legacy_open.rs"]
pub(crate) mod legacy_open;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteRecord {
    pub note_id: String,
    pub relative_path: String,
    pub content_hash: String,
    pub local_revision: u64,
    pub read_only: bool,
    pub id_embedded: bool,
}

pub(crate) fn summary(record: &NoteRecord, vault_id: &str) -> NoteSummary {
    NoteSummary {
        note_id: record.note_id.clone(),
        vault_id: vault_id.to_owned(),
        relative_path: record.relative_path.clone(),
        content_hash: record.content_hash.clone(),
        local_revision: record.local_revision,
        read_only: record.read_only,
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct VaultMeta {
    vault_id: String,
    schema_version: u32,
    display_name: String,
    sync_enabled: bool,
    writable: bool,
}

pub(crate) const SCHEMA_VERSION: u32 = 3;

impl VaultMeta {
    fn fresh(display_name: String) -> VaultMeta {
        VaultMeta {
            vault_id: Uuid::new_v4().to_string(),
            schema_version: SCHEMA_VERSION,
            display_name,
            sync_enabled: false,
            writable: true,
        }
    }
}

pub(crate) struct OpenedVault {
    pub root: PathBuf,
    pub conn: Option<Connection>,
    pub vault_info: Vault,
    pub notes: Vec<NoteRecord>,
    pub fatal_findings: usize,
    pub failpoints: std::cell::RefCell<Vec<crate::Failpoint>>,
}

pub(crate) fn initialize(root: PathBuf, display_name: Option<&str>) -> SeaRes<OpenedVault> {
    let fresh = display_name.is_some();
    let app_dir = root.join(".opennote");
    if fresh {
        if display_name.unwrap().trim().is_empty() {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.invalidDisplayName",
                &[],
            ));
        }
        if app_dir.exists() {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.vaultAlreadyInitialized",
                &[],
            ));
        }
    } else if !root.is_dir() {
        return Err(errors::err(
            ErrorCode::ObjectNotFound,
            "error.vaultRootMissing",
            &[],
        ));
    }

    if !fresh {
        if crate::upgrade::requires_readonly(&root).unwrap_or(true) {
            return legacy_open::inspect(&root, true)?
                .ok_or_else(|| errors::err(ErrorCode::IoDenied, "error.vaultMetaInvalid", &[]));
        }
        if let Some(opened) = legacy_open::open_if_incompatible(&root)? {
            return Ok(opened);
        }
    }

    std::fs::create_dir_all(&root).map_err(|e| errors::io(&e, "error.vaultRootUnavailable"))?;

    if scan::is_reparse(&app_dir) {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.opennoteReparse",
            &[],
        ));
    }
    fs::create_dir_all(&app_dir).map_err(|e| errors::io(&e, "error.opennoteUnavailable"))?;

    // Cross-process writer gate: held across the whole initialize scan/DB
    // protocol so overlapping opens of the same vault (any alias path)
    // cannot interleave their persistence. No nested re-acquisition happens
    // inside initialize or the write operations that follow it.
    let gate_arc = gate::acquire(&root)?;
    let _gate_guard = gate::lock(&gate_arc)?;

    let meta = ensure_meta(&root, &app_dir, display_name)?;

    if crate::upgrade::requires_readonly(&root).unwrap_or(true) {
        return legacy_open::inspect(&root, true)?
            .ok_or_else(|| errors::err(ErrorCode::IoDenied, "error.vaultMetaInvalid", &[]));
    }

    if meta.schema_version != SCHEMA_VERSION {
        return legacy_open::open_if_incompatible(&root)?
            .ok_or_else(|| errors::err(ErrorCode::VersionConflict, "error.vaultMetaChanged", &[]));
    }
    let scanned = scan::scan_vault(&root)?;
    let mut findings = scanned.findings;
    let fatal_findings = findings
        .iter()
        .filter(|f| f.code == "DuplicateNoteId")
        .count();

    let conn = open_db(&app_dir)?;
    init_schema(&conn)?;
    let prior = load_notes(&conn)?;
    if !crate::rename::pending(&conn)?.is_empty() {
        // The multi-file journal owns catalog reconciliation until explicit recovery.
        // In particular, a moved source may be absent or temporarily have two paths.
        crate::rename::reconcile_findings(
            &conn,
            &root,
            &meta.vault_id,
            &scanned.notes,
            &mut findings,
        )?;
        return Ok(OpenedVault {
            root,
            conn: Some(conn),
            notes: prior,
            fatal_findings: findings
                .iter()
                .filter(|f| f.code == "DuplicateNoteId")
                .count(),
            vault_info: Vault {
                vault_id: meta.vault_id,
                schema_version: meta.schema_version,
                display_name: meta.display_name,
                sync_enabled: false,
                writable: meta.writable && findings.is_empty(),
                findings,
            },
            failpoints: std::cell::RefCell::new(Vec::new()),
        });
    }

    // Journal reconciliation: prepared operation rows own their note paths
    // until explicit recover() runs. Scanning a replaced target must never
    // bump the revision or overwrite the authoritative prior baseline
    // (AfterReplace/BeforeDbCommit left replacement bytes on disk while the
    // DB still holds the pre-operation row), and a pending CREATE must not
    // acquire a competing provisional note mapping from the scan.
    let prepared = crate::ops::list_prepared(&conn)?;
    let mut pending_paths: HashSet<String> =
        prepared.iter().map(|p| p.note_relative.clone()).collect();
    let mut pending_ids: HashSet<String> = prepared.iter().map(|p| p.note_id.clone()).collect();
    let (trash_paths, trash_ids) = crate::trash::protected(&conn)?;
    pending_paths.extend(trash_paths);
    pending_ids.extend(trash_ids);
    let deleted = crate::trash::tombstones(&conn)?;

    let notes;
    if prepared.is_empty() && pending_paths.is_empty() && pending_ids.is_empty() {
        let deleted_ids = deleted.iter().map(|entry| entry.note_id.clone()).collect();
        let plan = crate::catalog::reconcile(&prior, &scanned.notes, &deleted_ids, findings)?;
        if meta.writable {
            crate::catalog::persist(&conn, &plan)?;
        }
        let mut observed = plan.records;
        if !meta.writable {
            for note in &mut observed {
                note.read_only = true;
                if let Some(previous) = prior.iter().find(|p| p.note_id == note.note_id) {
                    note.local_revision = previous.local_revision;
                }
            }
        }
        notes = observed;
        findings = plan.findings;
    } else {
        let mut protected_notes: Vec<NoteRecord> = Vec::with_capacity(scanned.notes.len());
        for s in scanned.notes {
            if s.embedded_id.as_ref().is_some_and(|id| {
                deleted.iter().any(|entry| &entry.note_id == id) && !pending_ids.contains(id)
            }) {
                findings.push(onote_domain::VaultFinding {
                    code: "DeletedNoteReappeared".into(),
                    target: Some(s.relative_path),
                    message_key: "finding.deletedNoteReappeared".into(),
                });
                continue;
            }
            let is_pending = pending_paths.contains(&s.relative_path)
                || matches!(&s.embedded_id, Some(id) if pending_ids.contains(id));
            if !is_pending {
                protected_notes.push(record_for(&prior, s));
                continue;
            }
            match prior.iter().find(|p| p.relative_path == s.relative_path) {
                // Authoritative prior DB baseline preserved verbatim until recover.
                Some(p) => protected_notes.push(p.clone()),
                // No baseline to preserve: never acquire a provisional mapping
                // from scan bytes that a pending journal row still owns.
                None => continue,
            }
        }
        persist_notes(&conn, &protected_notes)?;
        notes = protected_notes;
    }

    let writable = meta.writable
        && findings
            .iter()
            .all(|finding| finding.code.starts_with("Catalog"));
    Ok(OpenedVault {
        root,
        conn: Some(conn),
        vault_info: Vault {
            vault_id: meta.vault_id.clone(),
            schema_version: meta.schema_version,
            display_name: meta.display_name.clone(),
            sync_enabled: false,
            writable,
            findings,
        },
        notes,
        fatal_findings,
        failpoints: std::cell::RefCell::new(Vec::new()),
    })
}

fn ensure_meta(root: &Path, app_dir: &Path, display_name: Option<&str>) -> SeaRes<VaultMeta> {
    let meta_path = app_dir.join("vault.json");
    refuse_reparse(&meta_path, "error.metaReparse")?;
    let existing = match fs::read(&meta_path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(errors::io(&e, "error.vaultMetaReadDenied")),
    };
    match existing {
        Some(bytes) => match serde_json::from_slice::<VaultMeta>(&bytes) {
            Ok(meta) => Ok(meta),
            Err(_) => Err(errors::err(
                ErrorCode::InvalidInput,
                "error.vaultMetaInvalid",
                &[],
            )),
        },
        None => {
            let display = match display_name {
                Some(name) => name.trim().to_owned(),
                None => root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Vault".to_owned()),
            };
            let fresh = VaultMeta::fresh(display);
            write_meta(&meta_path, &fresh)?;
            Ok(fresh)
        }
    }
}

fn write_meta(meta_path: &Path, meta: &VaultMeta) -> SeaRes<()> {
    let mut file =
        fs::File::create(meta_path).map_err(|e| errors::io(&e, "error.vaultMetaWrite"))?;
    serde_json::to_writer_pretty(&mut file, meta)
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.vaultMetaWrite", &[]))?;
    file.sync_all()
        .map_err(|e| errors::io(&e, "error.vaultMetaWrite"))
}

pub(crate) fn patch_meta(app_dir: &Path, fields: &[(&str, serde_json::Value)]) -> SeaRes<()> {
    use std::io::Write;
    let path = app_dir.join("vault.json");
    refuse_reparse(&path, "error.metaReparse")?;
    let before = fs::read(&path).map_err(|e| errors::io(&e, "error.vaultMetaReadDenied"))?;
    let mut value: serde_json::Value = serde_json::from_slice(&before)
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.vaultMetaInvalid", &[]))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| errors::err(ErrorCode::IoDenied, "error.vaultMetaInvalid", &[]))?;
    for (name, value) in fields {
        object.insert((*name).into(), value.clone());
    }
    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.vaultMetaWrite", &[]))?;
    let stage = app_dir.join(format!(".vault-meta-{}", Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stage)
        .map_err(|e| errors::io(&e, "error.vaultMetaWrite"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| errors::io(&e, "error.vaultMetaWrite"))?;
    drop(file);
    refuse_reparse(&path, "error.metaReparse")?;
    if fs::read(&path).map_err(|e| errors::io(&e, "error.vaultMetaReadDenied"))? != before {
        return Err(errors::err(
            ErrorCode::VersionConflict,
            "error.vaultMetaChanged",
            &[],
        ));
    }
    fs::rename(stage, path).map_err(|e| errors::io(&e, "error.vaultMetaWrite"))
}

fn refuse_reparse(path: &Path, key: &str) -> SeaRes<()> {
    if scan::is_reparse(path) {
        return Err(errors::err(ErrorCode::IoDenied, key, &[]));
    }
    Ok(())
}

pub(crate) fn open_db(app_dir: &Path) -> SeaRes<Connection> {
    let base = app_dir.join("state.sqlite");
    for path in [
        base.clone(),
        app_dir.join("state.sqlite-wal"),
        app_dir.join("state.sqlite-shm"),
    ] {
        if path.exists() {
            refuse_reparse(&path, "error.stateReparse")?;
        }
    }
    Connection::open(&base)
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateOpenFailed", &[]))
}

pub(crate) fn init_schema(conn: &Connection) -> SeaRes<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         CREATE TABLE IF NOT EXISTS notes (
             relative_path TEXT PRIMARY KEY,
             note_id TEXT NOT NULL,
             content_hash TEXT NOT NULL,
             local_revision INTEGER NOT NULL,
             read_only INTEGER NOT NULL,
             id_embedded INTEGER NOT NULL
         );",
    )
    .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))?;
    migrate_operations_table(conn)?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_ops_note ON operations(note_id);
         CREATE INDEX IF NOT EXISTS idx_ops_digest ON operations(request_digest);",
    )
    .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))?;
    crate::drafts::ensure_draft_checkpoints_table(conn)?;
    crate::trash::init(conn)?;
    crate::snapshot::init(conn)?;
    crate::rename::init(conn)?;
    crate::tag_batches::init(conn)?;
    Ok(())
}

/// Additive preservation migration: the pre-hardening `operations` table was
/// keyed by `op_digest` (PRIMARY KEY) and wrote rows with INSERT OR REPLACE,
/// which could erase a different operation sharing the same request. The
/// current shape keys rows by `operation_id` (PRIMARY KEY) and stores the
/// request digest as a NON-unique column. Existing rows are renamed, copied
/// and preserved atomically; a failed migration leaves old bytes untouched.
fn migrate_operations_table(conn: &Connection) -> SeaRes<()> {
    let columns = conn
        .prepare("PRAGMA table_info(operations)")
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))?
        .filter_map(|r| r.ok())
        .collect::<Vec<String>>();
    if columns.is_empty() {
        // fresh table
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS operations (
                operation_id TEXT PRIMARY KEY,
                request_digest TEXT NOT NULL,
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
            );",
        )
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))?;
        return Ok(());
    }
    let has_current = columns.iter().any(|c| c == "operation_id")
        && columns.iter().any(|c| c == "request_digest");
    if has_current {
        return Ok(());
    }
    let has_old = columns.iter().any(|c| c == "op_digest");
    if !has_old {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.stateInitFailed",
            &[],
        ));
    }
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))?;
    let step = || -> SeaRes<()> {
        conn.execute_batch(
            "ALTER TABLE operations RENAME TO operations_pre1;
             DROP INDEX IF EXISTS idx_ops_note;
             DROP INDEX IF EXISTS idx_ops_opid;
             DROP INDEX IF EXISTS idx_ops_digest;
             CREATE TABLE operations (
                 operation_id TEXT PRIMARY KEY,
                 request_digest TEXT NOT NULL,
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
             INSERT INTO operations (operation_id, request_digest, note_id, status, kind, note_relative, old_revision, new_revision, old_hash, new_hash, stage_relative, old_relative, created_at)
             SELECT operation_id, op_digest, note_id, status, kind, note_relative, old_revision, new_revision, old_hash, new_hash, stage_relative, old_relative, created_at FROM operations_pre1;
             DROP TABLE operations_pre1;",
        )
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))
    };
    match step() {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[])),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

pub(crate) fn load_notes(conn: &Connection) -> SeaRes<Vec<NoteRecord>> {
    let mut stmt = conn
        .prepare("SELECT relative_path, note_id, content_hash, local_revision, read_only, id_embedded FROM notes")
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateReadFailed", &[]))?;
    let map = |row: &rusqlite::Row| -> Result<NoteRecord, rusqlite::Error> {
        Ok(NoteRecord {
            relative_path: row.get(0)?,
            note_id: row.get(1)?,
            content_hash: row.get(2)?,
            local_revision: row.get::<_, i64>(3)? as u64,
            read_only: row.get::<_, i64>(4)? != 0,
            id_embedded: row.get::<_, i64>(5)? != 0,
        })
    };
    let rows = stmt
        .query_map([], map)
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateReadFailed", &[]))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateReadFailed", &[]))?);
    }
    Ok(out)
}

pub(crate) fn persist_notes(conn: &Connection, notes: &[NoteRecord]) -> SeaRes<()> {
    let mut stmt = conn
        .prepare("INSERT INTO notes (relative_path, note_id, content_hash, local_revision, read_only, id_embedded) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
            ON CONFLICT(relative_path) DO UPDATE SET note_id=excluded.note_id, content_hash=excluded.content_hash, local_revision=excluded.local_revision, read_only=excluded.read_only, id_embedded=excluded.id_embedded \
            WHERE notes.note_id IS NOT excluded.note_id OR notes.content_hash IS NOT excluded.content_hash OR notes.local_revision IS NOT excluded.local_revision OR notes.read_only IS NOT excluded.read_only OR notes.id_embedded IS NOT excluded.id_embedded")
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateWriteFailed", &[]))?;
    for note in notes {
        stmt.execute(rusqlite::params![
            note.relative_path,
            note.note_id,
            note.content_hash,
            note.local_revision as i64,
            note.read_only as i64,
            note.id_embedded as i64,
        ])
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateWriteFailed", &[]))?;
    }
    Ok(())
}

fn record_for(prior: &[NoteRecord], scanned: ScannedNote) -> NoteRecord {
    let prior = prior
        .iter()
        .find(|p| p.relative_path == scanned.relative_path);
    // Stable identity: reuse the stored mapping for every unchanged legacy
    // path; the embedded id (when present on disk now) is authoritative.
    let note_id = match &scanned.embedded_id {
        Some(id) => id.clone(),
        None => prior
            .map(|p| p.note_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
    };
    // A real external hash change bumps the local revision; content-identical
    // rescans keep revision and ID untouched.
    let local_revision = match prior {
        Some(p) => {
            if p.content_hash != scanned.content_hash {
                p.local_revision + 1
            } else {
                p.local_revision
            }
        }
        None => 0,
    };
    NoteRecord {
        note_id,
        relative_path: scanned.relative_path,
        content_hash: scanned.content_hash,
        local_revision,
        read_only: scanned.read_only,
        id_embedded: scanned.embedded_id.is_some(),
    }
}

/// Read a stored relative path under `root`, refusing traversal, absolute
/// paths, reparse-point parents and vanished files. No path data in errors.
pub(crate) fn guarded_read(root: &Path, relative_path: &str) -> SeaRes<Vec<u8>> {
    let relative = relative_path.replace('\\', "/");
    if relative.is_empty()
        || relative.starts_with('/')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
    {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.pathEscape",
            &[],
        ));
    }
    let mut current = root.to_path_buf();
    let mut parts = relative.split('/').collect::<Vec<_>>();
    let file_name = parts.pop().expect("nonempty checked");
    for part in parts {
        current.push(part);
        if scan::is_reparse(&current) {
            return Err(errors::err(ErrorCode::IoDenied, "error.pathReparse", &[]));
        }
        if !current.is_dir() {
            return Err(errors::err(
                ErrorCode::ObjectNotFound,
                "error.noteFileMissing",
                &[],
            ));
        }
    }
    current.push(file_name);
    if scan::is_reparse(&current) {
        return Err(errors::err(ErrorCode::IoDenied, "error.pathReparse", &[]));
    }
    match fs::read(&current) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(errors::err(
            ErrorCode::ObjectNotFound,
            "error.noteFileMissing",
            &[],
        )),
        Err(e) => Err(errors::io(&e, "error.noteReadDenied")),
    }
}

/// Create any missing ancestor dirs between `root` and `parent_abs`, refusing
/// reparse points along the chain (no link-following directory creation).
pub(crate) fn ensure_writable_dir_parents(root: &Path, parent_abs: &Path) -> SeaRes<()> {
    let rel = parent_abs
        .strip_prefix(root)
        .map_err(|_| errors::err(ErrorCode::InvalidInput, "error.pathEscape", &[]))?;
    let mut current = root.to_path_buf();
    for part in rel.components() {
        current.push(part.as_os_str());
        if is_reparse_dir(&current) {
            return Err(errors::err(ErrorCode::IoDenied, "error.pathReparse", &[]));
        }
        if !current.exists() {
            fs::create_dir(&current).map_err(|e| errors::io(&e, "error.dirCreate"))?;
        } else if !current.is_dir() {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.pathNotDir",
                &[],
            ));
        }
    }
    Ok(())
}

fn is_reparse_dir(path: &std::path::Path) -> bool {
    scan::is_reparse(path)
}
