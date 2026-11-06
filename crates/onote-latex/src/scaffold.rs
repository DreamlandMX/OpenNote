//! M13 LaTeX project scaffolding (F46).
//!
//! A wizard that writes a project must not be clever. Three rules shape it:
//!
//! * the escaping is one deterministic encoder, not ad-hoc replacements scattered
//!   through the templates (F46 §5.1), so a title with `&` or `%` cannot break the
//!   generated source in one place and not another;
//! * nothing is created until the caller confirms a collision-free plan, and a
//!   non-empty destination is reported rather than overwritten (F46-R03);
//! * the document language is an independent field: the wizard's own UI language
//!   only chooses the *default* on first open, and changing the UI later never
//!   rewrites a language the user already picked (F46-R07).
//!
//! Nothing here reads or writes a file. The caller receives a file list and
//! publishes it through M02 once, which is what keeps creation idempotent.

use serde::{Deserialize, Serialize};

/// Longest accepted title, in characters (F46 §4).
pub const MAX_TITLE_CHARS: usize = 256;
/// Longest accepted author, in characters.
pub const MAX_AUTHOR_CHARS: usize = 128;

/// The three templates this build ships, each in two document languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TemplateId {
    Report,
    Paper,
    Assignment,
}

impl TemplateId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Paper => "paper",
            Self::Assignment => "assignment",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "report" => Some(Self::Report),
            "paper" => Some(Self::Paper),
            "assignment" => Some(Self::Assignment),
            _ => None,
        }
    }
}

/// The document's language. Deliberately separate from the interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentLanguage {
    ZhCn,
    En,
}

impl DocumentLanguage {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        }
    }

    /// The default a wizard shows on first open (F46-R07).
    ///
    /// Only `zh-CN` and `zh-TW` interfaces default to Chinese; every other
    /// interface defaults to English. This is a default, never a rewrite.
    pub fn default_for_ui(locale: &str) -> Self {
        if locale == "zh-CN" || locale == "zh-TW" {
            Self::ZhCn
        } else {
            Self::En
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaperSize {
    A4,
    Letter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub title: String,
    /// Empty means the author line is omitted entirely (F46 §5.1).
    pub author: String,
    /// Calendar day, `YYYY-MM-DD`; formatted into the document as written.
    pub date: String,
    pub language: DocumentLanguage,
    pub paper: PaperSize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// Internally tagged, like the other M13 enums, so the UI reads kind.
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum MetadataError {
    TitleEmpty,
    TitleTooLong { chars: usize, limit: usize },
    AuthorTooLong { chars: usize, limit: usize },
    InvalidDate { value: String },
}

/// Validates the metadata before anything is generated (METADATA_INVALID).
pub fn validate(metadata: &Metadata) -> Result<(), MetadataError> {
    let title_chars = metadata.title.chars().count();
    if metadata.title.trim().is_empty() {
        return Err(MetadataError::TitleEmpty);
    }
    if title_chars > MAX_TITLE_CHARS {
        return Err(MetadataError::TitleTooLong {
            chars: title_chars,
            limit: MAX_TITLE_CHARS,
        });
    }
    let author_chars = metadata.author.chars().count();
    if author_chars > MAX_AUTHOR_CHARS {
        return Err(MetadataError::AuthorTooLong {
            chars: author_chars,
            limit: MAX_AUTHOR_CHARS,
        });
    }
    if !is_calendar_date(&metadata.date) {
        return Err(MetadataError::InvalidDate {
            value: metadata.date.clone(),
        });
    }
    Ok(())
}

fn is_calendar_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }
    let year: i32 = value[0..4].parse().unwrap_or(0);
    let month: u32 = value[5..7].parse().unwrap_or(0);
    let day: u32 = value[8..10].parse().unwrap_or(0);
    if !(1..=12).contains(&month) || day == 0 {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let limit = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    day <= limit
}

/// Escapes plain text for use in LaTeX source.
///
/// One encoder for every insertion point. It handles the ten characters that
/// change LaTeX's meaning — `\ { } % & _ $ # ^ ~` — and leaves everything else,
/// including all Unicode, untouched. `^` and `~` become text commands because a
/// bare one is a superscript or a non-breaking space, not the character the user
/// typed.
pub fn escape(text: &str) -> String {
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

/// One generated file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedFile {
    /// Path relative to the project root.
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PlanIssue {
    /// A file the wizard would write already exists in the destination.
    Collision { path: String },
    /// The metadata did not validate.
    InvalidMetadata { error: MetadataError },
    /// The destination is not a plain vault-relative directory.
    UnsafeDestination { destination: String },
    /// A template variant is missing; the caller must not proceed silently.
    UnknownTemplate {
        #[serde(rename = "templateId")]
        template_id: String,
    },
}

/// The plan a user confirms before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaffoldPlan {
    /// Directory the project would occupy, without a trailing slash.
    pub destination: String,
    pub entry_file: String,
    pub files: Vec<GeneratedFile>,
    /// Empty only when the plan may be executed (F46-R03).
    pub issues: Vec<PlanIssue>,
    /// Fixed resources the build expects; reported, never downloaded.
    pub requires: Vec<String>,
}

impl ScaffoldPlan {
    pub fn is_executable(&self) -> bool {
        self.issues.is_empty() && !self.files.is_empty()
    }
}

fn localized(
    language: DocumentLanguage,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match language {
        // (introduction, body, conclusion, abstract)
        DocumentLanguage::ZhCn => ("引言", "正文", "结论", "摘要"),
        DocumentLanguage::En => ("Introduction", "Body", "Conclusion", "Abstract"),
    }
}

/// The `main.tex` preamble, shared by all three templates.
///
/// 11 pt body and 25 mm margins are the OpenNote defaults from F46-R08; the paper
/// size only changes the paper, never the margins.
fn preamble(metadata: &Metadata) -> String {
    let paper = match metadata.paper {
        PaperSize::A4 => "a4paper",
        PaperSize::Letter => "letterpaper",
    };
    format!(
        "\\documentclass[11pt,{paper}]{{article}}\n\
\\usepackage[margin=25mm]{{geometry}}\n\
\\usepackage[T1]{{fontenc}}\n\
\\usepackage[utf8]{{inputenc}}\n\
\\usepackage{{graphicx}}\n\
\\usepackage{{hyperref}}\n\
\\graphicspath{{{{assets/}}}}\n\n\
\\title{{{title}}}\n\
{author}\
\\date{{{date}}}\n\n\
\\begin{{document}}\n\
\\maketitle\n\n",
        paper = paper,
        title = escape(&metadata.title),
        author = if metadata.author.trim().is_empty() {
            String::new()
        } else {
            format!("\\author{{{}}}\n", escape(&metadata.author))
        },
        date = escape(&metadata.date),
    )
}

fn main_tex(metadata: &Metadata, template: TemplateId) -> String {
    let mut body = preamble(metadata);
    match template {
        TemplateId::Report => {
            let (intro, main, conclusion, _) = localized(metadata.language);
            body.push_str(&format!(
                "\\input{{sections/introduction}}\n\
\\input{{sections/body}}\n\
\\input{{sections/conclusion}}\n\n\
\\bibliographystyle{{plain}}\n\
\\bibliography{{references}}\n\n\
\\end{{document}}\n",
            ));
            let _ = (intro, main, conclusion);
        }
        TemplateId::Paper => {
            body.push_str(
                "\\input{sections/abstract}\n\
\\input{sections/introduction}\n\
\\input{sections/methods}\n\
\\input{sections/results}\n\
\\input{sections/conclusion}\n\n\
\\bibliographystyle{plain}\n\
\\bibliography{references}\n\n\
\\end{document}\n",
            );
        }
        TemplateId::Assignment => {
            body.push_str(
                "\\input{sections/questions}\n\n\
\\bibliographystyle{plain}\n\
\\bibliography{references}\n\n\
\\end{document}\n",
            );
        }
    }
    body
}

fn section(title: &str) -> String {
    format!("\\section{{{}}}\n\n", escape(title))
}

/// Canonical vault-relative spelling used when comparing planned and existing
/// paths.
///
/// `proj\main.tex`, `./proj//main.tex` and `proj/main.tex` name one file, so a
/// collision must not depend on how the caller spelled it (F46-R03). A leading
/// slash and `..` are preserved rather than resolved: they still have to reach
/// the unsafe-destination check instead of being silently flattened.
fn canonical_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let absolute = normalized.starts_with('/');
    let joined = normalized
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Builds the scaffold for one template.
///
/// `existing` lists the paths already present in the destination, so a collision
/// is reported in the plan instead of being discovered after a partial write.
pub fn plan(
    template: TemplateId,
    metadata: &Metadata,
    destination: &str,
    existing: &[String],
) -> ScaffoldPlan {
    let mut issues: Vec<PlanIssue> = Vec::new();
    if let Err(error) = validate(metadata) {
        issues.push(PlanIssue::InvalidMetadata { error });
    }
    let destination = canonical_path(destination.trim());
    if destination.starts_with('/')
        || destination.chars().nth(1) == Some(':')
        || destination.split('/').any(|segment| segment == "..")
    {
        issues.push(PlanIssue::UnsafeDestination {
            destination: destination.clone(),
        });
    }

    let (intro, main_title, conclusion, abstract_title) = localized(metadata.language);
    let mut files: Vec<GeneratedFile> = vec![GeneratedFile {
        path: "main.tex".to_owned(),
        content: main_tex(metadata, template),
    }];

    match template {
        TemplateId::Report => {
            files.push(GeneratedFile {
                path: "sections/introduction.tex".to_owned(),
                content: section(intro),
            });
            files.push(GeneratedFile {
                path: "sections/body.tex".to_owned(),
                content: section(main_title),
            });
            files.push(GeneratedFile {
                path: "sections/conclusion.tex".to_owned(),
                content: section(conclusion),
            });
        }
        TemplateId::Paper => {
            files.push(GeneratedFile {
                path: "sections/abstract.tex".to_owned(),
                content: format!("\\begin{{abstract}}\n\n\\end{{abstract}}\n"),
            });
            let _ = abstract_title;
            for (path, title) in [
                ("sections/introduction.tex", intro),
                ("sections/methods.tex", "Methods"),
                ("sections/results.tex", "Results"),
                ("sections/conclusion.tex", conclusion),
            ] {
                files.push(GeneratedFile {
                    path: path.to_owned(),
                    content: section(title),
                });
            }
        }
        TemplateId::Assignment => {
            // Two labelled example questions, deliberately without answers: the
            // wizard must not invent content the student did not write.
            let questions = match metadata.language {
                DocumentLanguage::ZhCn => {
                    "\\section{题目}\n\n\\subsection*{练习 1}\n\n在此作答。\n\n\\subsection*{练习 2}\n\n在此作答。\n"
                }
                DocumentLanguage::En => {
                    "\\section{Questions}\n\n\\subsection*{Exercise 1}\n\nWrite your answer here.\n\n\\subsection*{Exercise 2}\n\nWrite your answer here.\n"
                }
            };
            files.push(GeneratedFile {
                path: "sections/questions.tex".to_owned(),
                content: questions.to_owned(),
            });
        }
    }

    // An empty bibliography file is valid and keeps `\bibliography` resolvable.
    files.push(GeneratedFile {
        path: "references.bib".to_owned(),
        content: String::new(),
    });
    // The assets directory is represented by a placeholder so it survives a copy
    // that ignores empty directories.
    files.push(GeneratedFile {
        path: "assets/.gitkeep".to_owned(),
        content: String::new(),
    });

    let canonical_existing: Vec<String> =
        existing.iter().map(|path| canonical_path(path)).collect();
    for file in &files {
        let full = if destination.is_empty() {
            file.path.clone()
        } else {
            format!("{destination}/{}", file.path)
        };
        if canonical_existing.iter().any(|path| path == &full) {
            issues.push(PlanIssue::Collision { path: full });
        }
    }

    ScaffoldPlan {
        destination,
        entry_file: "main.tex".to_owned(),
        files,
        issues,
        requires: vec![
            "fontenc:T1".to_owned(),
            "geometry".to_owned(),
            "graphicx".to_owned(),
            "hyperref".to_owned(),
        ],
    }
}

/// The language the wizard should preselect, and whether the user has changed it.
///
/// Switching the interface afterwards must not replace a chosen language, so this
/// only reports a default; the caller keeps whatever the user selected (F46-R07).
pub fn default_language(
    ui_locale: &str,
    already_chosen: Option<DocumentLanguage>,
) -> DocumentLanguage {
    already_chosen.unwrap_or_else(|| DocumentLanguage::default_for_ui(ui_locale))
}

/// Every template id this build can scaffold, for the picker.
pub fn templates() -> [TemplateId; 3] {
    [
        TemplateId::Report,
        TemplateId::Paper,
        TemplateId::Assignment,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Metadata {
        Metadata {
            title: "Weekly review".to_owned(),
            author: "Alice".to_owned(),
            date: "2026-10-20".to_owned(),
            language: DocumentLanguage::En,
            paper: PaperSize::A4,
        }
    }

    #[test]
    fn escaping_covers_every_character_that_changes_latex_meaning() {
        // One encoder, tested in one place, so no template can forget a case.
        let (input, expected) = (
            "\\ { } % & _ $ # ^ ~",
            "\\textbackslash{} \\{ \\} \\% \\& \\_ \\$ \\# \\textasciicircum{} \\textasciitilde{}",
        );
        assert_eq!(escape(input), expected);
    }

    #[test]
    fn escaping_leaves_unicode_and_ordinary_text_alone() {
        assert_eq!(escape("报告：结论"), "报告：结论");
        assert_eq!(escape("plain text 123"), "plain text 123");
        assert_eq!(escape(""), "");
    }

    #[test]
    fn a_title_with_special_characters_cannot_break_the_preamble() {
        let mut input = metadata();
        input.title = "R&D: 50% of $5 _ #1 ^ ~ \\done".to_owned();
        let built = plan(TemplateId::Report, &input, "proj", &[]);
        let main = &built.files[0].content;
        assert!(main.contains("\\title{R\\&D: 50\\% of \\$5 \\_ \\#1 \\textasciicircum{}"));
        // No bare `&` or `%` survived into the document body.
        assert!(!main.contains("R&D"));
    }

    #[test]
    fn an_empty_author_omits_the_author_line_entirely() {
        let mut input = metadata();
        input.author = "   ".to_owned();
        let built = plan(TemplateId::Report, &input, "proj", &[]);
        assert!(!built.files[0].content.contains("\\author"));
    }

    #[test]
    fn the_three_templates_produce_the_documented_structure() {
        let report = plan(TemplateId::Report, &metadata(), "p", &[]);
        let paths: Vec<&str> = report.files.iter().map(|file| file.path.as_str()).collect();
        assert!(paths.contains(&"main.tex"));
        assert!(paths.contains(&"sections/introduction.tex"));
        assert!(paths.contains(&"sections/body.tex"));
        assert!(paths.contains(&"sections/conclusion.tex"));
        assert!(paths.contains(&"references.bib"));
        assert!(paths.contains(&"assets/.gitkeep"));

        let paper = plan(TemplateId::Paper, &metadata(), "p", &[]);
        let paths: Vec<&str> = paper.files.iter().map(|file| file.path.as_str()).collect();
        assert!(paths.contains(&"sections/abstract.tex"));
        assert!(paths.contains(&"sections/methods.tex"));
        assert!(paths.contains(&"sections/results.tex"));

        let assignment = plan(TemplateId::Assignment, &metadata(), "p", &[]);
        let paths: Vec<&str> = assignment
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert!(paths.contains(&"sections/questions.tex"));
        assert!(!paths.contains(&"sections/body.tex"));
    }

    #[test]
    fn main_tex_includes_every_generated_section_by_project_relative_path() {
        let built = plan(TemplateId::Paper, &metadata(), "p", &[]);
        let main = &built.files[0].content;
        for path in [
            "sections/abstract",
            "sections/introduction",
            "sections/methods",
            "sections/results",
            "sections/conclusion",
        ] {
            assert!(
                main.contains(&format!("\\input{{{path}}}")),
                "missing {path}"
            );
        }
        assert!(main.contains("\\bibliography{references}"));
    }

    #[test]
    fn an_empty_bibliography_is_valid_and_the_assignment_invents_no_answers() {
        let built = plan(TemplateId::Assignment, &metadata(), "p", &[]);
        let bib = built
            .files
            .iter()
            .find(|file| file.path == "references.bib")
            .expect("bib");
        assert!(bib.content.is_empty());
        let questions = built
            .files
            .iter()
            .find(|file| file.path == "sections/questions.tex")
            .expect("questions");
        // Two labelled exercises, no answers.
        assert!(questions.content.contains("Exercise 1"));
        assert!(questions.content.contains("Exercise 2"));
        assert!(!questions.content.contains("Answer:"));
    }

    #[test]
    fn the_document_language_is_localized_independently_of_the_ui() {
        let mut chinese = metadata();
        chinese.language = DocumentLanguage::ZhCn;
        let built = plan(TemplateId::Report, &chinese, "p", &[]);
        let intro = built
            .files
            .iter()
            .find(|file| file.path == "sections/introduction.tex")
            .expect("intro");
        assert!(intro.content.contains("引言"));

        // The English variant of the same template uses English headings.
        let english = plan(TemplateId::Report, &metadata(), "p", &[]);
        let intro = english
            .files
            .iter()
            .find(|file| file.path == "sections/introduction.tex")
            .expect("intro");
        assert!(intro.content.contains("Introduction"));
    }

    #[test]
    fn the_default_language_follows_the_ui_only_on_first_open() {
        // F46-R07: zh-CN and zh-TW default to Chinese; everything else to English.
        assert_eq!(
            DocumentLanguage::default_for_ui("zh-CN"),
            DocumentLanguage::ZhCn
        );
        assert_eq!(
            DocumentLanguage::default_for_ui("zh-TW"),
            DocumentLanguage::ZhCn
        );
        assert_eq!(DocumentLanguage::default_for_ui("en"), DocumentLanguage::En);
        assert_eq!(DocumentLanguage::default_for_ui("ja"), DocumentLanguage::En);
        // A language the user already picked is never replaced by the UI.
        assert_eq!(
            default_language("ja", Some(DocumentLanguage::ZhCn)),
            DocumentLanguage::ZhCn
        );
        assert_eq!(default_language("ja", None), DocumentLanguage::En);
    }

    #[test]
    fn the_defaults_are_a4_eleven_point_and_25mm_margins() {
        // F46-R08: the OpenNote defaults, not a claim about any journal.
        let built = plan(TemplateId::Report, &metadata(), "p", &[]);
        let main = &built.files[0].content;
        assert!(main.contains("11pt"));
        assert!(main.contains("a4paper"));
        assert!(main.contains("margin=25mm"));
    }

    #[test]
    fn letter_only_changes_the_paper_not_the_margins() {
        let mut letter = metadata();
        letter.paper = PaperSize::Letter;
        let built = plan(TemplateId::Report, &letter, "p", &[]);
        let main = &built.files[0].content;
        assert!(main.contains("letterpaper"));
        assert!(!main.contains("a4paper"));
        // The margin is unchanged: Letter must not silently alter the layout.
        assert!(main.contains("margin=25mm"));
    }

    #[test]
    fn an_invalid_title_is_refused_before_a_plan_exists() {
        let mut input = metadata();
        input.title = "   ".to_owned();
        let built = plan(TemplateId::Report, &input, "p", &[]);
        assert!(!built.is_executable());
        assert!(built.issues.iter().any(|issue| matches!(
            issue,
            PlanIssue::InvalidMetadata {
                error: MetadataError::TitleEmpty
            }
        )));
    }

    #[test]
    fn an_overlong_title_or_author_is_refused_with_the_limit() {
        let mut input = metadata();
        input.title = "t".repeat(300);
        assert_eq!(
            validate(&input),
            Err(MetadataError::TitleTooLong {
                chars: 300,
                limit: MAX_TITLE_CHARS
            })
        );
        input.title = "ok".to_owned();
        input.author = "a".repeat(200);
        assert_eq!(
            validate(&input),
            Err(MetadataError::AuthorTooLong {
                chars: 200,
                limit: MAX_AUTHOR_CHARS
            })
        );
    }

    #[test]
    fn an_impossible_date_is_refused() {
        let mut input = metadata();
        input.date = "2026-02-30".to_owned();
        assert_eq!(
            validate(&input),
            Err(MetadataError::InvalidDate {
                value: "2026-02-30".to_owned()
            })
        );
        input.date = "tomorrow".to_owned();
        assert!(matches!(
            validate(&input),
            Err(MetadataError::InvalidDate { .. })
        ));
    }

    #[test]
    fn a_collision_is_reported_in_the_plan_instead_of_overwriting() {
        // F46-R03: preview collisions, never overwrite.
        let built = plan(
            TemplateId::Report,
            &metadata(),
            "proj",
            &["proj/main.tex".to_owned()],
        );
        assert!(!built.is_executable());
        assert!(built.issues.iter().any(|issue| matches!(
            issue,
            PlanIssue::Collision { path } if path == "proj/main.tex"
        )));
    }

    #[test]
    fn a_collision_is_detected_however_the_same_path_is_spelled() {
        // F46-R03: a Windows separator, a `./` prefix or a doubled slash must not
        // hide a file that the wizard would overwrite.
        let built = plan(
            TemplateId::Report,
            &metadata(),
            "./proj//",
            &[
                "proj\\main.tex".to_owned(),
                "proj//sections/./introduction.tex".to_owned(),
            ],
        );
        assert_eq!(built.destination, "proj");
        assert!(!built.is_executable());
        for expected in ["proj/main.tex", "proj/sections/introduction.tex"] {
            assert!(
                built.issues.iter().any(
                    |issue| matches!(issue, PlanIssue::Collision { path } if path == expected)
                ),
                "{expected} should be reported as a collision"
            );
        }
    }

    #[test]
    fn a_dot_destination_is_the_vault_root_and_collisions_there_still_count() {
        let built = plan(
            TemplateId::Report,
            &metadata(),
            ".",
            &["main.tex".to_owned()],
        );
        assert_eq!(built.destination, "");
        assert!(built
            .issues
            .iter()
            .any(|issue| matches!(issue, PlanIssue::Collision { path } if path == "main.tex")));
    }

    #[test]
    fn an_unsafe_destination_is_refused() {
        for destination in [
            "../outside",
            "./../outside",
            "proj/./../outside",
            "/absolute",
            "C:\\vault",
            "C:relative",
            "\\\\server\\share",
        ] {
            let built = plan(TemplateId::Report, &metadata(), destination, &[]);
            assert!(
                built
                    .issues
                    .iter()
                    .any(|issue| matches!(issue, PlanIssue::UnsafeDestination { .. })),
                "{destination} should be refused"
            );
        }
    }

    #[test]
    fn a_clean_plan_is_executable_and_lists_the_resources_it_needs() {
        let built = plan(TemplateId::Report, &metadata(), "proj", &[]);
        assert!(built.is_executable());
        assert_eq!(built.entry_file, "main.tex");
        assert!(built.requires.contains(&"geometry".to_owned()));
        // The wizard never downloads or installs anything.
        assert!(built.requires.iter().all(|item| !item.contains("http")));
    }

    #[test]
    fn the_source_files_carry_no_yaml_frontmatter() {
        // F45-R01: an external tool must still be able to open these files.
        let built = plan(TemplateId::Paper, &metadata(), "p", &[]);
        for file in &built.files {
            assert!(
                !file.content.starts_with("---"),
                "{} must not start with YAML",
                file.path
            );
        }
    }

    #[test]
    fn template_ids_round_trip_and_an_unknown_one_is_rejected() {
        for template in templates() {
            assert_eq!(TemplateId::parse(template.as_str()), Some(template));
        }
        assert_eq!(TemplateId::parse("poster"), None);
        assert_eq!(templates().len(), 3);
    }
}
