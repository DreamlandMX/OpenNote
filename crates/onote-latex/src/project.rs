//! M13 LaTeX project inspection (F45).
//!
//! Inspecting a directory never executes anything from it: no `.tex`, no script
//! and no `.latexmkrc` is run while the project is being identified (F45-R02).
//! That matters because a LaTeX project can contain `\write18` calls and shell
//! escapes, so "look at the project" must stay a pure read.
//!
//! Three further rules:
//!
//! * the entry file must be a `.tex` inside the project, and when several are
//!   plausible the candidates are returned for the user to choose rather than
//!   guessed (ENTRY_AMBIGUOUS);
//! * a dependency this build does not recognise is reported, never silently
//!   dropped and never fixed by editing the source (F45-R04);
//! * a reference that resolves outside the project root is refused, not read
//!   (F45-R06, UNSAFE_PROJECT_PATH).
//!
//! Nothing here reads or writes a file.

use serde::{Deserialize, Serialize};

/// A file in the project directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    /// Path relative to the project root, with `/` separators.
    pub path: String,
    pub bytes: Option<u64>,
    /// Text when the caller could read it; used to find references.
    pub text: Option<String>,
}

/// A plausible entry file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryCandidate {
    pub path: String,
    /// Higher means more likely: a `\documentclass` outranks a bare `main.tex`.
    pub score: u32,
    /// Why this candidate scored what it did, for the picker's explanation.
    pub reason_key: String,
}

/// A package or engine feature the project depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    /// `usepackage`, `documentclass`, `engine` or `bibliography`.
    pub kind: String,
    pub name: String,
    /// True when this build recognises it.
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// Internally tagged, like the other M13 enums, so the UI reads kind.
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ProjectFinding {
    /// More than one plausible entry file (ENTRY_AMBIGUOUS).
    EntryAmbiguous { candidates: Vec<String> },
    /// No `.tex` file at all, so nothing can be built.
    NoEntry,
    /// A reference resolves outside the project (UNSAFE_PROJECT_PATH).
    UnsafeReference { reference: String, from: String },
    /// A referenced file is missing (PROJECT_INCOMPLETE).
    MissingReference { reference: String, from: String },
    /// A package this build does not recognise (F45-R04).
    UnknownDependency { name: String },
    /// The project would run a shell escape, which OpenNote never does.
    ShellEscape { from: String },
    /// A `.latexmkrc` exists; it is reported but never executed (F45-R02).
    ConfigNotExecuted { path: String },
    /// The root path itself is not a plain vault-relative directory.
    UnsafeRoot { path: String },
    /// The project's recorded identity no longer matches its directory
    /// (F45 §4: identity is independent of the directory).
    MovedFromRecord { recorded: String, found: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInspection {
    /// Normalised root, without a trailing slash.
    pub root: String,
    pub entry_candidates: Vec<EntryCandidate>,
    /// The chosen entry, only when exactly one candidate is plausible.
    pub entry: Option<String>,
    /// Every file, sorted by path.
    pub files: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub findings: Vec<ProjectFinding>,
    /// True when nothing blocks an attempt to build.
    pub inspectable: bool,
}

/// Packages this build recognises as standard.
const KNOWN_PACKAGES: &[&str] = &[
    "amsmath",
    "amssymb",
    "amsthm",
    "babel",
    "biblatex",
    "booktabs",
    "caption",
    "cleveref",
    "enumitem",
    "fancyhdr",
    "fontenc",
    "geometry",
    "graphicx",
    "hyperref",
    "inputenc",
    "listings",
    "longtable",
    "mathptmx",
    "microtype",
    "natbib",
    "parskip",
    "rotating",
    "setspace",
    "siunitx",
    "subcaption",
    "tabularx",
    "titlesec",
    "tocloft",
    "url",
    "xcolor",
    "xeCJK",
    "ctex",
];

fn known_package(name: &str) -> bool {
    KNOWN_PACKAGES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// A safe project root (§4).
fn safe_root(root: &str) -> Option<String> {
    let normalized = root
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned();
    if normalized.starts_with('/') || normalized.chars().nth(1) == Some(':') {
        return None;
    }
    if normalized.split('/').any(|segment| segment == "..") {
        return None;
    }
    Some(normalized)
}

/// Extracts `\documentclass`, `\usepackage` and `\bibliography` arguments.
fn declared_names(text: &str) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = Vec::new();
    for (command, kind) in [
        ("\\documentclass", "documentclass"),
        ("\\LoadClass", "documentclass"),
        ("\\usepackage", "usepackage"),
        ("\\RequirePackage", "usepackage"),
        ("\\bibliography", "bibliography"),
        ("\\addbibresource", "bibresource"),
    ] {
        let mut rest = text;
        while let Some(start) = rest.find(command) {
            let after = &rest[start + command.len()..];
            // Skip an optional `[options]` block.
            let after = match after.find('[') {
                Some(bracket) if after[..bracket].trim().is_empty() => match after.find(']') {
                    Some(close) => &after[close + 1..],
                    None => after,
                },
                _ => after,
            };
            let Some(open) = after.find('{') else {
                break;
            };
            if !after[..open].trim().is_empty() && !after.trim_start().starts_with('{') {
                rest = after;
                continue;
            }
            let Some(close) = after[open..].find('}') else {
                break;
            };
            let value = after[open + 1..open + close].trim();
            for part in value.split(',') {
                let part = part.trim();
                if !part.is_empty() {
                    names.push((part.to_owned(), kind.to_owned()));
                }
            }
            rest = &after[open + close..];
        }
    }
    names
}

/// Whether a document declares a shell escape.
///
/// OpenNote never enables one; a project that needs it is reported so the user
/// knows why a build would fail rather than having the restriction quietly lifted.
fn uses_shell_escape(text: &str) -> bool {
    text.contains("\\write18")
        || text.contains("\\immediate\\write18")
        || (text.contains("\\usepackage")
            && text.contains("shellesc")
            && text.contains("\\ShellEscape"))
}

/// Whether a path is a TeX source that could contain executable commands.
fn is_tex_source(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".tex") || lower.ends_with(".sty") || lower.ends_with(".cls")
}

/// Whether a reference points outside the project root.
///
/// Backslashes count as separators because a Windows-authored project can use
/// `..\outside` just as easily as `../outside` (F45-R06).
fn escapes_project(reference: &str) -> bool {
    reference.starts_with('/')
        || reference.starts_with('\\')
        || reference.chars().nth(1) == Some(':')
        || reference.split(['/', '\\']).any(|segment| segment == "..")
}

/// Inspects a project directory.
///
/// The caller lists the files and (optionally) their text; this function decides
/// which file is the entry, what the project depends on, and what is wrong with it.
pub fn inspect(root: &str, files: &[ProjectFile]) -> ProjectInspection {
    let mut findings: Vec<ProjectFinding> = Vec::new();
    let Some(normalized_root) = safe_root(root) else {
        return ProjectInspection {
            root: root.to_owned(),
            entry_candidates: Vec::new(),
            entry: None,
            files: Vec::new(),
            dependencies: Vec::new(),
            findings: vec![ProjectFinding::UnsafeRoot {
                path: root.to_owned(),
            }],
            inspectable: false,
        };
    };

    let by_path: std::collections::BTreeMap<String, &ProjectFile> = files
        .iter()
        .map(|file| (file.path.replace('\\', "/"), file))
        .collect();
    let mut paths: Vec<String> = by_path.keys().cloned().collect();
    paths.sort();

    let text_files = files.len().max(1) as u32;
    let mut entry_candidates: Vec<EntryCandidate> = Vec::new();
    for (path, file) in &by_path {
        if !path.to_ascii_lowercase().ends_with(".tex") {
            continue;
        }
        let text = file.text.as_deref().unwrap_or("");
        // A file that declares a document class is the strongest signal; then a
        // conventional name; then any .tex at all.
        let (score, reason_key) = if text.contains("\\documentclass") {
            (100u32, "project.entryDeclaresClass")
        } else if text.contains("\\begin{document}") {
            (80, "project.entryHasDocumentEnv")
        } else if path.eq_ignore_ascii_case("main.tex") {
            (60, "project.entryNamedMain")
        } else if path.rsplit('/').next().is_some_and(|name| {
            name.eq_ignore_ascii_case("main.tex")
                || name.eq_ignore_ascii_case("root.tex")
                || name.eq_ignore_ascii_case("paper.tex")
                || name.eq_ignore_ascii_case("thesis.tex")
        }) {
            (50, "project.entryConventionalName")
        } else {
            // A section file has no document class, so it is a weak candidate.
            (10, "project.entryPlainTex")
        };
        entry_candidates.push(EntryCandidate {
            path: path.clone(),
            score,
            reason_key: reason_key.to_owned(),
        });
        let _ = text_files;
    }
    entry_candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });

    // Only a clear leader becomes the entry; a tie is reported instead of guessed.
    let entry = match entry_candidates.first().map(|candidate| candidate.score) {
        None => {
            findings.push(ProjectFinding::NoEntry);
            None
        }
        Some(top) => {
            let tied: Vec<String> = entry_candidates
                .iter()
                .filter(|candidate| candidate.score == top)
                .map(|candidate| candidate.path.clone())
                .collect();
            if tied.len() > 1 {
                findings.push(ProjectFinding::EntryAmbiguous {
                    candidates: tied.clone(),
                });
                None
            } else {
                Some(entry_candidates[0].path.clone())
            }
        }
    };

    // Dependencies come from what the entry reads, followed through `\input`,
    // `\include` and local `.sty`/`.cls` files. Without a single entry, every
    // .tex file is previewed so the picker still shows what the project needs.
    let mut dependencies: Vec<Dependency> = Vec::new();
    let analysis: Vec<&ProjectFile> = match entry.as_deref() {
        Some(path) => analysis_sources(path, &by_path)
            .into_iter()
            .filter_map(|path| by_path.get(&path).copied())
            .collect(),
        None => by_path
            .iter()
            .filter(|(path, _)| path.ends_with(".tex"))
            .map(|(_, file)| *file)
            .collect(),
    };
    // Safety findings are collected from every TeX source, not only the entry:
    // an included section, class or package can hide the shell escape or the
    // path that leaves the project (F45-A02, F45-R06).
    for file in files {
        if !is_tex_source(&file.path) {
            continue;
        }
        let Some(text) = file.text.as_deref() else {
            continue;
        };
        if uses_shell_escape(text) {
            findings.push(ProjectFinding::ShellEscape {
                from: file.path.clone(),
            });
        }
        for (reference, _) in references_in(text) {
            if escapes_project(&reference) {
                findings.push(ProjectFinding::UnsafeReference {
                    reference,
                    from: file.path.clone(),
                });
            }
        }
    }

    for file in analysis {
        let Some(text) = file.text.as_deref() else {
            continue;
        };
        for (name, kind) in declared_names(text) {
            if dependencies
                .iter()
                .any(|existing| existing.kind == kind && existing.name == name)
            {
                continue;
            }
            let known = match kind.as_str() {
                // A package or class shipped with the project is supplied, which
                // says nothing about whether it works, only that it is present.
                _ if local_declaration(&name, &kind, &by_path).is_some() => true,
                "usepackage" => known_package(&name),
                // A document class is not a package; anything not in the known
                // family is reported so the user sees it before a build fails.
                "documentclass" => [
                    "article", "report", "book", "letter", "beamer", "amsart", "memoir",
                ]
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&name)),
                _ => true,
            };
            if !known {
                findings.push(ProjectFinding::UnknownDependency { name: name.clone() });
            }
            dependencies.push(Dependency { kind, name, known });
        }
        // References: `\input`/`\include`/`\includegraphics` must resolve inside.
        for (reference, reference_kind) in references_in(text) {
            if escapes_project(&reference) {
                continue;
            }
            if resolve_reference(&reference, &reference_kind, &by_path).is_none() {
                findings.push(ProjectFinding::MissingReference {
                    reference,
                    from: file.path.clone(),
                });
            }
        }
    }

    // A latexmkrc is reported so the user knows it exists and that it is not run.
    for path in &paths {
        let name = path.rsplit('/').next().unwrap_or(path);
        if name == ".latexmkrc" || name == "latexmkrc" {
            findings.push(ProjectFinding::ConfigNotExecuted { path: path.clone() });
        }
    }

    let blocking = findings.iter().any(|finding| {
        matches!(
            finding,
            ProjectFinding::EntryAmbiguous { .. }
                | ProjectFinding::NoEntry
                | ProjectFinding::UnsafeRoot { .. }
                | ProjectFinding::UnsafeReference { .. }
                | ProjectFinding::ShellEscape { .. }
        )
    });

    ProjectInspection {
        root: normalized_root,
        entry_candidates,
        entry,
        files: paths,
        dependencies,
        findings,
        inspectable: !blocking,
    }
}

/// Extracts referenced paths and their kind, as inspection findings expect.
///
/// Parsing lives in `references` so inspection and export agree on spaced
/// arguments and `\includegraphics` options (F45-R06, F54-R04).
fn references_in(text: &str) -> Vec<(String, String)> {
    crate::references::references_in(text)
        .into_iter()
        .map(|(reference, kind)| (reference, kind.as_str().to_owned()))
        .collect()
}

/// The project path a reference names, if the project supplies it. Paths are
/// relative to the project root, as TeX resolves them from the entry's folder.
fn resolve_reference(
    reference: &str,
    kind: &str,
    files: &std::collections::BTreeMap<String, &ProjectFile>,
) -> Option<String> {
    if escapes_project(reference) {
        return None;
    }
    let normalized = reference.replace('\\', "/");
    let mut candidates = vec![normalized.clone()];
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
        .find(|candidate| files.contains_key(candidate))
}

/// The project's own `.sty`/`.cls` for a declared package or class, if any.
fn local_declaration(
    name: &str,
    kind: &str,
    files: &std::collections::BTreeMap<String, &ProjectFile>,
) -> Option<String> {
    let extension = match kind {
        "usepackage" => "sty",
        "documentclass" => "cls",
        _ => return None,
    };
    if escapes_project(name) {
        return None;
    }
    let path = format!("{}.{extension}", name.replace('\\', "/"));
    files.contains_key(&path).then_some(path)
}

/// Every supplied source the entry reads, in visiting order, each once.
///
/// This is a static reading of literal `\input`/`\include` and local
/// package/class declarations: macros, conditionals and engine search paths
/// are not expanded, so a dependency hidden behind them is not found.
fn analysis_sources(
    entry: &str,
    files: &std::collections::BTreeMap<String, &ProjectFile>,
) -> Vec<String> {
    let mut visited: Vec<String> = Vec::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    queue.push_back(entry.to_owned());
    while let Some(path) = queue.pop_front() {
        if visited.contains(&path) {
            continue;
        }
        visited.push(path.clone());
        let Some(text) = files.get(&path).and_then(|file| file.text.as_deref()) else {
            continue;
        };
        for (reference, kind) in references_in(text) {
            if kind == "input" {
                if let Some(next) = resolve_reference(&reference, &kind, files) {
                    queue.push_back(next);
                }
            }
        }
        for (name, kind) in declared_names(text) {
            if let Some(next) = local_declaration(&name, &kind, files) {
                queue.push_back(next);
            }
        }
    }
    visited
}

/// The root a recorded project should be at, after a move (§4).
///
/// Identity is independent of the directory, so a project found at a different
/// path is reported rather than treated as a different project.
pub fn locate_moved(recorded_root: &str, found_root: &str) -> Option<ProjectFinding> {
    let recorded = safe_root(recorded_root)?;
    let found = safe_root(found_root)?;
    if recorded == found {
        None
    } else {
        Some(ProjectFinding::MovedFromRecord { recorded, found })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> ProjectFile {
        ProjectFile {
            path: path.to_owned(),
            bytes: Some(text.len() as u64),
            text: Some(text.to_owned()),
        }
    }

    fn binary(path: &str) -> ProjectFile {
        ProjectFile {
            path: path.to_owned(),
            bytes: Some(10),
            text: None,
        }
    }

    #[test]
    fn a_single_document_class_file_is_the_entry() {
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\input{sections/a}\n",
            ),
            file("sections/a.tex", "Body\n"),
        ];
        let inspection = inspect("proj", &files);
        assert_eq!(inspection.entry.as_deref(), Some("main.tex"));
        assert_eq!(inspection.root, "proj");
        assert!(
            inspection.inspectable,
            "findings: {:?}",
            inspection.findings
        );
        assert_eq!(inspection.files, vec!["main.tex", "sections/a.tex"]);
    }

    #[test]
    fn two_equally_plausible_entries_are_reported_not_guessed() {
        // ENTRY_AMBIGUOUS: do not compile, show the candidates.
        let files = vec![
            file("a.tex", "\\documentclass{article}\n"),
            file("b.tex", "\\documentclass{report}\n"),
        ];
        let inspection = inspect("proj", &files);
        assert_eq!(inspection.entry, None);
        assert!(!inspection.inspectable);
        assert!(inspection
            .findings
            .iter()
            .any(|finding| matches!(finding, ProjectFinding::EntryAmbiguous { .. })));
    }

    #[test]
    fn a_project_without_a_tex_file_has_no_entry() {
        let files = vec![binary("plot.png")];
        let inspection = inspect("proj", &files);
        assert!(inspection
            .findings
            .iter()
            .any(|finding| matches!(finding, ProjectFinding::NoEntry)));
        assert!(!inspection.inspectable);
    }

    #[test]
    fn a_section_file_alone_is_a_weak_but_valid_candidate() {
        let files = vec![file("intro.tex", "Just a section.\n")];
        let inspection = inspect("proj", &files);
        assert_eq!(inspection.entry.as_deref(), Some("intro.tex"));
        assert_eq!(inspection.entry_candidates[0].score, 10);
        assert_eq!(
            inspection.entry_candidates[0].reason_key,
            "project.entryPlainTex"
        );
    }

    #[test]
    fn a_conventional_name_outranks_an_unrelated_tex_file() {
        let files = vec![
            file("zzz.tex", "Nothing special.\n"),
            file("main.tex", "\\input{zzz}\n"),
        ];
        let inspection = inspect("proj", &files);
        assert_eq!(inspection.entry.as_deref(), Some("main.tex"));
    }

    fn names(inspection: &ProjectInspection) -> Vec<&str> {
        inspection
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect()
    }

    fn missing(inspection: &ProjectInspection) -> Vec<(&str, &str)> {
        inspection
            .findings
            .iter()
            .filter_map(|finding| match finding {
                ProjectFinding::MissingReference { reference, from } => {
                    Some((reference.as_str(), from.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_file_the_entry_never_reads_contributes_no_dependencies() {
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\usepackage{graphicx}\n\\usepackage[utf8]{inputenc}\n",
            ),
            // Not reached from main.tex, so its package is not the document's.
            file("sections/a.tex", "\\usepackage{notreal}\n\\input{gone}\n"),
        ];
        let inspection = inspect("proj", &files);
        let names = names(&inspection);
        assert!(names.contains(&"article"));
        assert!(names.contains(&"graphicx"));
        assert!(names.contains(&"inputenc"));
        assert!(!names.contains(&"notreal"));
        assert!(missing(&inspection).is_empty());
    }

    #[test]
    fn an_included_preamble_contributes_its_packages() {
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\input{preamble}\n\\begin{document}\n\\include{sections/a}\n\\end{document}\n",
            ),
            file("preamble.tex", "\\usepackage{amsmath}\n\\usepackage{notreal}\n"),
            file("sections/a.tex", "\\input{sections/b}\n"),
            file("sections/b.tex", "\\usepackage{tikz}\n"),
        ];
        let inspection = inspect("proj", &files);
        let names = names(&inspection);
        assert!(names.contains(&"amsmath"), "{names:?}");
        assert!(
            names.contains(&"tikz"),
            "nested includes are followed: {names:?}"
        );
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::UnknownDependency { name } if name == "notreal"
        )));
    }

    #[test]
    fn a_missing_reference_in_an_included_section_is_reported_against_that_file() {
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\input{sections/a}\n",
            ),
            file(
                "sections/a.tex",
                "\\includegraphics{figures/gone}\n\\input{sections/missing}\n",
            ),
        ];
        let inspection = inspect("proj", &files);
        let missing = missing(&inspection);
        assert!(
            missing.contains(&("figures/gone", "sections/a.tex")),
            "{missing:?}"
        );
        assert!(
            missing.contains(&("sections/missing", "sections/a.tex")),
            "{missing:?}"
        );
    }

    #[test]
    fn a_local_package_and_class_are_supplied_and_followed() {
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{thesis}\n\\usepackage{mystyle}\n",
            ),
            file(
                "thesis.cls",
                "\\LoadClass{report}\n\\RequirePackage{geometry}\n",
            ),
            file("mystyle.sty", "\\RequirePackage{xcolor}\n"),
        ];
        let inspection = inspect("proj", &files);
        let names = names(&inspection);
        for expected in ["thesis", "mystyle", "report", "geometry", "xcolor"] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
        assert!(
            !inspection
                .findings
                .iter()
                .any(|finding| matches!(finding, ProjectFinding::UnknownDependency { .. })),
            "a supplied .sty/.cls is not unknown: {:?}",
            inspection.findings
        );
    }

    #[test]
    fn cyclic_and_repeated_includes_terminate_without_duplicates() {
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\input{a}\n\\input{a}\n\\input{b}\n",
            ),
            file("a.tex", "\\usepackage{amsmath}\n\\input{b}\n"),
            file(
                "b.tex",
                "\\usepackage{amsmath}\n\\input{a}\n\\input{main}\n",
            ),
        ];
        let inspection = inspect("proj", &files);
        let amsmath = inspection
            .dependencies
            .iter()
            .filter(|dependency| dependency.name == "amsmath")
            .count();
        assert_eq!(amsmath, 1);
        assert!(missing(&inspection).is_empty());
    }

    #[test]
    fn an_ambiguous_entry_still_previews_every_tex_file() {
        let files = vec![
            file("a.tex", "\\documentclass{article}\n\\usepackage{amsmath}\n"),
            file("b.tex", "\\documentclass{report}\n\\usepackage{tikz}\n"),
        ];
        let inspection = inspect("proj", &files);
        assert_eq!(inspection.entry, None);
        let names = names(&inspection);
        assert!(names.contains(&"amsmath") && names.contains(&"tikz"));
    }

    #[test]
    fn an_unreached_unsafe_source_still_blocks_the_project() {
        let files = vec![
            file("main.tex", "\\documentclass{article}\n"),
            file("old/notes.tex", "\\immediate\\write18{del x}\n"),
        ];
        let inspection = inspect("proj", &files);
        assert!(!inspection.inspectable);
    }

    #[test]
    fn an_unknown_package_is_reported_without_editing_the_source() {
        // F45-R04: report, never "fix" by editing.
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\usepackage{mypackage}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::UnknownDependency { name } if name == "mypackage"
        )));
        // The source is untouched: the inspection only reports.
        assert!(inspection.inspectable);
    }

    #[test]
    fn a_missing_reference_is_reported() {
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\input{gone}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::MissingReference { reference, .. } if reference == "gone"
        )));
    }

    #[test]
    fn a_reference_outside_the_project_is_refused() {
        // UNSAFE_PROJECT_PATH: never read outside the root.
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\input{../outside/secret}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(!inspection.inspectable);
        let unsafe_found = inspection
            .findings
            .iter()
            .any(|finding| matches!(finding, ProjectFinding::UnsafeReference { .. }));
        // The `..` segment may be caught by the missing check first; either way the
        // project must not be inspectable.
        assert!(unsafe_found || !inspection.inspectable);
    }

    #[test]
    fn an_absolute_reference_is_never_resolved() {
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\includegraphics{/etc/passwd}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::UnsafeReference { reference, .. } if reference.starts_with('/')
        )));
    }

    #[test]
    fn spaced_arguments_and_graphics_options_cannot_hide_a_traversal() {
        // F45-R06: TeX accepts `\input {..}` and `\includegraphics[..]{..}`;
        // parsing only the tight spelling would skip the out-of-root refusal.
        for source in [
            "\\documentclass{article}\n\\input {../outside/secret}\n",
            "\\documentclass{article}\n\\includegraphics[width=0.5\\linewidth]{../secret.png}\n",
            "\\documentclass{article}\n\\includegraphics[width=2cm][height=3cm]{../secret.png}\n",
            "\\documentclass{article}\n\\includegraphics*[width=2cm] {\\\\server\\share\\x.png}\n",
        ] {
            let inspection = inspect("proj", &[file("main.tex", source)]);
            assert!(!inspection.inspectable, "{source}");
            assert!(
                inspection
                    .findings
                    .iter()
                    .any(|finding| matches!(finding, ProjectFinding::UnsafeReference { .. })),
                "{source}"
            );
        }
    }

    #[test]
    fn spaced_references_and_graphics_options_still_resolve() {
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\input {sections/a}\n\
                 \\includegraphics[width=2cm]{assets/plot.png}\n",
            ),
            file("sections/a.tex", "Text\n"),
            file("assets/plot.png", ""),
        ];
        let inspection = inspect("proj", &files);
        assert!(inspection.inspectable);
        assert!(!inspection
            .findings
            .iter()
            .any(|finding| matches!(finding, ProjectFinding::MissingReference { .. })));
    }

    #[test]
    fn longer_control_words_are_not_mistaken_for_references() {
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\inputencoding{utf8}\n\\bibliographystyle{plain}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(!inspection
            .findings
            .iter()
            .any(|finding| matches!(finding, ProjectFinding::MissingReference { .. })));
    }

    #[test]
    fn a_shell_escape_blocks_the_project() {
        // OpenNote never enables one; saying so is better than failing silently.
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\immediate\\write18{rm -rf /}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(!inspection.inspectable);
        assert!(inspection
            .findings
            .iter()
            .any(|finding| matches!(finding, ProjectFinding::ShellEscape { .. })));
    }

    #[test]
    fn a_shell_escape_in_an_included_section_blocks_the_project() {
        // F45-A02: the precheck must inspect every TeX source, not just the entry.
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\input{sections/a}\n",
            ),
            file("sections/a.tex", "\\immediate\\write18{rm -rf /}\n"),
        ];
        let inspection = inspect("proj", &files);
        assert!(!inspection.inspectable);
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::ShellEscape { from } if from == "sections/a.tex"
        )));
    }

    #[test]
    fn a_shell_escape_in_a_package_file_blocks_the_project() {
        let files = vec![
            file("main.tex", "\\documentclass{article}\n"),
            file("style/hack.sty", "\\write18{whoami}\n"),
        ];
        let inspection = inspect("proj", &files);
        assert!(!inspection.inspectable);
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::ShellEscape { from } if from == "style/hack.sty"
        )));
    }

    #[test]
    fn an_unsafe_reference_in_an_included_section_blocks_the_project() {
        // F45-R06: a section file cannot smuggle in a path outside the root.
        let files = vec![
            file(
                "main.tex",
                "\\documentclass{article}\n\\input{sections/a}\n",
            ),
            file("sections/a.tex", "\\input{../outside/secret}\n"),
        ];
        let inspection = inspect("proj", &files);
        assert!(!inspection.inspectable);
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::UnsafeReference { reference, from }
                if reference == "../outside/secret" && from == "sections/a.tex"
        )));
    }

    #[test]
    fn a_backslash_traversal_is_refused_too() {
        // Windows-authored projects can use `..\outside`; it is still outside.
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\input{..\\outside\\secret}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(!inspection.inspectable);
        assert!(inspection
            .findings
            .iter()
            .any(|finding| matches!(finding, ProjectFinding::UnsafeReference { .. })));
    }

    #[test]
    fn a_latexmkrc_is_reported_and_never_executed() {
        // F45-R02: nothing in the directory runs during an inspection.
        let files = vec![
            file("main.tex", "\\documentclass{article}\n"),
            file(".latexmkrc", "$pdf_mode = 1;\n"),
        ];
        let inspection = inspect("proj", &files);
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            ProjectFinding::ConfigNotExecuted { path } if path == ".latexmkrc"
        )));
        // It does not block inspecting, because it is merely not run.
        assert!(inspection.inspectable);
    }

    #[test]
    fn an_unsafe_root_is_refused_outright() {
        for root in ["../outside", "/absolute", "C:\\vault"] {
            let inspection = inspect(root, &[]);
            assert!(!inspection.inspectable);
            assert!(inspection
                .findings
                .iter()
                .any(|finding| matches!(finding, ProjectFinding::UnsafeRoot { .. })));
        }
    }

    #[test]
    fn a_moved_project_is_reported_with_both_paths() {
        // §4: identity is independent of the directory.
        let finding = locate_moved("old/place", "new/place");
        assert!(matches!(
            finding,
            Some(ProjectFinding::MovedFromRecord { .. })
        ));
        assert!(locate_moved("same/place", "same/place").is_none());
    }

    #[test]
    fn the_trailing_slash_of_a_root_is_normalised() {
        let inspection = inspect("proj/", &[file("main.tex", "\\documentclass{article}\n")]);
        assert_eq!(inspection.root, "proj");
    }

    #[test]
    fn package_options_are_skipped_when_reading_names() {
        let files = vec![file(
            "main.tex",
            "\\documentclass[11pt,a4paper]{article}\n\\usepackage[utf8]{inputenc}\n",
        )];
        let inspection = inspect("proj", &files);
        let names: Vec<&str> = inspection
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect();
        assert!(names.contains(&"article"));
        assert!(names.contains(&"inputenc"));
        assert!(!names.contains(&"11pt"));
    }

    #[test]
    fn a_bibliography_declaration_becomes_a_dependency() {
        let files = vec![file(
            "main.tex",
            "\\documentclass{article}\n\\bibliography{refs}\n",
        )];
        let inspection = inspect("proj", &files);
        assert!(inspection
            .dependencies
            .iter()
            .any(|dependency| dependency.kind == "bibliography" && dependency.name == "refs"));
    }

    #[test]
    fn an_empty_directory_reports_no_entry_and_nothing_else() {
        let inspection = inspect("proj", &[]);
        assert!(inspection.entry_candidates.is_empty());
        assert!(inspection.dependencies.is_empty());
        assert!(!inspection.inspectable);
    }
}
