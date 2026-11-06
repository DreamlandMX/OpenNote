//! F08 tag parsing: frontmatter `tags`/`tag` plus body `#tag` outside code.

use crate::scan::{lines, Frontmatter, Scanner};
use serde::{Deserialize, Serialize};
use yaml_rust2::{Yaml, YamlLoader};

/// F08 field rule: a tag is 1–64 characters after edge trimming.
pub const MAX_TAG_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TagSource {
    Frontmatter,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagOccurrence {
    pub tag: String,
    /// Byte offset of the `#` (body) or of the frontmatter key line.
    pub offset: usize,
    pub source: TagSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagIssue {
    Empty,
    TooLong,
}

/// Trims edge whitespace and enforces the 1–64 character rule. Display case is
/// preserved; identity comparisons belong to the caller.
pub fn validate_tag(raw: &str) -> Result<String, TagIssue> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TagIssue::Empty);
    }
    if trimmed.chars().count() > MAX_TAG_CHARS {
        return Err(TagIssue::TooLong);
    }
    Ok(trimmed.to_owned())
}

fn is_tag_char(value: char) -> bool {
    value.is_alphanumeric() || matches!(value, '_' | '-' | '/')
}

fn escaped(bytes: &[u8], index: usize) -> bool {
    let mut slashes = 0usize;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

/// All tags in `source`: frontmatter keys first (in key order), then body
/// occurrences in document order. Code spans and fenced/indented code blocks
/// never produce tags (F08-R01, F08-A02).
pub fn extract_tags(source: &str) -> Vec<TagOccurrence> {
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let mut tags = frontmatter_tags(source, &scanner);
    body_tags(&masked, scanner.body_start(), &mut tags);
    tags
}

fn frontmatter_tags(source: &str, scanner: &Scanner<'_>) -> Vec<TagOccurrence> {
    let Frontmatter::Closed(span) = scanner.frontmatter() else {
        return Vec::new();
    };
    let Ok(documents) = YamlLoader::load_from_str(&source[span.start..span.end]) else {
        return Vec::new();
    };
    let Some(Yaml::Hash(entries)) = documents.first() else {
        return Vec::new();
    };
    let mut tags = Vec::new();
    for (key, value) in entries {
        let Some(name) = key.as_str() else {
            continue;
        };
        if !matches!(name, "tags" | "tag") {
            continue;
        }
        let offset = key_line_offset(source, span, name).unwrap_or(span.start);
        let mut values = Vec::new();
        collect_yaml_tags(value, &mut values);
        for value in values {
            if let Ok(tag) = validate_tag(&value) {
                tags.push(TagOccurrence {
                    tag,
                    offset,
                    source: TagSource::Frontmatter,
                });
            }
        }
    }
    tags
}

fn collect_yaml_tags(value: &Yaml, out: &mut Vec<String>) {
    match value {
        Yaml::Array(items) => {
            for item in items {
                collect_yaml_tags(item, out);
            }
        }
        Yaml::String(text) => {
            for part in text.split(',') {
                let part = part.trim();
                if !part.is_empty() {
                    out.push(part.to_owned());
                }
            }
        }
        _ => (),
    }
}

fn key_line_offset(source: &str, span: crate::scan::Span, name: &str) -> Option<usize> {
    for line in lines(source)
        .into_iter()
        .filter(|line| line.start >= span.start && line.end <= span.end)
    {
        let trimmed = line.text.trim_start();
        if let Some(rest) = trimmed.strip_prefix(name) {
            if rest.starts_with(':') {
                return Some(line.start + (line.text.len() - trimmed.len()));
            }
        }
    }
    None
}

fn body_tags(masked: &str, body_start: usize, out: &mut Vec<TagOccurrence>) {
    for line in lines(masked)
        .into_iter()
        .filter(|line| line.start >= body_start)
    {
        let chars: Vec<(usize, char)> = line.text.char_indices().collect();
        let bytes = line.text.as_bytes();
        let mut index = 0usize;
        while index < chars.len() {
            let (offset, value) = chars[index];
            if value != '#' || escaped(bytes, offset) {
                index += 1;
                continue;
            }
            let preceded_ok = match index.checked_sub(1).map(|previous| chars[previous].1) {
                None => true,
                Some(previous) => {
                    previous.is_whitespace()
                        || matches!(previous, '(' | '[' | '{' | '<' | '"' | '\'' | ',' | ';')
                }
            };
            if !preceded_ok {
                index += 1;
                continue;
            }
            let mut end = index + 1;
            while end < chars.len() && is_tag_char(chars[end].1) {
                end += 1;
            }
            while end > index + 1 && matches!(chars[end - 1].1, '/' | '-' | '_') {
                end -= 1;
            }
            if end > index + 1 {
                let start_byte = chars[index + 1].0;
                let end_byte = chars
                    .get(end)
                    .map_or(line.text.len(), |(offset, _)| *offset);
                let candidate = &line.text[start_byte..end_byte];
                // A purely numeric run is a colour code or an issue number, not a tag.
                if candidate.chars().any(|value| !value.is_ascii_digit()) {
                    if let Ok(tag) = validate_tag(candidate) {
                        out.push(TagOccurrence {
                            tag,
                            offset: line.start + offset,
                            source: TagSource::Body,
                        });
                    }
                }
            }
            index = end.max(index + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(tags: &[TagOccurrence]) -> Vec<&str> {
        tags.iter()
            .filter(|tag| tag.source == TagSource::Body)
            .map(|tag| tag.tag.as_str())
            .collect()
    }

    #[test]
    fn code_blocks_and_inline_code_never_produce_tags() {
        let source = "#real\n```\n#例子\n```\n`#inline`\n    #indented\n## 标题\n";
        assert_eq!(body(&extract_tags(source)), vec!["real"]);
    }

    #[test]
    fn nested_and_unicode_tags_keep_display_case_and_offsets() {
        let source = "见 #项目/子项 与 #Ünicode。\n";
        let tags = extract_tags(source);
        assert_eq!(body(&tags), vec!["项目/子项", "Ünicode"]);
        assert_eq!(tags[0].offset, source.find("#项目").unwrap());
    }

    #[test]
    fn frontmatter_list_scalar_and_sequence_tags_are_read() {
        let source = "---\ntags: [a, b]\ntag: c, d\n---\nbody\n";
        let tags = extract_tags(source);
        assert!(tags.iter().all(|tag| tag.source == TagSource::Frontmatter));
        let names: Vec<&str> = tags.iter().map(|tag| tag.tag.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c", "d"]);
        let sequence = "---\ntags:\n  - one\n  - two\n---\nbody\n";
        let names: Vec<String> = extract_tags(sequence)
            .into_iter()
            .map(|tag| tag.tag)
            .collect();
        assert_eq!(names, vec!["one", "two"]);
    }

    #[test]
    fn a_hash_inside_a_word_or_link_is_not_a_tag() {
        let source = "abc#notag [[note#anchor]] #yes\n";
        assert_eq!(body(&extract_tags(source)), vec!["yes"]);
    }

    #[test]
    fn tag_validation_trims_edges_and_bounds_length() {
        assert_eq!(validate_tag("  work  "), Ok("work".into()));
        assert_eq!(validate_tag("   "), Err(TagIssue::Empty));
        assert_eq!(validate_tag(&"a".repeat(64)), Ok("a".repeat(64)));
        assert_eq!(validate_tag(&"a".repeat(65)), Err(TagIssue::TooLong));
    }

    #[test]
    fn unterminated_frontmatter_produces_no_frontmatter_tags() {
        let source = "---\ntags: [a]\n";
        assert!(extract_tags(source).is_empty());
    }
}
