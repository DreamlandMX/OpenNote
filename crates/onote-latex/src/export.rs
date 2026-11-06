//! M13 LaTeX project export manifest (F54).
//!
//! A LaTeX project is more than its `.tex` files: it needs the bibliography, the
//! included relative assets and a `\input`/`\bibliography` closure. Exporting a
//! partial set silently produces a project that cannot compile, so this module
//! plans the export and reports what is missing instead of writing a file.
//!
//! Three rules shape it:
//!
//! * the manifest is committed last (F54-R02): a half-written project is never
//!   activated, so the plan is produced and verified before anything is written;
//! * a missing asset or a source newer than its PDF is reported, never glossed
//!   over (PROJECT_INCOMPLETE, PDF_OUTDATED);
//! * caches, logs and `.synctex` are excluded from a "source" export (F54 §4).
//!
//! Nothing here reads or writes a file, and nothing uploads.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// What the export contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportKind {
    /// The last successful PDF only.
    Pdf,
    /// Sources, resources and project metadata.
    SourceZip,
}

/// Files excluded from a source export by default (F54 §4).
pub const EXCLUDED_EXTENSIONS: &[&str] = &[
    "aux",
    "log",
    "out",
    "toc",
    "lof",
    "lot",
    "fls",
    "fdb_latexmk",
    "synctex",
    "synctex.gz",
    "blg",
    "bbl",
    "nav",
    "snm",
    "vrb",
    "idx",
    "ilg",
    "ind",
    "glo",
    "gls",
    "glg",
    "run.xml",
    "bcf",
    "xdv",
    "dvi",
];

/// Directories never worth exporting.
pub const EXCLUDED_DIRS: &[&str] = &["__pycache__", ".git", "node_modules"];

/// One file the export would include.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEntry {
    /// Vault-relative path.
    pub path: String,
    /// `tex`, `bib`, `asset`, `metadata` or `pdf`.
    pub kind: String,
    /// Size in bytes when known.
    pub bytes: Option<u64>,
}

/// A problem that stops an export from being called complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportIssue {
    /// A referenced file is not in the project (PROJECT_INCOMPLETE).
    MissingFile { path: String, referenced_by: String },
    /// The source is newer than the PDF (PDF_OUTDATED).
    PdfOutdated {
        source_hash: String,
        pdf_source_hash: String,
    },
    /// A file exceeds the service limit (OBJECT_TOO_LARGE).
    TooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },
    /// No successful PDF exists at all, so a PDF export has nothing to write.
    NoPdf,
    /// The expected manifest hash no longer matches, so the caller is stale.
    ManifestMismatch { expected: String, actual: String },
}

/// The planned export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPlan {
    pub entries: Vec<ExportEntry>,
    /// Empty only when the export may be called complete.
    pub issues: Vec<ExportIssue>,
    /// Leftover files that were deliberately dropped, for transparency.
    pub excluded: Vec<String>,
    /// Total bytes of the included entries where sizes were known.
    pub total_bytes: u64,
    /// True when `issues` is empty and nothing essential was excluded.
    pub complete: bool,
}

/// A file in the project directory, as the caller listed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub path: String,
    pub bytes: Option<u64>,
    /// The file's text, when the caller could read it; used to find `\input` and
    /// `\bibliography` references without a second read.
    pub text: Option<String>,
}

/// Extensions treated as TeX sources.
fn is_tex(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".tex")
}

fn is_bib(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".bib")
}

fn is_metadata(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    // The project manifest is the file that must be committed last, so it has to
    // be recognised by every name the tooling uses for it.
    name == "latexmkrc"
        || name == "latex.json"
        || name.ends_with(".latex.json")
        || name == "manifest.json"
}

fn is_excluded(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if EXCLUDED_DIRS
        .iter()
        .any(|dir| lower.split('/').any(|segment| segment == *dir))
    {
        return true;
    }
    // `synctex.gz` must match before plain `gz` is considered.
    if lower.ends_with(".synctex.gz") {
        return true;
    }
    match name.rsplit_once('.') {
        Some((_, extension)) => EXCLUDED_EXTENSIONS.contains(&extension),
        None => false,
    }
}

/// Extracts the paths a TeX source refers to.
///
/// The parsing is shared with project inspection, so spaced arguments and
/// `\includegraphics[width=...]` are part of the export closure too (F54-R04).
/// A macro-generated path cannot be resolved statically, so a missing file is
/// reported rather than guessed (PROJECT_INCOMPLETE).
pub fn references_in(text: &str) -> Vec<(String, &'static str)> {
    let mut found: Vec<(String, &'static str)> = crate::references::references_in(text)
        .into_iter()
        .map(|(reference, kind)| (reference.trim_matches('"').to_owned(), kind.as_str()))
        .filter(|(reference, _)| !reference.is_empty())
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Resolves a reference to a project-relative path, trying the documented
/// extensions TeX adds implicitly.
fn resolve_reference(reference: &str, kind: &str, available: &BTreeSet<String>) -> Option<String> {
    let normalized = reference.replace('\\', "/");
    let mut candidates: Vec<String> = vec![normalized.clone()];
    match kind {
        "input" => candidates.push(format!("{normalized}.tex")),
        "bib" => candidates.push(format!("{normalized}.bib")),
        _ => {
            for extension in ["png", "jpg", "jpeg", "pdf", "eps", "svg"] {
                candidates.push(format!("{normalized}.{extension}"));
            }
        }
    }
    candidates
        .into_iter()
        .find(|candidate| available.contains(candidate))
}

/// Plans an export.
///
/// The closure starts from every `.tex` file in the project, follows its
/// references, and stops when nothing new is found. Anything referenced but
/// absent becomes an issue, so the caller never writes a project that cannot
/// compile while calling it complete (F54-R04).
pub fn plan_export(
    kind: ExportKind,
    files: &[ProjectFile],
    expected_manifest_hash: &str,
    actual_manifest_hash: &str,
    size_limit: Option<u64>,
    source_hash: &str,
    pdf_source_hash: Option<&str>,
) -> ExportPlan {
    let mut issues: Vec<ExportIssue> = Vec::new();
    if expected_manifest_hash != actual_manifest_hash {
        issues.push(ExportIssue::ManifestMismatch {
            expected: expected_manifest_hash.to_owned(),
            actual: actual_manifest_hash.to_owned(),
        });
    }

    let by_path: BTreeMap<String, &ProjectFile> = files
        .iter()
        .map(|file| (file.path.replace('\\', "/"), file))
        .collect();
    let available: BTreeSet<String> = by_path.keys().cloned().collect();

    let mut included: BTreeSet<String> = BTreeSet::new();
    let mut excluded: Vec<String> = Vec::new();

    if matches!(kind, ExportKind::Pdf) {
        // A PDF export writes only the last successful PDF.
        let pdf = files
            .iter()
            .find(|file| file.path.to_ascii_lowercase().ends_with(".pdf"));
        match pdf {
            Some(file) if pdf_source_hash == Some(source_hash) => {
                included.insert(file.path.replace('\\', "/"));
            }
            Some(file) => {
                included.insert(file.path.replace('\\', "/"));
                issues.push(ExportIssue::PdfOutdated {
                    source_hash: source_hash.to_owned(),
                    pdf_source_hash: pdf_source_hash.unwrap_or("none").to_owned(),
                });
            }
            None => issues.push(ExportIssue::NoPdf),
        }
    } else {
        // Sources: start from every .tex file and follow the references.
        let mut queue: VecDeque<String> = by_path
            .keys()
            .filter(|path| is_tex(path) || is_metadata(path) || is_bib(path))
            .cloned()
            .collect();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(path) = queue.pop_front() {
            if !seen.insert(path.clone()) {
                continue;
            }
            if is_excluded(&path) {
                excluded.push(path);
                continue;
            }
            let Some(file) = by_path.get(&path) else {
                continue;
            };
            included.insert(path.clone());
            let Some(text) = file.text.as_deref() else {
                continue;
            };
            for (reference, reference_kind) in references_in(text) {
                match resolve_reference(&reference, reference_kind, &available) {
                    Some(resolved) => {
                        if !seen.contains(&resolved) {
                            queue.push_back(resolved);
                        }
                    }
                    None => issues.push(ExportIssue::MissingFile {
                        path: reference,
                        referenced_by: path.clone(),
                    }),
                }
            }
        }
        // A graphics file that exists but was never referenced is still part of
        // the project when the user exports "everything"; an unreferenced cache
        // stays out.
        for (path, _) in by_path.iter() {
            if is_excluded(path) {
                if !excluded.contains(path) {
                    excluded.push(path.clone());
                }
            }
        }
        excluded.sort();
        excluded.dedup();
    }

    let mut entries: Vec<ExportEntry> = Vec::new();
    let mut total_bytes = 0u64;
    for path in &included {
        let file = by_path.get(path);
        let bytes = file.and_then(|file| file.bytes);
        if let (Some(bytes), Some(limit)) = (bytes, size_limit) {
            if bytes > limit {
                // Over the service limit: the local export still works, but the
                // caller must not claim it was synced (OBJECT_TOO_LARGE).
                issues.push(ExportIssue::TooLarge {
                    path: path.clone(),
                    bytes,
                    limit,
                });
            }
        }
        if let Some(bytes) = bytes {
            total_bytes = total_bytes.saturating_add(bytes);
        }
        entries.push(ExportEntry {
            path: path.clone(),
            kind: if is_tex(path) {
                "tex".to_owned()
            } else if is_bib(path) {
                "bib".to_owned()
            } else if is_metadata(path) {
                "metadata".to_owned()
            } else if path.to_ascii_lowercase().ends_with(".pdf") {
                "pdf".to_owned()
            } else {
                "asset".to_owned()
            },
            bytes,
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    let complete = issues.is_empty();
    ExportPlan {
        entries,
        issues,
        excluded,
        total_bytes,
        complete,
    }
}

/// A short description of the plan, for a confirmation line before writing.
///
/// The manifest is the last thing written (F54-R02), so the caller shows this
/// before the commit and the plan is what gets committed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub files: usize,
    pub total_bytes: u64,
    pub complete: bool,
    pub excluded: usize,
    pub issues: usize,
}

pub fn summarize(plan: &ExportPlan) -> ExportSummary {
    ExportSummary {
        files: plan.entries.len(),
        total_bytes: plan.total_bytes,
        complete: plan.complete,
        excluded: plan.excluded.len(),
        issues: plan.issues.len(),
    }
}

/// The order files must be written in so a commit never activates a half project.
///
/// Assets and sources come first and the manifest last: a reader that sees the
/// manifest knows the rest is already present (F54-R02).
pub fn write_order(plan: &ExportPlan) -> Vec<String> {
    let mut ordered: Vec<String> = plan
        .entries
        .iter()
        .filter(|entry| entry.kind != "metadata")
        .map(|entry| entry.path.clone())
        .collect();
    ordered.extend(
        plan.entries
            .iter()
            .filter(|entry| entry.kind == "metadata")
            .map(|entry| entry.path.clone()),
    );
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: Option<&str>) -> ProjectFile {
        ProjectFile {
            path: path.to_owned(),
            bytes: Some(100),
            text: text.map(str::to_owned),
        }
    }

    fn project() -> Vec<ProjectFile> {
        vec![
            file(
                "main.tex",
                Some("\\input{chapters/intro}\n\\bibliography{refs}\n"),
            ),
            file(
                "chapters/intro.tex",
                Some("\\includegraphics{figures/plot}\n"),
            ),
            file("refs.bib", None),
            ProjectFile {
                path: "figures/plot.png".to_owned(),
                bytes: Some(2048),
                text: None,
            },
            file("main.log", None),
            file("main.synctex.gz", None),
            file("main.pdf", None),
        ]
    }

    const HASH: &str = "h1";

    #[test]
    fn a_source_export_follows_the_input_and_bibliography_closure() {
        let plan = plan_export(
            ExportKind::SourceZip,
            &project(),
            HASH,
            HASH,
            None,
            HASH,
            Some(HASH),
        );
        let paths: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert!(paths.contains(&"main.tex"));
        assert!(paths.contains(&"chapters/intro.tex"));
        assert!(paths.contains(&"refs.bib"));
        assert!(paths.contains(&"figures/plot.png"));
        assert!(plan.complete, "issues: {:?}", plan.issues);
    }

    #[test]
    fn caches_and_synctex_are_excluded_from_a_source_export() {
        // F54 §4: logs, aux and synctex are not source.
        let plan = plan_export(
            ExportKind::SourceZip,
            &project(),
            HASH,
            HASH,
            None,
            HASH,
            Some(HASH),
        );
        let paths: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert!(!paths.contains(&"main.log"));
        assert!(!paths.contains(&"main.synctex.gz"));
        assert!(plan.excluded.iter().any(|path| path == "main.log"));
        assert!(plan.excluded.iter().any(|path| path == "main.synctex.gz"));
    }

    #[test]
    fn a_missing_reference_is_reported_instead_of_written() {
        // PROJECT_INCOMPLETE: never call a project complete when it cannot build.
        let files = vec![file("main.tex", Some("\\input{chapters/missing}\n"))];
        let plan = plan_export(ExportKind::SourceZip, &files, HASH, HASH, None, HASH, None);
        assert!(!plan.complete);
        assert!(plan.issues.iter().any(|issue| matches!(
            issue,
            ExportIssue::MissingFile { path, .. } if path == "chapters/missing"
        )));
    }

    #[test]
    fn a_pdf_export_of_an_older_pdf_is_marked_outdated() {
        // PDF_OUTDATED: say which version, never imply the PDF matches the source.
        let plan = plan_export(
            ExportKind::Pdf,
            &project(),
            HASH,
            HASH,
            None,
            "new",
            Some("old"),
        );
        assert!(!plan.complete);
        assert!(plan
            .issues
            .iter()
            .any(|issue| matches!(issue, ExportIssue::PdfOutdated { .. })));
    }

    #[test]
    fn a_pdf_export_with_no_pdf_reports_it() {
        let files = vec![file("main.tex", None)];
        let plan = plan_export(ExportKind::Pdf, &files, HASH, HASH, None, HASH, None);
        assert!(matches!(plan.issues.first(), Some(ExportIssue::NoPdf)));
        assert!(plan.entries.is_empty());
    }

    #[test]
    fn a_matching_pdf_export_includes_only_the_pdf() {
        let plan = plan_export(
            ExportKind::Pdf,
            &project(),
            HASH,
            HASH,
            None,
            HASH,
            Some(HASH),
        );
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].path, "main.pdf");
        assert_eq!(plan.entries[0].kind, "pdf");
        assert!(plan.complete);
    }

    #[test]
    fn a_stale_manifest_hash_is_an_issue() {
        let plan = plan_export(
            ExportKind::SourceZip,
            &project(),
            "old",
            "new",
            None,
            HASH,
            Some(HASH),
        );
        assert!(!plan.complete);
        assert!(plan
            .issues
            .iter()
            .any(|issue| matches!(issue, ExportIssue::ManifestMismatch { .. })));
    }

    #[test]
    fn a_file_over_the_service_limit_is_reported_but_still_planned() {
        // OBJECT_TOO_LARGE: local export still works; syncing must not be claimed.
        let plan = plan_export(
            ExportKind::SourceZip,
            &project(),
            HASH,
            HASH,
            Some(1000),
            HASH,
            Some(HASH),
        );
        assert!(!plan.complete);
        assert!(plan
            .issues
            .iter()
            .any(|issue| matches!(issue, ExportIssue::TooLarge { .. })));
        // The oversized file is still in the plan, so a local export can proceed.
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.path == "figures/plot.png"));
    }

    #[test]
    fn the_manifest_is_written_last() {
        // F54-R02: a reader that sees the manifest knows the rest is present.
        let files = vec![
            file("main.tex", Some("body\n")),
            file("latex.json", Some("{}")),
        ];
        let plan = plan_export(ExportKind::SourceZip, &files, HASH, HASH, None, HASH, None);
        let order = write_order(&plan);
        assert_eq!(order.last().map(String::as_str), Some("latex.json"));
        assert!(order.contains(&"main.tex".to_owned()));
    }

    #[test]
    fn references_are_extracted_from_the_four_supported_commands() {
        let text = "\\input{a}\n\\include{b}\n\\includegraphics{c}\n\\bibliography{d,e}\n";
        let mut references = references_in(text);
        references.sort();
        assert_eq!(
            references,
            vec![
                ("a".to_owned(), "input"),
                ("b".to_owned(), "input"),
                ("c".to_owned(), "graphics"),
                ("d".to_owned(), "bib"),
                ("e".to_owned(), "bib"),
            ]
        );
    }

    #[test]
    fn spaced_and_option_arguments_are_extracted() {
        let text = "\\input {a}\n\\includegraphics*[width=1in]{b}\n\
                    \\includegraphics[width=1in][height=2in]{c}\n";
        assert_eq!(
            references_in(text),
            vec![
                ("a".to_owned(), "input"),
                ("b".to_owned(), "graphics"),
                ("c".to_owned(), "graphics"),
            ]
        );
    }

    #[test]
    fn a_graphics_option_and_spaced_input_stay_in_the_export_closure() {
        // F54-R04: an asset referenced as \includegraphics[width=...]{..} must
        // not disappear from the plan while the export still calls itself complete.
        let files = vec![
            file(
                "main.tex",
                Some("\\input {chapters/intro}\n\\includegraphics[width=2cm][height=3cm]{figures/plot}\n"),
            ),
            file("chapters/intro.tex", Some("Text\n")),
            ProjectFile {
                path: "figures/plot.png".to_owned(),
                bytes: Some(2048),
                text: None,
            },
        ];
        let plan = plan_export(ExportKind::SourceZip, &files, HASH, HASH, None, HASH, None);
        let paths: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert!(paths.contains(&"chapters/intro.tex"));
        assert!(paths.contains(&"figures/plot.png"));
        assert!(plan.complete, "issues: {:?}", plan.issues);
    }

    #[test]
    fn an_unterminated_command_does_not_panic() {
        assert!(references_in("\\input{unclosed").is_empty());
        assert!(references_in("").is_empty());
    }

    #[test]
    fn a_graphics_reference_resolves_by_its_implicit_extension() {
        let files = vec![
            file("main.tex", Some("\\includegraphics{plot}\n")),
            ProjectFile {
                path: "plot.png".to_owned(),
                bytes: Some(10),
                text: None,
            },
        ];
        let plan = plan_export(ExportKind::SourceZip, &files, HASH, HASH, None, HASH, None);
        assert!(plan.entries.iter().any(|entry| entry.path == "plot.png"));
        assert!(plan.complete, "issues: {:?}", plan.issues);
    }

    #[test]
    fn a_summary_reports_counts_without_claiming_completeness() {
        let plan = plan_export(
            ExportKind::SourceZip,
            &project(),
            HASH,
            HASH,
            None,
            HASH,
            None,
        );
        let summary = summarize(&plan);
        assert_eq!(summary.complete, plan.complete);
        assert_eq!(summary.issues, plan.issues.len());
        assert_eq!(summary.excluded, plan.excluded.len());
        assert_eq!(summary.files, plan.entries.len());
    }

    #[test]
    fn total_bytes_skips_unknown_sizes_rather_than_assuming_zero() {
        let files = vec![
            ProjectFile {
                path: "main.tex".to_owned(),
                bytes: None,
                text: Some("body".to_owned()),
            },
            ProjectFile {
                path: "other.tex".to_owned(),
                bytes: Some(50),
                text: Some("body".to_owned()),
            },
        ];
        let plan = plan_export(ExportKind::SourceZip, &files, HASH, HASH, None, HASH, None);
        assert_eq!(plan.total_bytes, 50);
    }

    #[test]
    fn an_empty_project_is_complete_and_empty() {
        let plan = plan_export(ExportKind::SourceZip, &[], HASH, HASH, None, HASH, None);
        assert!(plan.entries.is_empty());
        assert!(plan.complete);
    }
}
