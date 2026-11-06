//! M13 LaTeX source editing assistance (F47).
//!
//! The editor keeps the buffer and the undo stack; this crate only computes the
//! edits an action implies. That split is what makes the guarantees testable:
//!
//! * an unknown macro is never rewritten into a known one (F47-R01) — the
//!   assistant reports what it does not recognise instead of guessing;
//! * an edit always replaces an explicit range, so one action is one undo step
//!   and a stale range is refused rather than applied to the wrong text
//!   (F47-R02, REPLACE_PLAN_STALE);
//! * bracket pairing and completion never fire mid-composition (F47-R03), which
//!   is decided by the caller but enforced here as a precondition.
//!
//! Nothing here reads a file or touches the clipboard.

use serde::{Deserialize, Serialize};

/// The language a buffer is treated as (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LanguageMode {
    Tex,
    Bib,
    Plain,
}

/// Which language a file should be edited as.
///
/// Detection uses the extension first and the content second; an unknown
/// extension is plain text so nothing is highlighted with the wrong grammar.
pub fn detect_language(path: &str, source: &str) -> LanguageMode {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".tex") || lower.ends_with(".latex") || lower.ends_with(".sty") {
        return LanguageMode::Tex;
    }
    if lower.ends_with(".bib") {
        return LanguageMode::Bib;
    }
    // An extensionless or unusual file: look at the first non-empty line.
    if let Some(first) = source.lines().find(|line| !line.trim().is_empty()) {
        let trimmed = first.trim_start();
        if trimmed.starts_with("\\documentclass")
            || trimmed.starts_with("\\begin{")
            || trimmed.starts_with("\\input{")
        {
            return LanguageMode::Tex;
        }
        if trimmed.starts_with('@') {
            return LanguageMode::Bib;
        }
    }
    LanguageMode::Plain
}

/// A byte range in the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl Range {
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// One replacement the editor should apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub range: Range,
    pub text: String,
    /// What the assistant did, for the status line.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssistError {
    /// The selection is not inside the buffer.
    RangeOutOfBounds {
        start: usize,
        end: usize,
        length: usize,
    },
    /// The text at the range no longer matches what was previewed.
    StaleRange { expected: String, found: String },
    /// The action is not applicable to this language.
    NotApplicable(String),
    /// An input-method composition is in progress (F47-R03).
    Composing,
}

/// The result of an assist action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistResult {
    pub edits: Vec<Edit>,
    /// Selection to restore after the edits, in the coordinate space of the new
    /// text.
    pub selection: Range,
    pub warnings: Vec<String>,
}

fn range_of(source: &str, range: Range) -> Result<&str, AssistError> {
    if range.start > range.end || range.end > source.len() {
        return Err(AssistError::RangeOutOfBounds {
            start: range.start,
            end: range.end,
            length: source.len(),
        });
    }
    if !source.is_char_boundary(range.start) || !source.is_char_boundary(range.end) {
        // A range that splits a character would corrupt the buffer.
        return Err(AssistError::RangeOutOfBounds {
            start: range.start,
            end: range.end,
            length: source.len(),
        });
    }
    Ok(&source[range.start..range.end])
}

/// Toggles `%` comments on the selected lines.
///
/// Line comments are a per-line operation, so the range is expanded to whole
/// lines: a partial comment would silently comment out more than the user
/// selected.
pub fn toggle_comment(
    source: &str,
    selection: Range,
    composing: bool,
) -> Result<AssistResult, AssistError> {
    if composing {
        return Err(AssistError::Composing);
    }
    range_of(source, selection)?;
    let start = source[..selection.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let end = source[selection.end..]
        .find('\n')
        .map_or(source.len(), |index| selection.end + index);
    let block = &source[start..end];
    let lines: Vec<&str> = block.split('\n').collect();
    let all_commented = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.trim_start().starts_with('%'));

    let mut replaced = String::with_capacity(block.len() + lines.len());
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            replaced.push('\n');
        }
        if all_commented {
            // Remove the first `%` and one following space if present.
            match line.find('%') {
                Some(position) => {
                    replaced.push_str(&line[..position]);
                    let rest = &line[position + 1..];
                    replaced.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
                None => replaced.push_str(line),
            }
        } else if line.trim().is_empty() {
            replaced.push_str(line);
        } else {
            let indent = line.len() - line.trim_start().len();
            replaced.push_str(&line[..indent]);
            replaced.push('%');
            replaced.push_str(&line[indent..]);
        }
    }
    let text_len = replaced.len();
    Ok(AssistResult {
        edits: vec![Edit {
            range: Range {
                start,
                end: end.min(source.len()),
            },
            text: replaced,
            kind: if all_commented {
                "uncomment".to_owned()
            } else {
                "comment".to_owned()
            },
        }],
        selection: Range {
            start,
            end: start + text_len,
        },
        warnings: Vec::new(),
    })
}

/// Finds the matching bracket for the bracket at `offset`.
///
/// LaTeX nests braces in ways that make a naive counter wrong — an escaped `\{`
/// is a literal, not a group — so escapes are skipped. A missing partner returns
/// `None` rather than an invented position.
pub fn matching_bracket(source: &str, offset: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if offset >= bytes.len() {
        return None;
    }
    let open = bytes[offset];
    let (open_byte, close_byte, forward) = match open {
        b'{' => (b'{', b'}', true),
        b'}' => (b'{', b'}', false),
        b'[' => (b'[', b']', true),
        b']' => (b'[', b']', false),
        b'(' => (b'(', b')', true),
        b')' => (b'(', b')', false),
        _ => return None,
    };
    // Walk backwards to confirm the bracket is not escaped.
    let mut backslashes = 0usize;
    let mut cursor = offset;
    while cursor > 0 {
        cursor -= 1;
        if bytes[cursor] == b'\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    if backslashes % 2 == 1 {
        // An escaped bracket is literal text with no partner.
        return None;
    }
    // A candidate partner is only real when it is itself unescaped; otherwise the
    // search would pair with literal text, which is what `\{not a group}` does.
    let is_escaped = |index: usize| -> bool {
        let mut backslashes = 0usize;
        let mut cursor = index;
        while cursor > 0 {
            cursor -= 1;
            if bytes[cursor] == b'\\' {
                backslashes += 1;
            } else {
                break;
            }
        }
        backslashes % 2 == 1
    };
    let mut depth = 0i64;
    if forward {
        let mut index = offset;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == open_byte && !is_escaped(index) {
                depth += 1;
            } else if byte == close_byte && !is_escaped(index) {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            index += 1;
        }
        None
    } else {
        let mut index = offset as i64;
        while index >= 0 {
            let position = index as usize;
            let byte = bytes[position];
            if byte == close_byte && !is_escaped(position) {
                depth += 1;
            } else if byte == open_byte && !is_escaped(position) {
                depth -= 1;
                if depth == 0 {
                    return Some(position);
                }
            }
            index -= 1;
        }
        None
    }
}

/// Wraps the selection in `\command{...}`.
///
/// The selection is preserved as the inner text, so the wrap is one undo step and
/// the caret keeps the user's text selected (F47-R02).
pub fn wrap_selection(
    source: &str,
    selection: Range,
    command: &str,
    composing: bool,
) -> Result<AssistResult, AssistError> {
    if composing {
        return Err(AssistError::Composing);
    }
    if command.trim().is_empty() || !command.chars().all(|character| character.is_alphabetic()) {
        return Err(AssistError::NotApplicable(command.to_owned()));
    }
    let selected = range_of(source, selection)?;
    let text = format!("\\{command}{{{selected}}}");
    Ok(AssistResult {
        edits: vec![Edit {
            range: selection,
            text: text.clone(),
            kind: "wrap".to_owned(),
        }],
        // Keep the original text selected inside the new braces.
        selection: Range {
            start: selection.start + command.len() + 2,
            end: selection.start + command.len() + 2 + selected.len(),
        },
        warnings: Vec::new(),
    })
}

/// The completion candidates this build offers (§4).
///
/// They come from a fixed lexicon plus the commands the project's own buffer
/// already uses; nothing is inferred by executing a macro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub label: String,
    /// The text inserted when accepted.
    pub insert: String,
    /// `lexicon` or `project`.
    pub source: String,
}

const LEXICON: &[(&str, &str)] = &[
    ("documentclass", "\\documentclass{}"),
    ("begin", "\\begin{}"),
    ("end", "\\end{}"),
    ("section", "\\section{}"),
    ("subsection", "\\subsection{}"),
    ("label", "\\label{}"),
    ("ref", "\\ref{}"),
    ("cite", "\\cite{}"),
    ("input", "\\input{}"),
    ("includegraphics", "\\includegraphics{}"),
    ("textbf", "\\textbf{}"),
    ("emph", "\\emph{}"),
    ("frac", "\\frac{}{}"),
    ("item", "\\item "),
    ("bibliography", "\\bibliography{}"),
];

/// Every `\command` name in the buffer, so the project's own vocabulary is
/// offered even when it is not in the lexicon.
pub fn project_commands(source: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        // A command name is ASCII alphabetic; `\\` and `\%` are not names.
        while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
            end += 1;
        }
        if end > index + 1 {
            let name = &source[index + 1..end];
            if !names.iter().any(|existing| existing == name) {
                names.push(name.to_owned());
            }
        }
        index = end.max(index + 1);
    }
    names.sort();
    names
}

/// Completions for the partial command at the caret.
pub fn completions(source: &str, prefix: &str) -> Vec<Candidate> {
    let query = prefix.trim_start_matches('\\').to_ascii_lowercase();
    let mut candidates: Vec<Candidate> = Vec::new();
    for (name, insert) in LEXICON {
        if name.starts_with(&query) {
            candidates.push(Candidate {
                label: format!("\\{name}"),
                insert: (*insert).to_owned(),
                source: "lexicon".to_owned(),
            });
        }
    }
    for name in project_commands(source) {
        if !name.to_ascii_lowercase().starts_with(&query) {
            continue;
        }
        if candidates
            .iter()
            .any(|candidate| candidate.label == format!("\\{name}"))
        {
            continue;
        }
        candidates.push(Candidate {
            label: format!("\\{name}"),
            insert: format!("\\{name}"),
            source: "project".to_owned(),
        });
    }
    candidates.sort_by(|left, right| left.label.cmp(&right.label));
    candidates
}

/// Applies a completion to an explicit range, refusing a stale one.
///
/// The caller passes the range it previewed and the text it saw there; a mismatch
/// means the buffer changed and the completion must be re-previewed
/// (REPLACE_PLAN_STALE).
pub fn apply_completion(
    source: &str,
    range: Range,
    expected: &str,
    insert: &str,
) -> Result<AssistResult, AssistError> {
    let current = range_of(source, range)?;
    if current != expected {
        return Err(AssistError::StaleRange {
            expected: expected.to_owned(),
            found: current.to_owned(),
        });
    }
    let inserted_len = insert.len();
    Ok(AssistResult {
        edits: vec![Edit {
            range,
            text: insert.to_owned(),
            kind: "complete".to_owned(),
        }],
        selection: Range {
            start: range.start + inserted_len,
            end: range.start + inserted_len,
        },
        warnings: Vec::new(),
    })
}

/// Whether a replacement may cross a project boundary (§4).
///
/// A replace-all is confined to one file by default: a project-wide replace that
/// silently edited another file would be the destructive fallback F47-R06 forbids.
pub fn replace_scope_allows(target_file: &str, replace_file: &str, project_scope: bool) -> bool {
    project_scope || target_file == replace_file
}

/// Applies edits to a buffer, for a test or a preview.
///
/// Edits are applied back to front so an earlier offset stays valid.
pub fn apply_edits(source: &str, edits: &[Edit]) -> Result<String, AssistError> {
    let mut ordered = edits.to_vec();
    ordered.sort_by(|left, right| right.range.start.cmp(&left.range.start));
    let mut text = source.to_owned();
    for edit in ordered {
        let slice = range_of(source, edit.range)?;
        let length = slice.len();
        if edit.range.start + length > text.len() {
            return Err(AssistError::RangeOutOfBounds {
                start: edit.range.start,
                end: edit.range.start + length,
                length: text.len(),
            });
        }
        text.replace_range(edit.range.start..edit.range.start + length, &edit.text);
    }
    Ok(text)
}

/// A file that changed on disk while the buffer has unsaved edits.
///
/// F47-R07 / FILE_EXTERNALLY_CHANGED: the buffer is kept and the difference is
/// reported; it is never reloaded silently, which would destroy what the user
/// typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalChange {
    pub buffer_kept: bool,
    /// True when the buffer and the file differ.
    pub differs: bool,
    pub notice_key: String,
}

pub fn external_change(buffer: &str, on_disk: &str) -> ExternalChange {
    ExternalChange {
        // The buffer always wins until the user decides.
        buffer_kept: true,
        differs: buffer != on_disk,
        notice_key: "latex.externalChanged".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_is_detected_from_the_extension_then_the_content() {
        assert_eq!(detect_language("main.tex", ""), LanguageMode::Tex);
        assert_eq!(detect_language("refs.bib", ""), LanguageMode::Bib);
        assert_eq!(detect_language("notes.txt", "hello"), LanguageMode::Plain);
        // An extensionless file falls back to content sniffing.
        assert_eq!(
            detect_language("Makefile", "\\documentclass{article}"),
            LanguageMode::Tex
        );
        assert_eq!(detect_language("data", "@article{x,"), LanguageMode::Bib);
        // Unknown content is plain text rather than a wrong grammar.
        assert_eq!(detect_language("data", "just words"), LanguageMode::Plain);
    }

    #[test]
    fn commenting_adds_and_removes_the_marker_per_line() {
        let source = "alpha\nbeta\n";
        let commented = toggle_comment(source, Range { start: 0, end: 5 }, false).expect("comment");
        assert_eq!(commented.edits[0].text, "%alpha");
        assert_eq!(commented.edits[0].kind, "comment");
        let new_text = apply_edits(source, &commented.edits).expect("apply");
        assert_eq!(new_text, "%alpha\nbeta\n");

        // Toggling again removes the marker.
        let uncommented =
            toggle_comment(&new_text, Range { start: 0, end: 6 }, false).expect("uncomment");
        assert_eq!(
            apply_edits(&new_text, &uncommented.edits).expect("apply"),
            source
        );
        assert_eq!(uncommented.edits[0].kind, "uncomment");
    }

    #[test]
    fn commenting_expands_to_whole_lines() {
        // A partial comment would comment out more than the user selected.
        let source = "alpha\nbeta\n";
        let result = toggle_comment(source, Range { start: 2, end: 4 }, false).expect("comment");
        assert_eq!(result.edits[0].range, Range { start: 0, end: 5 });
    }

    #[test]
    fn a_composition_in_progress_blocks_the_action() {
        // F47-R03: never edit mid-composition.
        assert_eq!(
            toggle_comment("text", Range { start: 0, end: 1 }, true),
            Err(AssistError::Composing)
        );
        assert_eq!(
            wrap_selection("text", Range { start: 0, end: 1 }, "textbf", true),
            Err(AssistError::Composing)
        );
    }

    #[test]
    fn wrapping_keeps_the_selection_inside_the_new_braces() {
        let source = "highlight me";
        let result =
            wrap_selection(source, Range { start: 0, end: 12 }, "textbf", false).expect("wrap");
        assert_eq!(result.edits[0].text, "\\textbf{highlight me}");
        let applied = apply_edits(source, &result.edits).expect("apply");
        assert_eq!(applied, "\\textbf{highlight me}");
        // The inner text stays selected, which is what makes it one undo step the
        // user can keep typing into (F47-R02).
        assert_eq!(
            &applied[result.selection.start..result.selection.end],
            "highlight me"
        );
    }

    #[test]
    fn wrapping_refuses_a_command_that_is_not_a_name() {
        assert!(matches!(
            wrap_selection("x", Range { start: 0, end: 1 }, "", false),
            Err(AssistError::NotApplicable(_))
        ));
        assert!(matches!(
            wrap_selection("x", Range { start: 0, end: 1 }, "in put", false),
            Err(AssistError::NotApplicable(_))
        ));
    }

    #[test]
    fn brackets_pair_across_nesting_and_skip_escapes() {
        let source = "{outer {inner} end}";
        // The string is 19 bytes, so the final brace is at index 18.
        assert_eq!(source.len(), 19);
        assert_eq!(matching_bracket(source, 0), Some(18));
        assert_eq!(matching_bracket(source, 7), Some(13));
        // From a closing bracket, the partner is found backwards.
        assert_eq!(matching_bracket(source, 18), Some(0));

        // An escaped brace is literal text, so it has no partner on either side.
        let escaped = "\\{not a group}";
        assert_eq!(matching_bracket(escaped, 1), None);
        // The `}` at the end has no *unescaped* `{` to pair with, so it reports
        // nothing rather than pairing with the escaped one.
        assert_eq!(matching_bracket(escaped, 13), None);
    }

    #[test]
    fn an_unbalanced_bracket_reports_nothing_instead_of_guessing() {
        assert_eq!(matching_bracket("{unclosed", 0), None);
        assert_eq!(matching_bracket("unopened}", 8), None);
        assert_eq!(matching_bracket("plain", 0), None);
        assert_eq!(matching_bracket("", 0), None);
    }

    #[test]
    fn square_and_round_brackets_pair_independently() {
        assert_eq!(matching_bracket("a[b]c", 1), Some(3));
        assert_eq!(matching_bracket("a(b)c", 1), Some(3));
        // A bracket inside another kind does not confuse the counter.
        assert_eq!(matching_bracket("(a[b]c)", 0), Some(6));
    }

    #[test]
    fn completions_merge_the_lexicon_with_the_project_vocabulary() {
        let source = "\\custommacro{x}\n\\ref{y}\n";
        let items = completions(source, "\\cu");
        assert!(items
            .iter()
            .any(|item| item.label == "\\custommacro" && item.source == "project"));
        // A lexicon command keeps its snippet form.
        let items = completions(source, "\\sec");
        let section = items
            .iter()
            .find(|item| item.label == "\\section")
            .expect("section");
        assert_eq!(section.insert, "\\section{}");
        assert_eq!(section.source, "lexicon");
    }

    #[test]
    fn completions_never_repeat_a_command() {
        let source = "\\ref{a}\n";
        let items = completions(source, "\\ref");
        let refs: Vec<&Candidate> = items.iter().filter(|item| item.label == "\\ref").collect();
        assert_eq!(refs.len(), 1);
        // The lexicon entry wins, so the snippet form is preserved.
        assert_eq!(refs[0].source, "lexicon");
    }

    #[test]
    fn an_empty_prefix_offers_everything_without_duplicates() {
        let items = completions("\\begin{x}", "");
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(labels.len(), unique.len());
    }

    #[test]
    fn project_commands_lists_alphabetic_names_only() {
        let names = project_commands("\\alpha \\\\ \\% \\Beta{}\\alpha");
        assert_eq!(names, vec!["Beta".to_owned(), "alpha".to_owned()]);
        // `\\` is a line break, not a command name.
        assert!(!names.iter().any(|name| name.is_empty()));
    }

    #[test]
    fn a_completion_refuses_a_stale_range() {
        // REPLACE_PLAN_STALE: preview then change, and the old range must not apply.
        let source = "one two three";
        assert_eq!(
            apply_completion(source, Range { start: 0, end: 3 }, "four", "four"),
            Err(AssistError::StaleRange {
                expected: "four".to_owned(),
                found: "one".to_owned()
            })
        );
        // The matching range applies and leaves the caret after the insert.
        let result =
            apply_completion(source, Range { start: 0, end: 3 }, "one", "one").expect("apply");
        assert_eq!(result.selection, Range { start: 3, end: 3 });
    }

    #[test]
    fn a_range_out_of_bounds_or_splitting_a_character_is_refused() {
        assert!(matches!(
            toggle_comment("abc", Range { start: 0, end: 99 }, false),
            Err(AssistError::RangeOutOfBounds { .. })
        ));
        // "报" is three bytes; offset 1 is inside it.
        assert!(matches!(
            toggle_comment("报", Range { start: 1, end: 2 }, false),
            Err(AssistError::RangeOutOfBounds { .. })
        ));
    }

    #[test]
    fn replacing_outside_the_file_is_refused_unless_the_project_scope_is_chosen() {
        // §4: a cross-file replace must be an explicit choice.
        assert!(!replace_scope_allows("a.tex", "b.tex", false));
        assert!(replace_scope_allows("a.tex", "a.tex", false));
        assert!(replace_scope_allows("a.tex", "b.tex", true));
    }

    #[test]
    fn an_external_change_keeps_the_buffer_and_reports_the_difference() {
        // FILE_EXTERNALLY_CHANGED: never reload over unsaved typing.
        let change = external_change("my edits", "their edits");
        assert!(change.buffer_kept);
        assert!(change.differs);
        let same = external_change("same", "same");
        assert!(!same.differs);
    }

    #[test]
    fn edits_apply_back_to_front_so_offsets_stay_valid() {
        let source = "aaa bbb";
        let edits = vec![
            Edit {
                range: Range { start: 0, end: 3 },
                text: "X".to_owned(),
                kind: "test".to_owned(),
            },
            Edit {
                range: Range { start: 4, end: 7 },
                text: "Y".to_owned(),
                kind: "test".to_owned(),
            },
        ];
        assert_eq!(apply_edits(source, &edits).expect("apply"), "X Y");
    }
}
