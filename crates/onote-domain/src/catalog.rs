use crate::{NoteSummary, VaultFinding};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogChangeKind {
    Added,
    Updated,
    Moved,
    Unavailable,
    ReadOnlyChanged,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogChange {
    pub note_id: String,
    pub kind: CatalogChangeKind,
    pub previous_path: Option<String>,
    pub relative_path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRefresh {
    pub notes: Vec<NoteSummary>,
    pub changes: Vec<CatalogChange>,
    pub unavailable_note_ids: Vec<String>,
    pub findings: Vec<VaultFinding>,
    pub fingerprint: String,
    pub deferred: bool,
    pub persisted: bool,
    pub writable: bool,
}
