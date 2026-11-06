use super::*;
use crate::reference_check::{local_path, markdown_targets};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotReferenceIssueReason {
    Missing,
    Unsafe,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotReferenceIssue {
    pub note_id: String,
    pub note_path: String,
    pub reference: String,
    pub reason: SnapshotReferenceIssueReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotReferenceReport {
    pub complete: bool,
    pub issues: Vec<SnapshotReferenceIssue>,
}

impl Default for SnapshotReferenceReport {
    fn default() -> Self {
        Self {
            complete: true,
            issues: Vec::new(),
        }
    }
}

impl BackupStore {
    pub(super) fn reference_report(
        &self,
        directory: &Path,
        manifest: &Manifest,
        cancel: &AtomicBool,
    ) -> Result<SnapshotReferenceReport, ServiceError> {
        let mut issues = Vec::new();
        for note in &manifest.notes {
            cancelled(cancel)?;
            let bytes = safe_io::read(
                directory,
                &directory.join("objects").join(&note.content_hash),
            )
            .map_err(|_| incomplete())?;
            let Ok(source) = std::str::from_utf8(&bytes) else {
                continue;
            };
            for target in markdown_targets(source) {
                match resolve_reference(&note.relative_path, &target, &manifest.files) {
                    ReferenceCheck::Present | ReferenceCheck::External => (),
                    ReferenceCheck::Missing => issues.push(SnapshotReferenceIssue {
                        note_id: note.note_id.clone(),
                        note_path: note.relative_path.clone(),
                        reference: target,
                        reason: SnapshotReferenceIssueReason::Missing,
                    }),
                    ReferenceCheck::Unsafe => issues.push(SnapshotReferenceIssue {
                        note_id: note.note_id.clone(),
                        note_path: note.relative_path.clone(),
                        reference: target,
                        reason: SnapshotReferenceIssueReason::Unsafe,
                    }),
                }
            }
        }
        issues.sort();
        issues.dedup();
        Ok(SnapshotReferenceReport {
            complete: issues.is_empty(),
            issues,
        })
    }
}

enum ReferenceCheck {
    Present,
    Missing,
    Unsafe,
    External,
}

fn resolve_reference(note_path: &str, raw: &str, files: &[SnapshotFile]) -> ReferenceCheck {
    let Some(target) = local_path(note_path, raw) else {
        return ReferenceCheck::External;
    };
    let target = match target {
        Ok(target) => target,
        Err(()) => return ReferenceCheck::Unsafe,
    };
    if files.iter().any(|file| {
        onote_vault_storage::compare_windows_paths(&file.relative_path, &target).is_eq()
    }) {
        ReferenceCheck::Present
    } else {
        ReferenceCheck::Missing
    }
}
