//! F09 targeted frontmatter property writes.
//!
//! The rule that drives this module: change exactly one top-level key and keep
//! every other byte of the file. A general YAML round-trip would reorder keys,
//! drop comments and rewrite unknown fields, which F09-A02 explicitly forbids.
//! So this module edits the frontmatter block line by line:
//!
//! * only a plain `key: value` line at column 0 is replaced;
//! * a nested/block value (a key whose following lines are indented) is
//!   replaced as a whole block when the new value is a scalar;
//! * an absent key is appended just before the closing `---`;
//! * anything this build cannot safely edit (flow mapping, duplicate key,
//!   multi-line value being replaced by another block) is refused with a typed
//!   error so the caller can fall back to source editing (F09-R03).

use crate::properties::{PropertyType, PropertyValue};
use crate::scan::{Frontmatter, Scanner};

/// F09 §4: a property name is 1–64 characters.
pub const MAX_PROPERTY_NAME_CHARS: usize = 64;

/// The reserved identity key. It is not editable as an ordinary property.
pub const RESERVED_ID_KEY: &str = "opennote-id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyWriteError {
    /// The name is empty, too long, or reserved.
    InvalidName,
    /// The source has an unterminated frontmatter block.
    Unterminated,
    /// Frontmatter that does not parse as a mapping; the caller must offer
    /// source repair instead of a structured edit (F09-R03).
    InvalidYaml,
    /// A shape this editor refuses to rewrite (flow mapping, duplicate key).
    Unsupported(String),
}

/// Result of a targeted write: the new whole source plus the value actually
/// written. `previous` is returned so the caller can show a diff or detect that
/// nothing changed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyWrite {
    pub source: String,
    pub previous: PropertyValue,
    pub changed: bool,
}

fn eol_of(source: &str) -> &'static str {
    // Keep a file's existing convention; a mixed file uses LF for the one
    // inserted line rather than converting the rest.
    if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn indent_width(line: &str) -> usize {
    line.chars().take_while(|value| *value == ' ').count()
}

/// A top-level key line: no indentation, `key:`, and not a `#` comment.
fn key_of(line: &str) -> Option<&str> {
    if indent_width(line) != 0 || line.starts_with('#') || line.starts_with('\t') {
        return None;
    }
    let (key, _) = line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.starts_with('{') || key.starts_with('[') {
        return None;
    }
    Some(key)
}

fn needs_quoting(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    // Values that YAML would re-type or misread: booleans, numbers, dates,
    // leading indicators, `#` comments and surrounding whitespace.
    let lower = text.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
    ) {
        return true;
    }
    if text.parse::<f64>().is_ok() {
        return true;
    }
    if text != text.trim() {
        return true;
    }
    let first = text.chars().next().unwrap_or(' ');
    if matches!(
        first,
        '-' | '?'
            | ':'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
    ) {
        return true;
    }
    if text.contains(": ") || text.contains(" #") || text.contains('\n') {
        return true;
    }
    // A date-looking plain scalar is still valid YAML as a string, but reading
    // it back would type it as a date. Quote it so the written type wins.
    let bytes = text.as_bytes();
    if bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-' {
        return true;
    }
    false
}

fn quote(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Serializes one property value as a single YAML scalar/flow value.
pub fn render_value(value: &PropertyValue) -> String {
    match value {
        PropertyValue::Text(text) | PropertyValue::Select(text) | PropertyValue::Link(text) => {
            if needs_quoting(text) {
                quote(text)
            } else {
                text.clone()
            }
        }
        PropertyValue::Date(text) | PropertyValue::DateTime(text) => {
            // A date is stored unquoted so a human can read it and so F09-R02's
            // calendar-day rule is visible in the file; a datetime is quoted
            // because YAML would parse an unquoted timestamp into its own type.
            if value.property_type() == PropertyType::Date {
                if looks_like_plain_date(text) {
                    text.clone()
                } else {
                    quote(text)
                }
            } else {
                quote(text)
            }
        }
        PropertyValue::Number(number) => {
            // F09-R01: finite only, and a locale decimal comma never appears in
            // the persisted form.
            if !number.is_finite() {
                return "null".to_owned();
            }
            let rendered = format!("{number}");
            if rendered == "-0" {
                "0".to_owned()
            } else {
                rendered
            }
        }
        PropertyValue::Boolean(flag) => {
            if *flag {
                "true".to_owned()
            } else {
                "false".to_owned()
            }
        }
        PropertyValue::MultiSelect(values) => {
            let items: Vec<String> = values
                .iter()
                .map(|entry| {
                    if needs_quoting(entry) {
                        quote(entry)
                    } else {
                        entry.clone()
                    }
                })
                .collect();
            if items.is_empty() {
                "[]".to_owned()
            } else {
                format!("[{}]", items.join(", "))
            }
        }
        PropertyValue::Unset => "null".to_owned(),
    }
}

fn looks_like_plain_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn validate_name(name: &str) -> Result<&str, PropertyWriteError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || ArrayChars::count(trimmed) > MAX_PROPERTY_NAME_CHARS
        || trimmed.eq_ignore_ascii_case(RESERVED_ID_KEY)
    {
        return Err(PropertyWriteError::InvalidName);
    }
    Ok(trimmed)
}

struct ArrayChars;

impl ArrayChars {
    fn count(text: &str) -> usize {
        text.chars().count()
    }
}

/// Writes one top-level property, preserving every other byte.
pub fn set_property(
    source: &str,
    name: &str,
    value: &PropertyValue,
) -> Result<PropertyWrite, PropertyWriteError> {
    let name = validate_name(name)?;
    let scanner = Scanner::new(source);
    let (open, close) = match scanner.frontmatter() {
        Frontmatter::None => (None, None),
        Frontmatter::Unterminated(span) => {
            return Err(PropertyWriteError::Unterminated).inspect_err(|_| {
                let _ = span;
            })
        }
        Frontmatter::Closed(span) => (Some(span.start), Some(span.end)),
    };
    let eol = eol_of(source);
    // Refuse to edit frontmatter this build cannot read at all. Rewriting it
    // would be a guess, and F09-R03 requires source repair instead.
    match crate::properties::parse_properties(source, &[]) {
        Ok(_) => {}
        Err(crate::properties::PropertyError::Unterminated) => {
            return Err(PropertyWriteError::Unterminated);
        }
        Err(crate::properties::PropertyError::InvalidYaml) => {
            return Err(PropertyWriteError::InvalidYaml);
        }
        Err(crate::properties::PropertyError::NotAMapping) => {
            return Err(PropertyWriteError::Unsupported(
                "frontmatter is not a mapping".to_owned(),
            ));
        }
    }
    let Some(open) = open else {
        // No frontmatter: create one at the very start, keeping a BOM.
        let bom = if source.starts_with('\u{FEFF}') {
            "\u{FEFF}"
        } else {
            ""
        };
        let body = &source[bom.len()..];
        let rendered = format!(
            "---{eol}{name}: {value}{eol}---{eol}{body}",
            value = render_value(value)
        );
        let composed = format!("{bom}{rendered}");
        return finish(source, composed, PropertyValue::Unset);
    };
    let close = close.unwrap_or(open);

    let block = &source[open..close];
    let lines = split_block(block);
    if lines.is_empty() {
        let insert = format!("{name}: {}", render_value(value));
        let mut composed = String::with_capacity(source.len() + insert.len() + eol.len());
        composed.push_str(&source[..open]);
        composed.push_str(&insert);
        composed.push_str(eol);
        composed.push_str(&source[close..]);
        return finish(source, composed, PropertyValue::Unset);
    }
    let first_text = lines[0].text.trim_start();
    if first_text.starts_with('{') || first_text.starts_with('[') {
        return Err(PropertyWriteError::Unsupported(
            "flow-mapping frontmatter".to_owned(),
        ));
    }

    let mut found: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        if key_of(line.text) != Some(name) {
            continue;
        }
        if found.is_some() {
            return Err(PropertyWriteError::Unsupported(
                "duplicate top-level key".to_owned(),
            ));
        }
        found = Some(index);
    }

    let rendered_value = render_value(value);
    let new_line = format!("{name}: {rendered_value}");
    let mut composed = String::with_capacity(source.len() + new_line.len());

    if let Some(index) = found {
        let line = &lines[index];
        let start = open + line.start;
        // A block value (nested mapping or sequence) owns the following indented
        // lines; replacing it with a scalar must remove them, not leave orphans.
        let mut end = open + line.start + line.text.len() + line.terminator.len();
        for next in &lines[index + 1..] {
            if next.text.trim().is_empty() || indent_width(next.text) > 0 {
                end = open + next.start + next.text.len() + next.terminator.len();
            } else {
                break;
            }
        }
        let removed = &source[start..end];
        composed.push_str(&source[..start]);
        composed.push_str(&new_line);
        // Preserve the replaced range's own terminator so the next key keeps
        // its line break and a CRLF file stays CRLF.
        if removed.ends_with("\r\n") {
            composed.push_str("\r\n");
        } else if removed.ends_with('\n') {
            composed.push('\n');
        } else if end < source.len() {
            composed.push_str(eol);
        }
        composed.push_str(&source[end..]);
    } else {
        // Append as the last property, immediately before the closing marker.
        let insert_at = open + block.len();
        let mut head = source[..insert_at].to_owned();
        if !head.ends_with('\n') && !head.is_empty() {
            head.push_str(eol);
        }
        composed.push_str(&head);
        composed.push_str(&new_line);
        composed.push_str(eol);
        composed.push_str(&source[insert_at..]);
    }

    finish(source, composed, previous_value(source, name))
}

fn previous_value(source: &str, name: &str) -> PropertyValue {
    crate::properties::parse_properties(source, &[])
        .ok()
        .and_then(|set| set.get(name).map(|property| property.value.clone()))
        .unwrap_or(PropertyValue::Unset)
}

fn finish(
    original: &str,
    composed: String,
    previous: PropertyValue,
) -> Result<PropertyWrite, PropertyWriteError> {
    // A composed file that no longer parses is a bug, not a note. Refuse it so
    // a property edit can never corrupt the frontmatter (F09-R03).
    if crate::properties::parse_properties(&composed, &[]).is_err() {
        return Err(PropertyWriteError::Unsupported(
            "composed frontmatter does not parse".to_owned(),
        ));
    }
    Ok(PropertyWrite {
        changed: composed != original,
        source: composed,
        previous,
    })
}

/// Splits a block into lines plus their own terminators, so joining the pieces
/// reproduces the block byte-for-byte.
/// One physical line of the frontmatter block, with its own terminator so the
/// pieces join back into the original bytes exactly.
struct BlockLine<'a> {
    start: usize,
    text: &'a str,
    terminator: &'a str,
}

fn split_block(block: &str) -> Vec<BlockLine<'_>> {
    let mut lines = Vec::new();
    let mut cursor = 0;
    while cursor < block.len() {
        let rest = &block[cursor..];
        match rest.find('\n') {
            Some(index) => {
                let raw = &rest[..index];
                let text = raw.strip_suffix('\r').unwrap_or(raw);
                lines.push(BlockLine {
                    start: cursor,
                    text,
                    terminator: &rest[text.len()..index + 1],
                });
                cursor += index + 1;
            }
            None => {
                lines.push(BlockLine {
                    start: cursor,
                    text: rest,
                    terminator: "",
                });
                cursor = block.len();
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> PropertyValue {
        PropertyValue::Text(value.to_owned())
    }

    #[test]
    fn an_existing_scalar_is_replaced_in_place() {
        let source = "---\ntitle: Old\ntags: [a]\n---\n\n# Body\n";
        let write = set_property(source, "title", &text("New")).expect("write");
        assert_eq!(write.source, "---\ntitle: New\ntags: [a]\n---\n\n# Body\n");
        assert_eq!(write.previous, text("Old"));
        assert!(write.changed);
    }

    #[test]
    fn an_absent_key_is_appended_without_touching_other_lines() {
        let source = "---\ntitle: T\ncustom: keep\n---\nbody\n";
        let write =
            set_property(source, "due", &PropertyValue::Date("2026-10-20".into())).expect("write");
        assert_eq!(
            write.source,
            "---\ntitle: T\ncustom: keep\ndue: 2026-10-20\n---\nbody\n"
        );
    }

    #[test]
    fn unknown_fields_and_their_order_survive_an_edit() {
        let source = "---\ncustom-block:\n  nested: 1\ntitle: T\nzeta: z\n---\n";
        let write = set_property(source, "title", &text("Changed")).expect("write");
        assert_eq!(
            write.source,
            "---\ncustom-block:\n  nested: 1\ntitle: Changed\nzeta: z\n---\n"
        );
    }

    #[test]
    fn a_block_value_is_replaced_as_a_whole_block() {
        let source = "---\ntags:\n  - a\n  - b\ntitle: T\n---\n";
        let write = set_property(
            source,
            "tags",
            &PropertyValue::MultiSelect(vec!["c".into(), "d".into()]),
        )
        .expect("write");
        assert_eq!(write.source, "---\ntags: [c, d]\ntitle: T\n---\n");
    }

    #[test]
    fn no_frontmatter_creates_one_and_keeps_a_bom() {
        let write = set_property("\u{FEFF}# Body\n", "title", &text("T")).expect("write");
        assert_eq!(write.source, "\u{FEFF}---\ntitle: T\n---\n# Body\n");
        assert_eq!(write.previous, PropertyValue::Unset);
    }

    #[test]
    fn empty_frontmatter_gets_its_first_key() {
        let write = set_property("---\n---\nbody\n", "title", &text("T")).expect("write");
        assert_eq!(write.source, "---\ntitle: T\n---\nbody\n");
    }

    #[test]
    fn crlf_files_keep_their_line_endings() {
        let source = "---\r\ntitle: Old\r\n---\r\nbody\r\n";
        let write = set_property(source, "title", &text("New")).expect("write");
        assert_eq!(write.source, "---\r\ntitle: New\r\n---\r\nbody\r\n");
    }

    #[test]
    fn zero_false_and_empty_text_stay_distinct() {
        for (value, rendered) in [
            (PropertyValue::Number(0.0), "0"),
            (PropertyValue::Boolean(false), "false"),
            (PropertyValue::Unset, "null"),
        ] {
            let write = set_property("---\ntitle: T\n---\n", "field", &value).expect("write");
            assert!(
                write.source.contains(&format!("field: {rendered}")),
                "{:?} -> {}",
                value,
                write.source
            );
        }
        let empty = set_property("---\n---\n", "field", &text("")).expect("write");
        assert!(empty.source.contains("field: \"\""), "{}", empty.source);
    }

    #[test]
    fn values_yaml_would_retype_are_quoted() {
        assert_eq!(render_value(&text("true")), "\"true\"");
        assert_eq!(render_value(&text("2026-10-20")), "\"2026-10-20\"");
        assert_eq!(render_value(&text("42")), "\"42\"");
        assert_eq!(render_value(&text(" two ")), "\" two \"");
        assert_eq!(render_value(&text("a: b")), "\"a: b\"");
        assert_eq!(render_value(&text("plain")), "plain");
        assert_eq!(render_value(&text("京都")), "京都");
    }

    #[test]
    fn reserved_and_invalid_names_are_refused() {
        for name in ["", "   ", "opennote-id", &"x".repeat(65)] {
            assert_eq!(
                set_property("---\n---\n", name, &text("v")),
                Err(PropertyWriteError::InvalidName),
                "{name}"
            );
        }
        assert!(set_property("---\n---\n", &"x".repeat(64), &text("v")).is_ok());
    }

    #[test]
    fn invalid_and_flow_frontmatter_are_refused_not_rewritten() {
        assert_eq!(
            set_property("---\nkey: [unclosed\n---\n", "title", &text("T")),
            Err(PropertyWriteError::InvalidYaml)
        );
        assert_eq!(
            set_property("---\ntitle: T\n", "title", &text("New")),
            Err(PropertyWriteError::Unterminated)
        );
        assert!(matches!(
            set_property("---\n{title: T}\n---\n", "title", &text("New")),
            Err(PropertyWriteError::Unsupported(_))
        ));
    }

    #[test]
    fn duplicate_keys_are_refused_instead_of_guessed() {
        // Depending on the YAML reader, a duplicate key either fails to parse or
        // parses with one winner. Either way the write must refuse rather than
        // pick one; both refusals are typed.
        let source = "---\ntitle: A\ntitle: B\n---\n";
        assert!(set_property(source, "title", &text("C")).is_err());
    }

    #[test]
    fn a_missing_closing_marker_with_body_is_invalid_yaml() {
        // The scanner reports an unterminated block; no rewrite is attempted.
        assert_eq!(
            set_property("---\ntitle: T\nbody\n", "title", &text("New")),
            Err(PropertyWriteError::Unterminated)
        );
    }

    #[test]
    fn datetime_is_quoted_so_yaml_keeps_it_a_string() {
        let value = PropertyValue::DateTime("2026-10-20T12:30:00Z".into());
        assert_eq!(render_value(&value), "\"2026-10-20T12:30:00Z\"");
        let write = set_property("---\n---\n", "when", &value).expect("write");
        assert!(write.source.contains("when: \"2026-10-20T12:30:00Z\""));
    }

    #[test]
    fn multiselect_with_special_items_is_quoted_item_by_item() {
        let value = PropertyValue::MultiSelect(vec!["a".into(), "b: c".into()]);
        assert_eq!(render_value(&value), "[a, \"b: c\"]");
    }

    #[test]
    fn no_op_write_reports_unchanged() {
        let source = "---\ntitle: T\n---\n";
        let write = set_property(source, "title", &text("T")).expect("write");
        assert!(!write.changed);
        assert_eq!(write.source, source);
    }
}
