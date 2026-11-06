use crate::{SaveResult, ServiceError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TagWritePlan {
    pub plan_id: String,
    pub vault_id: String,
    pub old_tag: String,
    pub new_tag: Option<String>,
    pub is_merge: bool,
    pub items: Vec<TagWriteItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TagWriteItem {
    pub note_id: String,
    pub relative_path: String,
    pub expected_revision: u64,
    pub content_hash: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagBatchResult {
    pub operation_id: String,
    pub old_tag: String,
    pub new_tag: Option<String>,
    pub is_merge: bool,
    pub completed: bool,
    pub cancelled: bool,
    pub items: Vec<TagBatchItemResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagBatchItemResult {
    pub note_id: String,
    pub relative_path: String,
    pub result: Option<SaveResult>,
    pub error: Option<ServiceError>,
}
