//! M06 template expansion (F18).
//!
//! A template is an ordinary Markdown file; this crate only expands the small
//! set of whitelisted variables, never an expression. Two rules shape it:
//!
//! * an unknown `{{something}}` is kept verbatim and reported as a warning, so a
//!   typo cannot silently erase the author's text (F18-R01);
//! * dates come from the caller's chosen calendar day, never from the clock, so
//!   a template applied at 23:59 cannot land on the wrong day (F18-R02).
//!
//! Nothing here reads or writes a file, and expanding never mutates the template.

use serde::{Deserialize, Serialize};

/// Variables this build understands. Anything else is preserved.
pub const KNOWN_VARIABLES: &[&str] = &["title", "date", "time", "cursor"];

/// The `{{cursor}}` placeholder, replaced by the caller with a caret position.
pub const CURSOR_MARKER: &str = "{{cursor}}";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateVariables {
    /// Note title, usually the destination file stem.
    pub title: String,
    /// Calendar day, `YYYY-MM-DD`, chosen by the user or the target note.
    pub date: String,
    /// Clock time `HH:MM`, or None when the caller has no meaningful time.
    pub time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpansionWarning {
    /// `{{name}}` is not a whitelisted variable; the text is kept.
    UnknownVariable(String),
    /// `{{` without a closing `}}`; the text is kept.
    Unterminated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Expansion {
    pub fragment: String,
    pub warnings: Vec<ExpansionWarning>,
    /// Byte offset of the caret in `fragment`, or None when the template has no
    /// `{{cursor}}` marker.
    pub cursor_offset: Option<usize>,
}

fn valid_date(value: &str) -> bool {
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

fn valid_time(value: &str) -> bool {
    let mut parts = value.split(':');
    let (Some(hours), Some(minutes), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    hours.len() == 2
        && minutes.len() == 2
        && hours.parse::<u32>().is_ok_and(|hours| hours < 24)
        && minutes.parse::<u32>().is_ok_and(|minutes| minutes < 60)
}

/// Expands a template body.
///
/// `variables.time` is used for `{{time}}`; when absent that variable is left as
/// written and warned about, because inventing a time would be a lie about when
/// the note was created.
pub fn expand(body: &str, variables: &TemplateVariables) -> Expansion {
    let mut fragment = String::with_capacity(body.len());
    let mut warnings: Vec<ExpansionWarning> = Vec::new();
    let mut cursor_offset: Option<usize> = None;
    let mut rest = body;

    while let Some(start) = rest.find("{{") {
        fragment.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // No closing marker: keep the text and report it once.
            if !warnings.contains(&ExpansionWarning::Unterminated) {
                warnings.push(ExpansionWarning::Unterminated);
            }
            fragment.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let name = after[..end].trim();
        let original = &rest[start..start + 2 + end + 2];
        let replacement = match name {
            "title" => Some(variables.title.clone()),
            "date" => {
                if valid_date(&variables.date) {
                    Some(variables.date.clone())
                } else {
                    // An invalid date is kept as the marker text rather than
                    // producing a broken note; the caller sees the warning.
                    warnings.push(ExpansionWarning::UnknownVariable(name.to_owned()));
                    None
                }
            }
            "time" => match variables.time.as_deref() {
                Some(value) if valid_time(value) => Some(value.to_owned()),
                _ => {
                    warnings.push(ExpansionWarning::UnknownVariable(name.to_owned()));
                    None
                }
            },
            "cursor" => {
                cursor_offset = Some(fragment.len());
                Some(String::new())
            }
            other => {
                // F18-R01: unknown variables stay exactly as written.
                warnings.push(ExpansionWarning::UnknownVariable(other.to_owned()));
                None
            }
        };
        fragment.push_str(replacement.as_deref().unwrap_or(original));
        rest = &after[end + 2..];
    }
    fragment.push_str(rest);

    Expansion {
        fragment,
        warnings,
        cursor_offset,
    }
}

/// A template file's own frontmatter flag, so a note can be marked as one.
pub fn is_template(source: &str) -> bool {
    let scanner = onote_metadata_links::Scanner::new(source);
    let Some(frontmatter) = scanner.frontmatter_body() else {
        return false;
    };
    frontmatter.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.eq_ignore_ascii_case("template: true")
            || trimmed.eq_ignore_ascii_case("template: yes")
    })
}

/// A safe default file name for a new note from a title.
pub fn file_name_for(title: &str) -> String {
    let mut name = String::with_capacity(title.len());
    for character in title.trim().chars() {
        if matches!(
            character,
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
        ) {
            name.push('-');
        } else if character == '\0' {
            continue;
        } else {
            name.push(character);
        }
    }
    let name = name.trim().trim_end_matches('.').trim();
    if name.is_empty() {
        "Untitled".to_owned()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> TemplateVariables {
        TemplateVariables {
            title: "Weekly review".to_owned(),
            date: "2026-10-20".to_owned(),
            time: Some("09:30".to_owned()),
        }
    }

    #[test]
    fn expands_the_whitelisted_variables() {
        let expansion = expand("# {{title}}\n\n{{date}} at {{time}}\n", &vars());
        assert_eq!(
            expansion.fragment,
            "# Weekly review\n\n2026-10-20 at 09:30\n"
        );
        assert!(expansion.warnings.is_empty());
    }

    #[test]
    fn an_unknown_variable_is_kept_and_reported() {
        // F18-R01: never erase what the author wrote.
        let expansion = expand("{{title}} {{mood}}\n", &vars());
        assert_eq!(expansion.fragment, "Weekly review {{mood}}\n");
        assert_eq!(
            expansion.warnings,
            vec![ExpansionWarning::UnknownVariable("mood".to_owned())]
        );
    }

    #[test]
    fn an_unterminated_marker_is_kept_verbatim() {
        let expansion = expand("start {{title}} then {{oops", &vars());
        assert!(expansion.fragment.ends_with("{{oops"));
        assert!(expansion.warnings.contains(&ExpansionWarning::Unterminated));
    }

    #[test]
    fn the_cursor_marker_becomes_an_offset() {
        let expansion = expand("before {{cursor}} after", &vars());
        assert_eq!(expansion.fragment, "before  after");
        assert_eq!(expansion.cursor_offset, Some(7));
    }

    #[test]
    fn a_missing_time_is_reported_rather_than_invented() {
        let variables = TemplateVariables {
            title: "T".to_owned(),
            date: "2026-10-20".to_owned(),
            time: None,
        };
        let expansion = expand("{{time}}", &variables);
        assert_eq!(expansion.fragment, "{{time}}");
        assert!(expansion
            .warnings
            .contains(&ExpansionWarning::UnknownVariable("time".to_owned())));
    }

    #[test]
    fn an_impossible_date_is_not_written_into_the_note() {
        let variables = TemplateVariables {
            title: "T".to_owned(),
            date: "2026-02-30".to_owned(),
            time: None,
        };
        let expansion = expand("{{date}}", &variables);
        assert_eq!(expansion.fragment, "{{date}}");
        assert!(!expansion.fragment.contains("2026-02-30"));
    }

    #[test]
    fn a_template_that_only_uses_the_title_is_not_flagged() {
        // A template does not have to use every variable; warning about that
        // would train users to ignore the warnings that matter.
        let expansion = expand("# {{title}}", &vars());
        assert!(expansion.warnings.is_empty());
        assert_eq!(expansion.fragment, "# Weekly review");
    }

    #[test]
    fn expansion_does_not_mutate_or_echo_the_template_metadata() {
        let body = "---\ntemplate: true\n---\n# {{title}}\n";
        let expansion = expand(body, &vars());
        // The body passed in is expanded verbatim; stripping frontmatter is the
        // caller's job, so this proves no hidden rewriting happens.
        assert!(expansion.fragment.contains("template: true"));
        assert!(expansion.fragment.contains("# Weekly review"));
    }

    #[test]
    fn detects_a_template_marker_in_frontmatter_only() {
        assert!(is_template("---\ntemplate: true\n---\nbody\n"));
        assert!(is_template("---\nTemplate: Yes\n---\n"));
        assert!(!is_template("---\ntemplate: false\n---\n"));
        assert!(!is_template("template: true\n"));
        assert!(!is_template("body only"));
    }

    #[test]
    fn file_names_lose_path_separators_and_never_become_empty() {
        assert_eq!(file_name_for("Weekly review"), "Weekly review");
        assert_eq!(file_name_for("a/b:c*d?e"), "a-b-c-d-e");
        assert_eq!(file_name_for("  trailing.  "), "trailing");
        assert_eq!(file_name_for("   "), "Untitled");
        assert_eq!(file_name_for("京都"), "京都");
    }

    #[test]
    fn an_empty_template_expands_to_nothing_with_no_cursor() {
        let expansion = expand("", &vars());
        assert_eq!(expansion.fragment, "");
        assert_eq!(expansion.cursor_offset, None);
    }
}
