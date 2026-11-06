mod catalog;
pub use catalog::*;
mod rename;
pub use rename::*;
mod tag_update;
pub use tag_update::*;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vault {
    pub vault_id: String,
    pub schema_version: u32,
    pub display_name: String,
    pub sync_enabled: bool,
    pub writable: bool,
    pub findings: Vec<VaultFinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFinding {
    pub code: String,
    pub target: Option<String>,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    pub note_id: String,
    pub vault_id: String,
    pub relative_path: String,
    pub content_hash: String,
    pub local_revision: u64,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDocument {
    pub note_id: String,
    pub vault_id: String,
    pub relative_path: String,
    pub source: String,
    pub local_revision: u64,
    pub content_hash: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub note_id: String,
    pub local_revision: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRevision {
    pub revision_id: String,
    pub note_id: String,
    pub relative_path: String,
    pub local_revision: u64,
    pub content_hash: String,
    pub saved_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryListing {
    pub items: Vec<HistoryRevision>,
    pub unavailable_revision_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    pub object_id: String,
    pub note_id: String,
    pub original_path: String,
    pub content_hash: String,
    pub deleted_revision: u64,
    pub deleted_at: String,
    pub retention_days: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashRecord {
    pub entry: TrashEntry,
    pub document: NoteDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashRestoreResult {
    pub note_id: String,
    pub relative_path: String,
    pub local_revision: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrashCancellation {
    Cancelled,
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashOperation {
    pub operation_id: String,
    pub kind: String,
    pub phase: String,
    pub entry: TrashEntry,
    pub target_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashOperationResult {
    pub kind: String,
    pub entry: TrashEntry,
    pub restored: Option<TrashRestoreResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub snapshot_id: String,
    pub vault_id: String,
    pub schema_version: u32,
    pub manifest_hash: String,
    pub created_at: String,
    pub object_count: u64,
    pub total_bytes: u64,
    pub completion_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotProgress {
    pub phase: String,
    pub completed_objects: u64,
    pub total_objects: u64,
}

/// Durable draft checkpoint request: the caller's original editing state is
/// stored verbatim (raw bytes, never normalized/composed/filtered). The
/// operationId is the idempotency identity and the returned draftId.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRequest {
    pub note_id: String,
    pub relative_path: String,
    pub source: String,
    pub base_revision: u64,
    pub base_content_hash: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointResult {
    pub draft_id: String,
    pub note_id: String,
    pub source_hash: String,
    pub saved_at: String,
    pub sequence: u64,
}

/// Full stored draft, exposed only after integrity verification
/// (source/hash/request digest/paths).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDraft {
    pub draft_id: String,
    pub note_id: String,
    pub source_hash: String,
    pub saved_at: String,
    pub sequence: u64,
    pub relative_path: String,
    pub source: String,
    pub base_revision: u64,
    pub base_content_hash: String,
}

/// Listing metadata without the draft `source` body; bodies are loaded
/// per-item via read_draft only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSummary {
    pub draft_id: String,
    pub note_id: String,
    pub relative_path: String,
    pub source_hash: String,
    pub base_revision: u64,
    pub base_content_hash: String,
    pub saved_at: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftPage {
    pub items: Vec<DraftSummary>,
    pub next_before_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidInput,
    UnsupportedSchema,
    ObjectNotFound,
    VersionConflict,
    IdempotencyMismatch,
    IoDenied,
    DiskFull,
    FileLocked,
    RescanRequired,
    NotImplemented,
    HistoryMissing,
    PathCollision,
    BackupIncomplete,
    RestoreInterrupted,
    OperationCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceError {
    pub code: ErrorCode,
    pub message_key: String,
    pub params: BTreeMap<String, String>,
    pub retryable: bool,
    pub operation_id: Option<String>,
    pub retry_after_seconds: Option<u64>,
}
