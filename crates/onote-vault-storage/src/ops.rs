use sha2::{Digest, Sha256};

use onote_domain::ErrorCode;

use crate::errors::{self, SeaRes};

/// Durable operations keyed by unique UUID operationId; `request_digest`
/// holds the digest of the ORIGINAL caller request and is intentionally
/// NON-unique (distinct operations may carry identical requests).
#[derive(Debug, Clone)]
pub(crate) struct OperationRecord {
    pub request_digest: String,
    pub operation_id: String,
    pub status: String,
    pub kind: String,
    pub note_id: String,
    pub note_relative: String,
    pub old_revision: Option<u64>,
    pub new_revision: Option<u64>,
    pub old_hash: Option<String>,
    pub new_hash: String,
    pub stage_relative: String,
    pub old_relative: Option<String>,
    pub created_at: String,
}

const DIGEST_VERSION: &str = "v2";

/// Digest of the original caller request: kind, note path, FULL raw source
/// (hashed before nesting so separator bytes are never ambiguous), caller
/// expectedRevision and noteId. A retry of the same request yields the same
/// digest; any changed field yields a different one.
pub(crate) fn digest(
    kind: &str,
    note_relative: &str,
    raw_source: &str,
    expected_revision: Option<u64>,
    note_id: Option<&str>,
) -> String {
    let source_hash = hex::encode(Sha256::digest(raw_source.as_bytes()));
    let rev = expected_revision
        .map(|r| r.to_string())
        .unwrap_or_else(|| "-".into());
    let nid = match note_id {
        Some(id) => id,
        None => "-",
    };
    let input =
        format!("{DIGEST_VERSION}\x1f{kind}\x1f{note_relative}\x1f{source_hash}\x1f{rev}\x1f{nid}");
    hex::encode(Sha256::digest(input.as_bytes()))
}

pub(crate) fn now_stamp() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs().to_string(),
        Err(_) => "0".to_owned(),
    }
}

const SELECT: &str = "SELECT request_digest, operation_id, status, kind, note_id, note_relative, \
old_revision, new_revision, old_hash, new_hash, stage_relative, old_relative, created_at \
FROM operations";

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<OperationRecord> {
    Ok(OperationRecord {
        request_digest: row.get(0)?,
        operation_id: row.get(1)?,
        status: row.get(2)?,
        kind: row.get(3)?,
        note_id: row.get(4)?,
        note_relative: row.get(5)?,
        old_revision: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
        new_revision: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
        old_hash: row.get(8)?,
        new_hash: row.get(9)?,
        stage_relative: row.get(10)?,
        old_relative: row.get(11)?,
        created_at: row.get(12)?,
    })
}

pub(crate) fn db_err() -> onote_domain::ServiceError {
    errors::err(ErrorCode::IoDenied, "error.stateWriteFailed", &[])
}

pub(crate) fn find_by_operation_id(
    conn: &rusqlite::Connection,
    operation_id: &str,
) -> SeaRes<Option<OperationRecord>> {
    let mut stmt = conn
        .prepare(&format!("{SELECT} WHERE operation_id = ?1"))
        .map_err(|_| db_err())?;
    let mut rows = stmt
        .query_map([operation_id], row_to_record)
        .map_err(|_| db_err())?;
    match rows.next() {
        Some(Ok(rec)) => Ok(Some(rec)),
        Some(Err(_)) => Err(db_err()),
        None => Ok(None),
    }
}

pub(crate) fn list_prepared(conn: &rusqlite::Connection) -> SeaRes<Vec<OperationRecord>> {
    let mut stmt = conn
        .prepare(&format!(
            "{SELECT} WHERE status = 'prepared' ORDER BY created_at, rowid"
        ))
        .map_err(|_| db_err())?;
    let rows = stmt.query_map([], row_to_record).map_err(|_| db_err())?;
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(rec) => out.push(rec),
            Err(_) => return Err(db_err()),
        }
    }
    Ok(out)
}

pub(crate) fn write_prepared(conn: &rusqlite::Connection, rec: &OperationRecord) -> SeaRes<()> {
    let err_out = || db_err();
    let step = || -> SeaRes<()> {
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| err_out())?;
        // Plain INSERT keyed by operationId. A UNIQUE conflict must surface as
        // a typed failure: INSERT OR REPLACE here would silently erase a
        // different operation whose row shares this operation_id.
        let r = conn.execute(
            "INSERT INTO operations (operation_id, request_digest, note_id, status, kind, note_relative, old_revision, new_revision, old_hash, new_hash, stage_relative, old_relative, created_at) \
             VALUES (?1,?2,?3,'prepared',?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                rec.operation_id,
                rec.request_digest,
                rec.note_id,
                rec.kind,
                rec.note_relative,
                rec.old_revision.map(|v| v as i64),
                rec.new_revision.map(|v| v as i64),
                rec.old_hash,
                rec.new_hash,
                rec.stage_relative,
                rec.old_relative,
                rec.created_at,
            ],
        );
        match r {
            Ok(_) => {
                conn.execute_batch("COMMIT").map_err(|_e| err_out())?;
                Ok(())
            }
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(err_out())
            }
        }
    };
    step()
}

/// Finalize SQL pieces inside BEGIN IMMEDIATE .. COMMIT: operation status plus
/// the note row snapshot, applied atomically.
pub(crate) fn finalize_commit(
    conn: &rusqlite::Connection,
    operation_id: &str,
    note: &super::state::NoteRecord,
) -> SeaRes<()> {
    let err_out = || db_err();
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|_| err_out())?;
    let ok = (|| -> SeaRes<()> {
        conn.execute(
            "UPDATE operations SET status='committed', new_revision=?2, new_hash=?3 WHERE operation_id=?1",
            rusqlite::params![operation_id, note.local_revision as i64, note.content_hash],
        )
        .map_err(|_| err_out())?;
        conn.execute(
            "INSERT OR REPLACE INTO notes (relative_path, note_id, content_hash, local_revision, read_only, id_embedded) \
             VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![
                note.relative_path,
                note.note_id,
                note.content_hash,
                note.local_revision as i64,
                note.read_only as i64,
                note.id_embedded as i64,
            ],
        )
        .map_err(|_| err_out())?;
        Ok(())
    })();
    match ok {
        Ok(()) => conn.execute_batch("COMMIT").map_err(|_| err_out()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
