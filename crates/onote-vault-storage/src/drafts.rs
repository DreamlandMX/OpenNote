use sha2::{Digest, Sha256};

use onote_domain::{CheckpointResult, DraftPage, DraftSummary, ErrorCode, NoteDraft, ServiceError};

use rusqlite::{Transaction, TransactionBehavior};

use crate::errors::{self, SeaRes};
use crate::{commit, Failpoint};

/// IPC-safe numeric bound (2^53-1): values above it cannot round-trip through
/// JSON/JS frontends, so baseline revisions, stored sequence numbers and
/// pagination cursors are refused beyond it.
pub(crate) const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// One immutable draft checkpoint row. Rows are append-only: nothing of
/// this table is ever updated or deleted by this crate. The `source` body is
/// NOT part of this metadata struct: metadata queries never load bodies, and
/// `read_draft` fetches the source separately (see `fetch_source`).
pub(crate) struct DraftRow {
    pub draft_id: String,
    pub note_id: String,
    pub relative_path: String,
    pub source_hash: String,
    pub base_revision: u64,
    pub base_content_hash: String,
    pub request_digest: String,
    pub saved_at: String,
    pub sequence: Option<i64>,
}

/// Digest of the ORIGINAL caller request: operationId, noteId, canonical
/// path, SHA256 of the raw source bytes, baseRevision and baseContentHash —
/// every original request field is bound, so same id + same payload always
/// reproduces it, and any stored-field tampering (including a valid hash
/// substitution) is detected without touching the body. The caller supplies
/// the already-computed source hash; on the write path that is
/// `sha256(raw_source bytes)` so the digest binds the exact stored bytes.
pub(crate) fn request_digest(
    operation_id: &str,
    note_id: &str,
    relative_path: &str,
    source_hash: &str,
    base_revision: u64,
    base_content_hash: &str,
) -> String {
    let input = format!(
        "v3c\x1f{operation_id}\x1f{note_id}\x1f{relative_path}\x1f{source_hash}\x1f{base_revision}\x1f{base_content_hash}"
    );
    hex::encode(Sha256::digest(input.as_bytes()))
}

pub(crate) fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// UTC RFC 3339 via chrono (already locked); used for display only —
/// ordering is always the DB's monotonic `sequence`, never a timestamp.
pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Compatibly additive creation of the immutable append-only draft table:
/// `CREATE TABLE IF NOT EXISTS` never touches existing notes/operations
/// tables, rows, indexes or any prior data (no drop/reset/rewrite).
pub(crate) fn ensure_draft_checkpoints_table(conn: &rusqlite::Connection) -> SeaRes<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS draft_checkpoints (
             sequence INTEGER PRIMARY KEY AUTOINCREMENT,
             draft_id TEXT NOT NULL UNIQUE,
             request_digest TEXT NOT NULL,
             note_id TEXT NOT NULL,
             relative_path TEXT NOT NULL,
             source TEXT NOT NULL,
             source_hash TEXT NOT NULL,
             base_revision INTEGER NOT NULL,
             base_content_hash TEXT NOT NULL,
             saved_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_drafts_note_seq ON draft_checkpoints(note_id, sequence);",
    )
    .map_err(|_| errors::err(ErrorCode::IoDenied, "error.stateInitFailed", &[]))
}

const SELECT_META: &str = "SELECT draft_id, note_id, relative_path, source_hash, base_revision, base_content_hash, request_digest, saved_at, sequence FROM draft_checkpoints";

const INSERT: &str = "INSERT INTO draft_checkpoints \
    (draft_id, note_id, relative_path, source, source_hash, base_revision, base_content_hash, request_digest, saved_at) \
    VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)";

fn row_to_meta(row: &rusqlite::Row) -> rusqlite::Result<DraftRow> {
    let base_revision: i64 = row.get(4)?;
    if base_revision < 0 {
        // Corrupted row shape; surfaced by the validation layer as a typed
        // refusal, not converted into a fake value.
        return Err(corrupt_decode("base_revision"));
    }
    let sequence: i64 = row.get(8)?;
    if sequence <= 0 {
        return Err(corrupt_decode("sequence"));
    }
    Ok(DraftRow {
        draft_id: row.get(0)?,
        note_id: row.get(1)?,
        relative_path: row.get(2)?,
        source_hash: row.get(3)?,
        base_revision: base_revision as u64,
        base_content_hash: row.get(5)?,
        request_digest: row.get(6)?,
        saved_at: row.get(7)?,
        sequence: Some(sequence),
    })
}

fn corrupt_decode(name: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("corrupted draft field {name}").into(),
    )
}

fn decode_err() -> ServiceError {
    errors::err(ErrorCode::IoDenied, "error.draftCorrupted", &[])
}

fn db_err() -> ServiceError {
    errors::err(ErrorCode::IoDenied, "error.stateWriteFailed", &[])
}

/// Map SQLite failure codes onto existing typed errors: FULL/DISKFULL →
/// `DiskFull`, BUSY/LOCKED → retryable `FileLocked`; everything else stays
/// the established typed write failure. No broader framework.
fn map_sqlite_err(e: rusqlite::Error) -> ServiceError {
    if let rusqlite::Error::SqliteFailure(ffi_err, _) = &e {
        return match ffi_err.code {
            rusqlite::ffi::ErrorCode::DiskFull => {
                errors::err(ErrorCode::DiskFull, "error.stateWriteFailed", &[])
            }
            rusqlite::ffi::ErrorCode::DatabaseBusy | rusqlite::ffi::ErrorCode::DatabaseLocked => {
                errors::file_locked()
            }
            _ => db_err(),
        };
    }
    db_err()
}

pub(crate) fn find_by_draft_id(
    conn: &rusqlite::Connection,
    draft_id: &str,
) -> SeaRes<Option<DraftRow>> {
    let mut stmt = conn
        .prepare(&format!("{SELECT_META} WHERE draft_id = ?1"))
        .map_err(|_| db_err())?;
    let mut rows = stmt
        .query_map([draft_id], row_to_meta)
        .map_err(|_| db_err())?;
    match rows.next() {
        Some(Ok(rec)) => Ok(Some(rec)),
        Some(Err(_)) => Err(decode_err()),
        None => Ok(None),
    }
}

/// The full body of a metadata row, loaded only by `read_draft`. A vanished
/// row between metadata and body reads is an integrity violation: typed
/// refusal, never a fabricated empty body.
pub(crate) fn fetch_source(conn: &rusqlite::Connection, draft_id: &str) -> SeaRes<String> {
    let mut stmt = conn
        .prepare("SELECT source FROM draft_checkpoints WHERE draft_id = ?1")
        .map_err(|_| db_err())?;
    match stmt.query_row([draft_id], |row| row.get::<_, String>(0)) {
        Ok(source) => Ok(source),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(decode_err()),
        Err(_) => Err(db_err()),
    }
}

/// Append one immutable row inside an immediate RAII transaction. Every
/// error path — including a failing COMMIT — rolls the transaction back
/// (rusqlite `Transaction` Drop defaults to Rollback), leaving no partial
/// row and a reusable connection. The existing deterministic `WriteFailure`
/// failpoint is consumed mid-transaction to prove the rollback surface: the
/// row is undone and a typed `DiskFull` is returned. On success the
/// DB-assigned AUTOINCREMENT sequence is returned to the caller; a sequence
/// beyond `MAX_SAFE_INTEGER` (2^53-1, e.g. an externally corrupted
/// `sqlite_sequence`) is refused typed BEFORE COMMIT (RAII rollback, the
/// over-bound row is never committed or acknowledged, nothing is clamped or
/// renumbered) and the connection stays usable.
pub(crate) fn insert(
    conn: &rusqlite::Connection,
    row: &DraftRow,
    source: &str,
    failpoints: &mut impl FnMut(Failpoint) -> bool,
) -> SeaRes<i64> {
    let tx = match Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => return Err(map_sqlite_err(e)),
    };
    let params = rusqlite::params![
        row.draft_id,
        row.note_id,
        row.relative_path,
        source,
        row.source_hash,
        row.base_revision as i64,
        row.base_content_hash,
        row.request_digest,
        row.saved_at,
    ];
    if let Err(e) = tx.execute(INSERT, params) {
        // Transaction drops here: automatic ROLLBACK, connection reusable.
        return Err(map_sqlite_err(e));
    }
    let sequence = tx.last_insert_rowid();
    // BEFORE COMMIT: a DB-assigned AUTOINCREMENT sequence beyond the IPC-safe
    // 2^53-1 bound (externally corrupted sqlite_sequence) must never be
    // acknowledged — the transaction drops here (RAII ROLLBACK, the over-bound
    // row and its sequence bump are undone, never clamped/renumbered) and the
    // refusal is typed, leaving the connection reusable.
    if sequence <= 0 || sequence as u64 > MAX_SAFE_INTEGER {
        return Err(draft_corrupt());
    }
    if failpoints(Failpoint::WriteFailure) {
        // Drop performs the ROLLBACK; typed injected failure.
        return Err(errors::err(
            ErrorCode::DiskFull,
            "error.simulatedWriteFailure",
            &[],
        ));
    }
    match tx.commit() {
        Ok(()) => Ok(sequence),
        Err(e) => Err(map_sqlite_err(e)),
    }
}

/// Integrity contract of the STORED METADATA of a row before exposure (list,
/// replay, read pre-check): canonical UUID identities, hex-64 hashes,
/// safe canonical Markdown path shape exact form, IPC-safe revision and
/// sequence numbers, parsable UTC RFC3339 saved_at, and a request digest
/// reproducible from the stored metadata alone (binding source_hash,
/// baseline hash/revision, path, noteId and draftId). Violations are a typed
/// refusal — never a fabricated/empty result. The body is NOT inspected
/// here: full source verification happens only in `read_draft`.
pub(crate) fn validate_meta(row: &DraftRow) -> SeaRes<()> {
    let corrupt = draft_corrupt();
    if !commit::is_canonical_uuid(&row.draft_id) || !commit::is_canonical_uuid(&row.note_id) {
        return Err(corrupt);
    }
    if !is_sha256_hex(&row.source_hash) || !is_sha256_hex(&row.base_content_hash) {
        return Err(corrupt);
    }
    if crate::pathcheck::validate_new_note_path(&row.relative_path)
        .map(|canonical| canonical != row.relative_path)
        != Ok(false)
    {
        return Err(corrupt);
    }
    if row.base_revision > MAX_SAFE_INTEGER {
        return Err(corrupt);
    }
    let sequence = row.sequence.unwrap_or(0);
    if sequence <= 0 || sequence as u64 > MAX_SAFE_INTEGER {
        return Err(corrupt);
    }
    if chrono::DateTime::parse_from_rfc3339(&row.saved_at).is_err() {
        return Err(corrupt);
    }
    if row.request_digest
        != request_digest(
            &row.draft_id,
            &row.note_id,
            &row.relative_path,
            &row.source_hash,
            row.base_revision,
            &row.base_content_hash,
        )
    {
        return Err(corrupt);
    }
    Ok(())
}

fn draft_corrupt() -> ServiceError {
    errors::err(ErrorCode::IoDenied, "error.draftCorrupted", &[])
}

/// Full exposure (read path only): metadata integrity first, then the body
/// is fetched and its EXACT bytes verified against `source_hash`.
pub(crate) fn validate_with_source(row: &DraftRow, source: &str) -> SeaRes<()> {
    validate_meta(row)?;
    if hex::encode(Sha256::digest(source.as_bytes())) != row.source_hash {
        return Err(draft_corrupt());
    }
    Ok(())
}

pub(crate) fn result(row: &DraftRow) -> CheckpointResult {
    CheckpointResult {
        draft_id: row.draft_id.clone(),
        note_id: row.note_id.clone(),
        source_hash: row.source_hash.clone(),
        saved_at: row.saved_at.clone(),
        sequence: row.sequence.unwrap_or(0) as u64,
    }
}

pub(crate) fn summary(row: &DraftRow) -> DraftSummary {
    DraftSummary {
        draft_id: row.draft_id.clone(),
        note_id: row.note_id.clone(),
        relative_path: row.relative_path.clone(),
        source_hash: row.source_hash.clone(),
        base_revision: row.base_revision,
        base_content_hash: row.base_content_hash.clone(),
        saved_at: row.saved_at.clone(),
        sequence: row.sequence.unwrap_or(0) as u64,
    }
}

pub(crate) fn draft(row: DraftRow, source: String) -> NoteDraft {
    NoteDraft {
        draft_id: row.draft_id,
        note_id: row.note_id,
        source_hash: row.source_hash,
        saved_at: row.saved_at,
        sequence: row.sequence.unwrap_or(0) as u64,
        relative_path: row.relative_path,
        source,
        base_revision: row.base_revision,
        base_content_hash: row.base_content_hash,
    }
}

/// Validate normalized limit arguments: None => default 50; 0 => typed
/// rejection; otherwise hard cap 200.
pub(crate) fn normalize_limit(limit: Option<usize>) -> SeaRes<usize> {
    let n = match limit {
        None => LIST_DEFAULT_LIMIT,
        Some(0) => {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.numericRange",
                &[],
            ))
        }
        Some(n) => n.min(LIST_MAX_LIMIT),
    };
    Ok(n)
}

const LIST_DEFAULT_LIMIT: usize = 50;
const LIST_MAX_LIMIT: usize = 200;

/// Sequence-DESC page with `before` cursor (exclusive). Details:
/// - the query selects metadata columns ONLY (no source bodies are read or
///   hashed; the metadata digest binds the stored source_hash instead);
/// - every exposed row is validated via `validate_meta`; the first corrupted
///   row refuses the whole page typed instead of returning partial or
///   fabricated results;
/// - pagination and the default 50 / cap 200 contract are unchanged.
pub(crate) fn list_page(
    conn: &rusqlite::Connection,
    note_id: Option<&str>,
    before_sequence: Option<u64>,
    limit: usize,
) -> SeaRes<DraftPage> {
    let limit_i = i64::try_from(limit)
        .map_err(|_| errors::err(ErrorCode::InvalidInput, "error.numericRange", &[]))?;
    let before_i = match before_sequence {
        None => None,
        Some(before) => match i64::try_from(before) {
            Ok(v) => Some(v),
            Err(_) => {
                return Err(errors::err(
                    ErrorCode::InvalidInput,
                    "error.numericRange",
                    &[],
                ))
            }
        },
    };
    let mut stmt = match (note_id.is_some(), before_i.is_some()) {
        (true, true) => conn
            .prepare(&format!(
                "{SELECT_META} WHERE note_id = ?1 AND sequence < ?2 ORDER BY sequence DESC LIMIT ?3"
            ))
            .map_err(|_| db_err())?,
        (true, false) => conn
            .prepare(&format!(
                "{SELECT_META} WHERE note_id = ?1 ORDER BY sequence DESC LIMIT ?2"
            ))
            .map_err(|_| db_err())?,
        (false, true) => conn
            .prepare(&format!(
                "{SELECT_META} WHERE sequence < ?1 ORDER BY sequence DESC LIMIT ?2"
            ))
            .map_err(|_| db_err())?,
        (false, false) => conn
            .prepare(&format!("{SELECT_META} ORDER BY sequence DESC LIMIT ?1"))
            .map_err(|_| db_err())?,
    };
    use rusqlite::types::Value as V;
    let params: Vec<V> = match (note_id, before_i) {
        (Some(nid), Some(b)) => vec![V::from(nid.to_owned()), V::from(b), V::from(limit_i + 1)],
        (Some(nid), None) => vec![V::from(nid.to_owned()), V::from(limit_i + 1)],
        (None, Some(b)) => vec![V::from(b), V::from(limit_i + 1)],
        (None, None) => vec![V::from(limit_i + 1)],
    };
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), row_to_meta)
        .map_err(|_| db_err())?;
    let mut fetched: Vec<DraftRow> = Vec::new();
    for row in rows {
        let rec = match row {
            Ok(rec) => rec,
            // decode failures from corrupted rows are typed refusals
            Err(_) => return Err(decode_err()),
        };
        fetched.push(rec);
    }
    let has_more = fetched.len() > limit;
    if has_more {
        fetched.truncate(limit);
    }
    let mut items = Vec::with_capacity(fetched.len());
    for row in fetched {
        validate_meta(&row)?;
        items.push(summary(&row));
    }
    let next_before_sequence = if has_more {
        items.last().map(|s| s.sequence)
    } else {
        None
    };
    Ok(DraftPage {
        items,
        next_before_sequence,
    })
}
