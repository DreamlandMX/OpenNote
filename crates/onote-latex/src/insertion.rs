//! M13 structured LaTeX insertion (F48).
//!
//! Each kind of insertion has its own parameters because they are not
//! interchangeable: an equation body is TeX source and must not be escaped, while
//! a section title is plain text and must be. Treating both as "text" is the bug
//! this module exists to prevent.
//!
//! Three further rules:
//!
//! * a label must be unique inside the project, and the suggested prefixes keep
//!   `eq:`/`fig:`/`tab:` references readable (§4);
//! * a missing package becomes a reported dependency, never a silent edit of the
//!   preamble (F48-R03);
//! * the fragment and any dependency change are returned together so the caller
//!   can put them in one undo group (F48-R04).
//!
//! Nothing here writes a file.

use serde::{Deserialize, Serialize};

/// The kinds the assistant can insert (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InsertKind {
    Equation,
    Image,
    Table,
    Section,
    Reference,
}

impl InsertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Equation => "equation",
            Self::Image => "image",
            Self::Table => "table",
            Self::Section => "section",
            Self::Reference => "reference",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "equation" => Some(Self::Equation),
            "image" => Some(Self::Image),
            "table" => Some(Self::Table),
            "section" => Some(Self::Section),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }

    /// The prefix a label of this kind should carry (§4).
    pub fn label_prefix(self) -> Option<&'static str> {
        match self {
            Self::Equation => Some("eq:"),
            Self::Image => Some("fig:"),
            Self::Table => Some("tab:"),
            Self::Section | Self::Reference => None,
        }
    }
}

/// Escapes plain text for LaTeX.
///
/// Deliberately duplicated from the scaffold encoder's rules but kept separate in
/// meaning: this one is applied to *titles and captions*, and the equation body
/// never goes through it (F48-R01).
pub fn escape_plain(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 8);
    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\textbackslash{}"),
            '{' => output.push_str("\\{"),
            '}' => output.push_str("\\}"),
            '%' => output.push_str("\\%"),
            '&' => output.push_str("\\&"),
            '_' => output.push_str("\\_"),
            '$' => output.push_str("\\$"),
            '#' => output.push_str("\\#"),
            '^' => output.push_str("\\textasciicircum{}"),
            '~' => output.push_str("\\textasciitilde{}"),
            other => output.push(other),
        }
    }
    output
}

/// Whether a string is a legal label.
///
/// LaTeX labels are fragile with spaces and braces, so only letters, digits and
/// the conventional `: - _ .` separators are allowed.
pub fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 128
        && label.chars().all(|character| {
            character.is_alphanumeric() || matches!(character, ':' | '-' | '_' | '.')
        })
}

/// One insertion request, with only the fields that kind uses.
// `PartialEq` only: an image width is an f64, which has no `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Parameters {
    Equation {
        /// TeX source, kept verbatim (F48-R01).
        body: String,
        label: Option<String>,
    },
    Image {
        /// Project-relative path.
        path: String,
        /// Fraction of the line width, 0.1–1.0 (§4).
        width: f64,
        caption: Option<String>,
        label: Option<String>,
    },
    Table {
        /// Plain-text cells, escaped (F48-R02).
        rows: Vec<Vec<String>>,
        /// Raw TeX rows, used instead of `rows` when present.
        advanced: Option<String>,
        caption: Option<String>,
        label: Option<String>,
    },
    Section {
        title: String,
        /// 1–3.
        level: u8,
        label: Option<String>,
    },
    Reference {
        label: String,
    },
}

// `PartialEq` only: `InvalidWidth` carries an f64, which has no `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InsertionIssue {
    /// A required field is missing.
    Missing { field: String },
    /// A label is empty or contains characters LaTeX treats specially.
    InvalidLabel { label: String },
    /// Another label in the project already uses this name (§4).
    DuplicateLabel {
        label: String,
        existing: Vec<String>,
    },
    /// An image path points outside the project.
    UnsafeImagePath { path: String },
    /// An image width outside 0.1–1.0.
    InvalidWidth { value: f64 },
    /// A table has no rows, or a row has a different cell count than the first.
    MalformedTable { detail: String },
    /// A section level outside 1–3.
    InvalidLevel { level: u8 },
    /// A referenced label does not exist in the project.
    UnknownLabel { label: String },
}

/// A package the fragment needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    /// `usepackage` or `usetikzlibrary`.
    pub kind: String,
    pub name: String,
    /// The line that would be added, shown to the user before anything changes.
    pub line: String,
    /// True when the project already loads it.
    pub present: bool,
}

/// The plan the user confirms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertionPlan {
    /// The fragment, or empty when an issue blocks it.
    pub fragment: String,
    pub dependencies: Vec<Dependency>,
    pub issues: Vec<InsertionIssue>,
    /// The label actually used, when one was requested.
    pub label: Option<String>,
}

impl InsertionPlan {
    /// True when the fragment may be inserted as-is.
    pub fn is_ready(&self) -> bool {
        self.issues.is_empty() && !self.fragment.is_empty()
    }

    /// Dependencies that would need a preamble change.
    pub fn missing_dependencies(&self) -> Vec<&Dependency> {
        self.dependencies
            .iter()
            .filter(|entry| !entry.present)
            .collect()
    }
}

/// Labels already used in the project, so a new one can be checked.
pub fn collect_labels(source: &str) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("\\label{") {
        let after = &rest[start + 7..];
        let Some(end) = after.find('}') else {
            break;
        };
        let label = after[..end].trim();
        if !label.is_empty() && !labels.iter().any(|existing| existing == label) {
            labels.push(label.to_owned());
        }
        rest = &after[end..];
    }
    labels.sort();
    labels
}

fn dependency(kind: &str, name: &str, line: &str, preamble: &str) -> Dependency {
    Dependency {
        kind: kind.to_owned(),
        name: name.to_owned(),
        line: line.to_owned(),
        // A package counts as present when a "usepackage" line already names it,
        // including inside a comma-separated option list.
        present: preamble.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("\\usepackage")
                && (trimmed.contains(&format!("{{{name}}}"))
                    || trimmed.contains(&format!("{{{name},"))
                    || trimmed.contains(&format!(",{name}}}"))
                    || trimmed.contains(&format!(",{name},")))
        }),
    }
}

fn check_label(
    label: &Option<String>,
    existing: &[String],
) -> (Option<InsertionIssue>, Option<String>) {
    match label.as_deref() {
        None => (None, None),
        Some(value) if value.trim().is_empty() => (
            Some(InsertionIssue::Missing {
                field: "label".to_owned(),
            }),
            None,
        ),
        Some(value) => {
            if !valid_label(value) {
                return (
                    Some(InsertionIssue::InvalidLabel {
                        label: value.to_owned(),
                    }),
                    None,
                );
            }
            if existing.iter().any(|used| used == value) {
                return (
                    Some(InsertionIssue::DuplicateLabel {
                        label: value.to_owned(),
                        existing: existing.to_vec(),
                    }),
                    None,
                );
            }
            (None, Some(value.to_owned()))
        }
    }
}

/// A safe project-relative image path (§4).
fn safe_image_path(path: &str) -> Result<String, InsertionIssue> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err(InsertionIssue::Missing {
            field: "path".to_owned(),
        });
    }
    if normalized.starts_with('/')
        || normalized.chars().nth(1) == Some(':')
        || normalized.split('/').any(|segment| segment == "..")
    {
        // A path that escapes the project would read outside it (F48-R06).
        return Err(InsertionIssue::UnsafeImagePath {
            path: path.to_owned(),
        });
    }
    Ok(normalized)
}

/// Plans one insertion.
///
/// `existing_labels` and `preamble` describe the project so the plan can check a
/// duplicate label and an already-loaded package without touching a file.
pub fn plan(parameters: &Parameters, existing_labels: &[String], preamble: &str) -> InsertionPlan {
    let mut issues: Vec<InsertionIssue> = Vec::new();
    let mut dependencies: Vec<Dependency> = Vec::new();
    let mut fragment = String::new();
    let mut used_label: Option<String> = None;

    match parameters {
        Parameters::Equation { body, label } => {
            if body.trim().is_empty() {
                issues.push(InsertionIssue::Missing {
                    field: "body".to_owned(),
                });
            }
            let (issue, label_value) = check_label(label, existing_labels);
            if let Some(issue) = issue {
                issues.push(issue);
            }
            used_label = label_value;
            if issues.is_empty() {
                // The body is TeX source: it is inserted verbatim (F48-R01).
                fragment.push_str("\\begin{equation}\n");
                if let Some(value) = used_label.as_deref() {
                    fragment.push_str(&format!("  {body}\n  \\label{{{value}}}\n"));
                } else {
                    fragment.push_str(&format!("  {body}\n"));
                }
                fragment.push_str("\\end{equation}\n");
            }
        }
        Parameters::Image {
            path,
            width,
            caption,
            label,
        } => {
            let path_value = match safe_image_path(path) {
                Ok(value) => Some(value),
                Err(issue) => {
                    issues.push(issue);
                    None
                }
            };
            if !(0.1..=1.0).contains(width) {
                issues.push(InsertionIssue::InvalidWidth { value: *width });
            }
            let (issue, label_value) = check_label(label, existing_labels);
            if let Some(issue) = issue {
                issues.push(issue);
            }
            used_label = label_value;
            if issues.is_empty() {
                let path_value = path_value.unwrap_or_default();
                fragment.push_str("\\begin{figure}[htbp]\n  \\centering\n");
                fragment.push_str(&format!(
                    "  \\includegraphics[width={:.2}\\linewidth]{{{path_value}}}\n",
                    width
                ));
                if let Some(value) = caption.as_deref().filter(|value| !value.trim().is_empty()) {
                    // A caption is plain text, so it is escaped.
                    fragment.push_str(&format!("  \\caption{{{}}}\n", escape_plain(value)));
                }
                if let Some(value) = used_label.as_deref() {
                    fragment.push_str(&format!("  \\label{{{value}}}\n"));
                }
                fragment.push_str("\\end{figure}\n");
            }
        }
        Parameters::Table {
            rows,
            advanced,
            caption,
            label,
        } => {
            let body = match advanced.as_deref().filter(|value| !value.trim().is_empty()) {
                // Advanced mode is raw TeX, kept as written (F48-R02).
                Some(raw) => raw.to_owned(),
                None => {
                    if rows.is_empty() {
                        issues.push(InsertionIssue::MalformedTable {
                            detail: "no rows".to_owned(),
                        });
                        String::new()
                    } else {
                        let columns = rows[0].len();
                        if columns == 0 {
                            issues.push(InsertionIssue::MalformedTable {
                                detail: "no columns".to_owned(),
                            });
                        }
                        for (index, row) in rows.iter().enumerate() {
                            if row.len() != columns {
                                issues.push(InsertionIssue::MalformedTable {
                                    detail: format!(
                                        "row {} has {} cells, expected {}",
                                        index + 1,
                                        row.len(),
                                        columns
                                    ),
                                });
                                break;
                            }
                        }
                        if columns == 0 {
                            String::new()
                        } else {
                            let mut built = String::new();
                            built.push_str("\\begin{tabular}{");
                            for _ in 0..columns {
                                built.push('l');
                            }
                            built.push_str("}\n");
                            for (index, row) in rows.iter().enumerate() {
                                let cells: Vec<String> =
                                    row.iter().map(|cell| escape_plain(cell)).collect();
                                built.push_str("  ");
                                built.push_str(&cells.join(" & "));
                                built.push_str(if index + 1 == rows.len() {
                                    " \\\\\n"
                                } else {
                                    " \\\\\n  \\hline\n"
                                });
                            }
                            built.push_str("\\end{tabular}\n");
                            built
                        }
                    }
                }
            };
            let (issue, label_value) = check_label(label, existing_labels);
            if let Some(issue) = issue {
                issues.push(issue);
            }
            used_label = label_value;
            if issues.is_empty() && !body.is_empty() {
                fragment.push_str("\\begin{table}[htbp]\n  \\centering\n");
                if let Some(value) = caption.as_deref().filter(|value| !value.trim().is_empty()) {
                    fragment.push_str(&format!("  \\caption{{{}}}\n", escape_plain(value)));
                }
                if let Some(value) = used_label.as_deref() {
                    fragment.push_str(&format!("  \\label{{{value}}}\n"));
                }
                for line in body.lines() {
                    fragment.push_str("  ");
                    fragment.push_str(line);
                    fragment.push('\n');
                }
                fragment.push_str("\\end{table}\n");
            }
        }
        Parameters::Section {
            title,
            level,
            label,
        } => {
            if title.trim().is_empty() {
                issues.push(InsertionIssue::Missing {
                    field: "title".to_owned(),
                });
            }
            if !(1..=3).contains(level) {
                issues.push(InsertionIssue::InvalidLevel { level: *level });
            }
            let (issue, label_value) = check_label(label, existing_labels);
            if let Some(issue) = issue {
                issues.push(issue);
            }
            used_label = label_value;
            if issues.is_empty() {
                let command = match level {
                    1 => "section",
                    2 => "subsection",
                    _ => "subsubsection",
                };
                // A title is plain text, so it is escaped (F48-R01).
                fragment.push_str(&format!("\\{command}{{{}}}\n", escape_plain(title)));
                if let Some(value) = used_label.as_deref() {
                    fragment.push_str(&format!("\\label{{{value}}}\n"));
                }
            }
        }
        Parameters::Reference { label } => {
            if !valid_label(label) {
                issues.push(InsertionIssue::InvalidLabel {
                    label: label.clone(),
                });
            } else if !existing_labels.iter().any(|used| used == label) {
                // F48-R04's counterpart: never insert a reference to nothing.
                issues.push(InsertionIssue::UnknownLabel {
                    label: label.clone(),
                });
            }
            if issues.is_empty() {
                fragment.push_str(&format!("\\ref{{{label}}}\n"));
            }
        }
    }

    match parameters {
        Parameters::Image { .. } => {
            dependencies.push(dependency(
                "usepackage",
                "graphicx",
                "\\usepackage{graphicx}",
                preamble,
            ));
        }
        Parameters::Table {
            advanced: Some(_), ..
        } => {
            dependencies.push(dependency(
                "usepackage",
                "booktabs",
                "\\usepackage{booktabs}",
                preamble,
            ));
        }
        _ => {}
    }

    InsertionPlan {
        fragment,
        dependencies,
        issues,
        label: used_label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local alias so a test variable named `plan` does not shadow the function.
    fn plan_of(parameters: &Parameters, labels: &[String], preamble: &str) -> InsertionPlan {
        plan(parameters, labels, preamble)
    }

    #[test]
    fn an_equation_body_is_inserted_verbatim_not_escaped() {
        // F48-R01: a formula is TeX source, not plain text.
        let plan = plan_of(
            &Parameters::Equation {
                body: "a_b^2 = \\frac{1}{2}".to_owned(),
                label: None,
            },
            &[],
            "",
        );
        assert!(plan.is_ready());
        assert!(plan.fragment.contains("a_b^2 = \\frac{1}{2}"));
        // No escaping artefacts appear.
        assert!(!plan.fragment.contains("\\textbackslash"));
    }

    #[test]
    fn a_section_title_is_escaped() {
        // F48-R01: a title is plain text.
        let plan = plan_of(
            &Parameters::Section {
                title: "R&D 50%".to_owned(),
                level: 1,
                label: None,
            },
            &[],
            "",
        );
        assert!(plan.fragment.contains("\\section{R\\&D 50\\%}"));
    }

    #[test]
    fn a_duplicate_label_is_reported_with_the_existing_ones() {
        // §4: a label must be unique inside the project.
        let plan = plan_of(
            &Parameters::Equation {
                body: "x".to_owned(),
                label: Some("eq:first".to_owned()),
            },
            &["eq:first".to_owned()],
            "",
        );
        assert!(!plan.is_ready());
        assert!(plan.issues.iter().any(|issue| matches!(
            issue,
            InsertionIssue::DuplicateLabel { label, .. } if label == "eq:first"
        )));
    }

    #[test]
    fn an_invalid_label_is_refused() {
        for label in ["", "has space", "has{brace", "tab#1"] {
            let plan = plan_of(
                &Parameters::Equation {
                    body: "x".to_owned(),
                    label: Some(label.to_owned()),
                },
                &[],
                "",
            );
            assert!(
                plan.issues.iter().any(|issue| matches!(
                    issue,
                    InsertionIssue::InvalidLabel { .. } | InsertionIssue::Missing { .. }
                )),
                "{label} should be refused"
            );
        }
    }

    #[test]
    fn an_absent_package_becomes_a_reported_dependency() {
        // F48-R03: never silently edit the preamble.
        let plan = plan_of(
            &Parameters::Image {
                path: "assets/plot.png".to_owned(),
                width: 0.8,
                caption: None,
                label: None,
            },
            &[],
            "\\documentclass{article}",
        );
        assert!(plan.is_ready());
        let missing = plan.missing_dependencies();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].name, "graphicx");
        assert_eq!(missing[0].line, "\\usepackage{graphicx}");
    }

    #[test]
    fn a_package_already_in_the_preamble_is_not_reported_as_missing() {
        let plan = plan_of(
            &Parameters::Image {
                path: "a.png".to_owned(),
                width: 0.5,
                caption: None,
                label: None,
            },
            &[],
            "\\usepackage{graphicx}",
        );
        assert!(plan.dependencies[0].present);
        assert!(plan.missing_dependencies().is_empty());
    }

    #[test]
    fn an_image_width_outside_the_documented_range_is_refused() {
        for width in [0.0, 0.09, 1.01, 2.0] {
            let plan = plan_of(
                &Parameters::Image {
                    path: "a.png".to_owned(),
                    width,
                    caption: None,
                    label: None,
                },
                &[],
                "\\usepackage{graphicx}",
            );
            assert!(
                plan.issues
                    .iter()
                    .any(|issue| matches!(issue, InsertionIssue::InvalidWidth { .. })),
                "{width} should be refused"
            );
        }
        // The boundaries themselves are allowed.
        for width in [0.1, 1.0] {
            let plan = plan_of(
                &Parameters::Image {
                    path: "a.png".to_owned(),
                    width,
                    caption: None,
                    label: None,
                },
                &[],
                "\\usepackage{graphicx}",
            );
            assert!(plan.is_ready(), "{width} should be allowed");
        }
    }

    #[test]
    fn an_image_path_outside_the_project_is_refused() {
        // F48-R06: never read outside the project.
        for path in ["../secret.png", "/etc/passwd", "C:\\Users\\a.png"] {
            let plan = plan_of(
                &Parameters::Image {
                    path: path.to_owned(),
                    width: 0.5,
                    caption: None,
                    label: None,
                },
                &[],
                "\\usepackage{graphicx}",
            );
            assert!(
                plan.issues
                    .iter()
                    .any(|issue| matches!(issue, InsertionIssue::UnsafeImagePath { .. })),
                "{path} should be refused"
            );
        }
    }

    #[test]
    fn a_plain_table_escapes_every_cell_and_needs_matching_rows() {
        let plan = plan_of(
            &Parameters::Table {
                rows: vec![
                    vec!["a&b".to_owned(), "c_d".to_owned()],
                    vec!["1%".to_owned(), "2".to_owned()],
                ],
                advanced: None,
                caption: Some("R&D".to_owned()),
                label: Some("tab:one".to_owned()),
            },
            &[],
            "",
        );
        assert!(plan.is_ready(), "issues: {:?}", plan.issues);
        assert!(plan.fragment.contains("a\\&b & c\\_d"));
        assert!(plan.fragment.contains("1\\% & 2"));
        assert!(plan.fragment.contains("\\caption{R\\&D}"));
        assert!(plan.fragment.contains("\\label{tab:one}"));
    }

    #[test]
    fn a_ragged_table_is_refused_with_the_differing_row() {
        let plan = plan_of(
            &Parameters::Table {
                rows: vec![vec!["a".to_owned()], vec!["b".to_owned(), "c".to_owned()]],
                advanced: None,
                caption: None,
                label: None,
            },
            &[],
            "",
        );
        assert!(!plan.is_ready());
        assert!(plan.issues.iter().any(|issue| matches!(
            issue,
            InsertionIssue::MalformedTable { detail } if detail.contains("row 2")
        )));
    }

    #[test]
    fn the_advanced_table_mode_keeps_raw_tex_and_reports_booktabs() {
        // F48-R02: plain cells and advanced source are different modes.
        let plan = plan_of(
            &Parameters::Table {
                rows: Vec::new(),
                advanced: Some("\\toprule\nA & B \\\\\n\\bottomrule".to_owned()),
                caption: None,
                label: None,
            },
            &[],
            "",
        );
        assert!(plan.is_ready());
        assert!(plan.fragment.contains("\\toprule"));
        assert!(plan
            .missing_dependencies()
            .iter()
            .any(|entry| entry.name == "booktabs"));
    }

    #[test]
    fn a_section_level_outside_one_to_three_is_refused() {
        for level in [0u8, 4] {
            let plan = plan_of(
                &Parameters::Section {
                    title: "x".to_owned(),
                    level,
                    label: None,
                },
                &[],
                "",
            );
            assert!(plan
                .issues
                .iter()
                .any(|issue| matches!(issue, InsertionIssue::InvalidLevel { .. })));
        }
        // Levels 1-3 map to the three documented commands.
        for (level, command) in [
            (1u8, "\\section"),
            (2, "\\subsection"),
            (3, "\\subsubsection"),
        ] {
            let plan = plan_of(
                &Parameters::Section {
                    title: "x".to_owned(),
                    level,
                    label: None,
                },
                &[],
                "",
            );
            assert!(plan.fragment.starts_with(command), "level {level}");
        }
    }

    #[test]
    fn a_reference_to_an_unknown_label_is_refused() {
        // Inserting a reference to nothing would produce a broken document.
        let plan = plan_of(
            &Parameters::Reference {
                label: "eq:missing".to_owned(),
            },
            &["eq:present".to_owned()],
            "",
        );
        assert!(!plan.is_ready());
        assert!(plan
            .issues
            .iter()
            .any(|issue| matches!(issue, InsertionIssue::UnknownLabel { .. })));

        let ok = plan_of(
            &Parameters::Reference {
                label: "eq:present".to_owned(),
            },
            &["eq:present".to_owned()],
            "",
        );
        assert!(ok.is_ready());
        assert_eq!(ok.fragment, "\\ref{eq:present}\n");
    }

    #[test]
    fn an_empty_required_field_is_refused_for_every_kind() {
        let cases = [
            Parameters::Equation {
                body: "   ".to_owned(),
                label: None,
            },
            Parameters::Section {
                title: "".to_owned(),
                level: 1,
                label: None,
            },
            Parameters::Image {
                path: "  ".to_owned(),
                width: 0.5,
                caption: None,
                label: None,
            },
        ];
        for parameters in cases {
            let plan = plan_of(&parameters, &[], "\\usepackage{graphicx}");
            assert!(!plan.is_ready(), "{parameters:?} should be refused");
            assert!(plan
                .issues
                .iter()
                .any(|issue| matches!(issue, InsertionIssue::Missing { .. })));
        }
    }

    #[test]
    fn labels_are_collected_from_the_source_without_duplicates() {
        let labels = collect_labels("\\label{eq:a}\n\\label{fig:b}\n\\label{eq:a}\n");
        assert_eq!(labels, vec!["eq:a".to_owned(), "fig:b".to_owned()]);
        assert!(collect_labels("no labels").is_empty());
    }

    #[test]
    fn the_label_prefixes_match_the_documented_convention() {
        assert_eq!(InsertKind::Equation.label_prefix(), Some("eq:"));
        assert_eq!(InsertKind::Image.label_prefix(), Some("fig:"));
        assert_eq!(InsertKind::Table.label_prefix(), Some("tab:"));
        assert_eq!(InsertKind::Section.label_prefix(), None);
        assert_eq!(InsertKind::Reference.label_prefix(), None);
    }

    #[test]
    fn kinds_round_trip_and_an_unknown_one_is_rejected() {
        for kind in [
            InsertKind::Equation,
            InsertKind::Image,
            InsertKind::Table,
            InsertKind::Section,
            InsertKind::Reference,
        ] {
            assert_eq!(InsertKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(InsertKind::parse("figure*"), None);
    }

    #[test]
    fn an_optional_empty_label_is_treated_as_absent_not_invalid() {
        let plan = plan_of(
            &Parameters::Section {
                title: "x".to_owned(),
                level: 1,
                label: None,
            },
            &[],
            "",
        );
        assert!(plan.is_ready());
        assert_eq!(plan.label, None);
        assert!(!plan.fragment.contains("\\label"));
    }
}
