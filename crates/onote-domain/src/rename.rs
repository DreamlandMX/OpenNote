use serde::{Deserialize, Serialize};

/// One byte range of one note that the rename must rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameEdit {
    pub note_id: String,
    pub relative_path: String,
    /// Byte range of the whole link (`![[…]]` included).
    pub offset: usize,
    pub length: usize,
    /// The link text exactly as it appears today.
    pub before: String,
    /// The rewritten link text; the label and anchor are preserved.
    pub after: String,
}

/// What a rename would touch, with nothing applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePlan {
    pub plan_id: String,
    pub vault_id: String,
    pub expected_hashes: Vec<RenameBaseline>,
    pub note_id: String,
    pub from_path: String,
    pub to_path: String,
    /// True when the note id and content stay the same (F03-R02).
    pub keeps_identity: bool,
    pub edits: Vec<RenameEdit>,
    /// Links that already fail and point at something resembling the old name.
    /// They are reported, never rewritten.
    pub untouched: Vec<RenameWarning>,
}

/// A reference the plan deliberately leaves alone, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameWarning {
    pub note_id: String,
    pub relative_path: String,
    pub offset: usize,
    pub target: String,
    pub reason: RenameWarningReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenameWarningReason {
    /// The link does not resolve to a single note, so retargeting it would be
    /// a guess (F10-A02).
    AmbiguousTarget,
    /// The target no longer exists; renaming must not silently repair it.
    BrokenLink,
}

/// Exact saved-byte baseline, including BOM, EOL and frontmatter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameBaseline {
    pub note_id: String,
    pub relative_path: String,
    pub expected_revision: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameResult {
    pub commit_id: String,
    pub completed: bool,
    pub items: Vec<RenameItemResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameItemResult {
    pub note_id: String,
    pub relative_path: String,
    pub local_revision: u64,
    pub content_hash: String,
}
