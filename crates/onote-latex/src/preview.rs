//! M13 preview artifact bookkeeping and SyncTeX mapping (F52).
//!
//! A preview is only trustworthy when the PDF, the source snapshot and the
//! mapping all come from the same build. This module models that explicitly:
//!
//! * an artefact is identified by its id and the source hash it was built from,
//!   not by its path, so a rebuilt file cannot silently be treated as the old one
//!   (F52 §4);
//! * `matching` is the only state that permits a precise jump — `stale` and
//!   `none` must disable it and say why (F52-R03, MAPPING_STALE);
//! * a failed build never replaces the last readable PDF (F52-R01), and bytes are
//!   only published after a complete verification (F52-R04).
//!
//! Nothing here reads a file, renders a page or runs an engine.

use serde::{Deserialize, Serialize};

/// A built preview artefact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Immutable id, derived from the job and source hash (F52 §4).
    pub artifact_id: String,
    /// Hash of the source snapshot this PDF was built from.
    pub source_hash: String,
    /// Whether a SyncTeX file exists for this build at all.
    pub has_synctex: bool,
}

/// Whether a jump may be precise, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MappingState {
    /// Artefact and current source agree and a mapping exists.
    Matching,
    /// The source changed, or the mapping is missing.
    Stale,
    /// No artefact or no mapping at all.
    None,
}

/// The state of the readable PDF, independent of the build state (F52 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PdfState {
    Missing,
    Current,
    /// A readable PDF exists but comes from an older build.
    Outdated,
}

/// The outcome of a build, as far as the preview is concerned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildOutcome {
    /// Verified output: it may replace the previous PDF (F52-R04).
    Published { artifact: Artifact },
    /// The build failed; the previous artefact stays readable (F52-R01).
    Failed { reason: String },
    /// Output existed but did not pass verification, so it is not published.
    Invalid { reason: String },
}

/// What the preview should now show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewState {
    /// The artefact the viewer should open, if any.
    pub artifact_id: Option<String>,
    pub pdf_state: PdfState,
    pub mapping_state: MappingState,
    /// A message key explaining a non-matching state, if any.
    pub notice_key: Option<String>,
}

/// A source position a jump can target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePosition {
    /// Vault-relative path of the source file.
    pub file: String,
    /// One-based line in that file.
    pub line: u64,
    /// Zero-based column, when the mapping provided one.
    pub column: Option<u64>,
}

/// One SyncTeX record, as parsed from the `.synctex` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecord {
    /// Page number, one-based.
    pub page: u32,
    /// Vertical position on the page in the unit the file uses.
    pub y: i64,
    pub position: SourcePosition,
}

/// A jump request from the PDF back into the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JumpTarget {
    pub position: SourcePosition,
    pub mapping_state: MappingState,
    /// False when the caller must not highlight: a stale mapping points at an
    /// older snapshot (F52-A01).
    pub allow_highlight: bool,
}

/// The mapping held for one artefact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mapping {
    pub artifact_id: String,
    pub source_hash: String,
    pub records: Vec<SyncRecord>,
}

/// Applies a build result to the preview state.
///
/// `current_hash` is the source now on disk. A published artefact whose hash does
/// not match the current source is `Outdated` with a `Stale` mapping: the user can
/// keep reading, but a jump would land on the wrong line (F52-R03).
pub fn apply_build(
    previous: Option<&Artifact>,
    outcome: &BuildOutcome,
    current_hash: &str,
    has_mapping: bool,
) -> PreviewState {
    match outcome {
        BuildOutcome::Published { artifact } => {
            let mapping_state = mapping_state_for(artifact, current_hash, has_mapping);
            PreviewState {
                artifact_id: Some(artifact.artifact_id.clone()),
                // F52 §4: whether the readable PDF is current depends on the
                // source hash alone. A missing SyncTeX disables jumps; it does
                // not make an up-to-date PDF outdated.
                pdf_state: pdf_state_for(artifact, current_hash),
                mapping_state,
                notice_key: notice_for(mapping_state),
            }
        }
        // A failed or invalid build must not replace what the user can still
        // read (F52-R01). Its state is still described by its own source hash,
        // so a failed rebuild leaves a matching PDF current rather than
        // pretending the document is stale.
        BuildOutcome::Failed { .. } | BuildOutcome::Invalid { .. } => match previous {
            Some(artifact) => PreviewState {
                artifact_id: Some(artifact.artifact_id.clone()),
                pdf_state: pdf_state_for(artifact, current_hash),
                mapping_state: mapping_state_for(artifact, current_hash, has_mapping),
                notice_key: Some("preview.previousKept".to_owned()),
            },
            None => PreviewState {
                artifact_id: None,
                pdf_state: PdfState::Missing,
                mapping_state: MappingState::None,
                notice_key: Some("preview.missing".to_owned()),
            },
        },
    }
}

fn pdf_state_for(artifact: &Artifact, current_hash: &str) -> PdfState {
    if artifact.source_hash == current_hash {
        PdfState::Current
    } else {
        PdfState::Outdated
    }
}

fn mapping_state_for(artifact: &Artifact, current_hash: &str, has_mapping: bool) -> MappingState {
    if !artifact.has_synctex || !has_mapping {
        MappingState::None
    } else if artifact.source_hash == current_hash {
        MappingState::Matching
    } else {
        MappingState::Stale
    }
}

fn notice_for(state: MappingState) -> Option<String> {
    match state {
        MappingState::Matching => None,
        MappingState::Stale => Some("preview.mappingStale".to_owned()),
        MappingState::None => Some("preview.mappingNone".to_owned()),
    }
}

/// Resolves the source position at a PDF location.
///
/// A jump is offered even when the mapping is stale, but `allow_highlight` is
/// false so the caller opens the snapshot instead of highlighting the current
/// line (F52-R03).
pub fn locate(
    mapping: Option<&Mapping>,
    current_hash: &str,
    page: u32,
    y: i64,
) -> Result<JumpTarget, MappingState> {
    let Some(mapping) = mapping else {
        return Err(MappingState::None);
    };
    let state = if mapping.source_hash == current_hash {
        MappingState::Matching
    } else {
        MappingState::Stale
    };
    // The nearest record on the page at or above the requested vertical position
    // is the one a reader is looking at; exact equality is rare in a real file.
    let on_page: Vec<&SyncRecord> = mapping
        .records
        .iter()
        .filter(|record| record.page == page)
        .collect();
    let chosen = on_page
        .iter()
        .filter(|record| record.y <= y)
        .max_by_key(|record| record.y)
        .or_else(|| on_page.iter().min_by_key(|record| record.y));
    match chosen {
        Some(record) => Ok(JumpTarget {
            position: record.position.clone(),
            mapping_state: state,
            allow_highlight: state == MappingState::Matching,
        }),
        None => Err(state),
    }
}

/// Whether any record names a file outside the vault.
///
/// F52-R02: a mapping that contains absolute paths stays local and is not
/// synchronised by default; this reports that condition so the caller can act.
pub fn has_absolute_paths(mapping: &Mapping) -> bool {
    mapping.records.iter().any(|record| {
        let file = record.position.file.replace('\\', "/");
        file.starts_with('/') || file.chars().nth(1) == Some(':')
    })
}

/// The basename of a SyncTeX file path, with a drive prefix removed first.
///
/// `has_absolute_paths` treats `C:main.tex` as path-qualified even without a
/// separator, so localisation must strip the drive before taking the basename;
/// otherwise the copy would still be flagged as absolute (F52-R02).
fn localized_file_name(file: &str) -> String {
    let normalized = file.replace('\\', "/");
    let without_drive = if normalized.as_bytes().get(1) == Some(&b':') {
        &normalized[2..]
    } else {
        &normalized
    };
    without_drive
        .rsplit('/')
        .next()
        .unwrap_or(without_drive)
        .to_owned()
}

/// A copy of the mapping with absolute paths reduced to their file name.
///
/// The basename is kept so the mapping remains useful inside the vault, while the
/// machine path that must stay local is removed (F52-R02).
pub fn localize_paths(mapping: &Mapping) -> Mapping {
    Mapping {
        artifact_id: mapping.artifact_id.clone(),
        source_hash: mapping.source_hash.clone(),
        records: mapping
            .records
            .iter()
            .map(|record| SyncRecord {
                page: record.page,
                y: record.y,
                position: SourcePosition {
                    file: localized_file_name(&record.position.file),
                    line: record.position.line,
                    column: record.position.column,
                },
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(id: &str, hash: &str, synctex: bool) -> Artifact {
        Artifact {
            artifact_id: id.to_owned(),
            source_hash: hash.to_owned(),
            has_synctex: synctex,
        }
    }

    #[test]
    fn a_matching_build_allows_a_current_pdf() {
        let published = BuildOutcome::Published {
            artifact: artifact("a1", "h1", true),
        };
        let state = apply_build(None, &published, "h1", true);
        assert_eq!(state.pdf_state, PdfState::Current);
        assert_eq!(state.mapping_state, MappingState::Matching);
        assert_eq!(state.notice_key, None);
        assert_eq!(state.artifact_id.as_deref(), Some("a1"));
    }

    #[test]
    fn a_source_change_makes_the_mapping_stale_not_matching() {
        // F52-A01: an older PDF must not jump into a changed source.
        let published = BuildOutcome::Published {
            artifact: artifact("a1", "h1", true),
        };
        let state = apply_build(None, &published, "h2", true);
        assert_eq!(state.mapping_state, MappingState::Stale);
        assert_eq!(state.pdf_state, PdfState::Outdated);
        assert_eq!(state.notice_key.as_deref(), Some("preview.mappingStale"));
    }

    #[test]
    fn a_build_without_synctex_has_no_mapping() {
        let published = BuildOutcome::Published {
            artifact: artifact("a1", "h1", false),
        };
        let state = apply_build(None, &published, "h1", false);
        // F52 §4: the PDF state is independent of the mapping. Without SyncTeX
        // jumps are unavailable, but a PDF built from the current source is
        // still current.
        assert_eq!(state.pdf_state, PdfState::Current);
        assert_eq!(state.mapping_state, MappingState::None);
        assert_eq!(state.notice_key.as_deref(), Some("preview.mappingNone"));
    }

    #[test]
    fn a_failed_build_keeps_the_previous_pdf_readable() {
        // F52-R01: a failure never removes what the user can still read.
        let previous = artifact("a1", "h1", true);
        let state = apply_build(
            Some(&previous),
            &BuildOutcome::Failed {
                reason: "engine error".to_owned(),
            },
            "h2",
            true,
        );
        assert_eq!(state.artifact_id.as_deref(), Some("a1"));
        assert_eq!(state.pdf_state, PdfState::Outdated);
        assert_eq!(state.mapping_state, MappingState::Stale);
        assert_eq!(state.notice_key.as_deref(), Some("preview.previousKept"));
    }

    #[test]
    fn a_failed_rebuild_does_not_make_a_matching_pdf_stale() {
        // F52-R01 and §4: the rebuild failed, but the previous PDF still came
        // from the current source, so only the attempt is newer — not the PDF.
        let previous = artifact("a1", "h1", true);
        let state = apply_build(
            Some(&previous),
            &BuildOutcome::Failed {
                reason: "engine error".to_owned(),
            },
            "h1",
            true,
        );
        assert_eq!(state.artifact_id.as_deref(), Some("a1"));
        assert_eq!(state.pdf_state, PdfState::Current);
        assert_eq!(state.mapping_state, MappingState::Matching);
        assert_eq!(state.notice_key.as_deref(), Some("preview.previousKept"));
    }

    #[test]
    fn a_missing_mapping_does_not_hide_an_outdated_pdf() {
        let published = BuildOutcome::Published {
            artifact: artifact("a1", "h1", true),
        };
        let state = apply_build(None, &published, "h2", false);
        assert_eq!(state.pdf_state, PdfState::Outdated);
        assert_eq!(state.mapping_state, MappingState::None);
    }

    #[test]
    fn an_invalid_output_is_not_published() {
        // F52-R04: bytes are published only after verification.
        let previous = artifact("a1", "h1", false);
        let state = apply_build(
            Some(&previous),
            &BuildOutcome::Invalid {
                reason: "empty pdf".to_owned(),
            },
            "h1",
            false,
        );
        assert_eq!(state.artifact_id.as_deref(), Some("a1"));
    }

    #[test]
    fn a_first_build_failure_shows_the_empty_state() {
        // F52-A03: no unrelated document is shown as if it were this one.
        let state = apply_build(
            None,
            &BuildOutcome::Failed {
                reason: "engine error".to_owned(),
            },
            "h1",
            false,
        );
        assert_eq!(state.artifact_id, None);
        assert_eq!(state.pdf_state, PdfState::Missing);
        assert_eq!(state.mapping_state, MappingState::None);
    }

    #[test]
    fn locating_on_a_matching_mapping_allows_highlight() {
        let mapping = Mapping {
            artifact_id: "a1".to_owned(),
            source_hash: "h1".to_owned(),
            records: vec![
                SyncRecord {
                    page: 1,
                    y: 100,
                    position: SourcePosition {
                        file: "main.tex".to_owned(),
                        line: 10,
                        column: Some(1),
                    },
                },
                SyncRecord {
                    page: 1,
                    y: 300,
                    position: SourcePosition {
                        file: "main.tex".to_owned(),
                        line: 20,
                        column: Some(1),
                    },
                },
            ],
        };
        let target = locate(Some(&mapping), "h1", 1, 250).expect("target");
        // The nearest record at or above the click wins.
        assert_eq!(target.position.line, 10);
        assert!(target.allow_highlight);
        assert_eq!(target.mapping_state, MappingState::Matching);
    }

    #[test]
    fn locating_on_a_stale_mapping_forbids_highlight() {
        // F52-R03: open the snapshot, do not highlight the current line.
        let mapping = Mapping {
            artifact_id: "a1".to_owned(),
            source_hash: "h1".to_owned(),
            records: vec![SyncRecord {
                page: 1,
                y: 10,
                position: SourcePosition {
                    file: "main.tex".to_owned(),
                    line: 3,
                    column: None,
                },
            }],
        };
        let target = locate(Some(&mapping), "h2", 1, 10).expect("target");
        assert!(!target.allow_highlight);
        assert_eq!(target.mapping_state, MappingState::Stale);
    }

    #[test]
    fn locating_without_a_mapping_reports_none() {
        assert_eq!(locate(None, "h1", 1, 10), Err(MappingState::None));
    }

    #[test]
    fn locating_on_a_page_without_records_reports_the_state() {
        let mapping = Mapping {
            artifact_id: "a1".to_owned(),
            source_hash: "h1".to_owned(),
            records: vec![],
        };
        assert_eq!(
            locate(Some(&mapping), "h1", 2, 10),
            Err(MappingState::Matching)
        );
    }

    #[test]
    fn absolute_paths_are_detected_and_can_be_localized() {
        // F52-R02: such a mapping stays local.
        let mapping = Mapping {
            artifact_id: "a1".to_owned(),
            source_hash: "h1".to_owned(),
            records: vec![SyncRecord {
                page: 1,
                y: 1,
                position: SourcePosition {
                    file: "C:\\Users\\u\\doc\\main.tex".to_owned(),
                    line: 1,
                    column: None,
                },
            }],
        };
        assert!(has_absolute_paths(&mapping));
        let localized = localize_paths(&mapping);
        assert!(!has_absolute_paths(&localized));
        assert_eq!(localized.records[0].position.file, "main.tex");
    }

    #[test]
    fn a_relative_mapping_is_not_flagged() {
        let mapping = Mapping {
            artifact_id: "a1".to_owned(),
            source_hash: "h1".to_owned(),
            records: vec![SyncRecord {
                page: 1,
                y: 1,
                position: SourcePosition {
                    file: "chapters/intro.tex".to_owned(),
                    line: 1,
                    column: None,
                },
            }],
        };
        assert!(!has_absolute_paths(&mapping));
    }

    #[test]
    fn a_drive_relative_path_is_localized_not_just_flagged() {
        // `has_absolute_paths` flags `C:main.tex`, so localisation must remove
        // the drive prefix or the result would still be considered absolute.
        let mapping = Mapping {
            artifact_id: "a1".to_owned(),
            source_hash: "h1".to_owned(),
            records: vec![SyncRecord {
                page: 1,
                y: 1,
                position: SourcePosition {
                    file: "C:main.tex".to_owned(),
                    line: 1,
                    column: None,
                },
            }],
        };
        assert!(has_absolute_paths(&mapping));
        let localized = localize_paths(&mapping);
        assert_eq!(localized.records[0].position.file, "main.tex");
        assert!(!has_absolute_paths(&localized));
    }
}
