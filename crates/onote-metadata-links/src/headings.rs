//! F14/F10/F11 heading, slug and explicit block-anchor parsing.

use crate::scan::{lines, Scanner};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One ATX heading. `line` is a zero-based line index; `offset` is the byte
/// offset of the line start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub slug: String,
    pub offset: usize,
    pub line: usize,
}

/// An explicit `^anchor-id` at the end of a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockAnchor {
    pub id: String,
    pub offset: usize,
    pub line: usize,
}

/// Obsidian-style slug: lowercase, whitespace and `-` collapse into a single
/// `-`, other punctuation is dropped. Only alphanumerics and `_` survive.
pub fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for value in text.trim().chars() {
        if value.is_alphanumeric() || value == '_' {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.extend(value.to_lowercase());
        } else if value.is_whitespace() || value == '-' {
            pending_dash = true;
        }
    }
    slug
}

fn atx_marker(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    let indent = line.len() - trimmed.len();
    if indent > 3 {
        return None;
    }
    let hashes = trimmed.chars().take_while(|value| *value == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    // `#tag` is a tag, not a heading: an ATX heading needs a space or EOL.
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((hashes as u8, indent + hashes))
}

fn strip_closing(text: &str) -> String {
    let trimmed = text.trim_end();
    let hashes = trimmed
        .chars()
        .rev()
        .take_while(|value| *value == '#')
        .count();
    if hashes == 0 {
        return trimmed.to_owned();
    }
    let head = &trimmed[..trimmed.len() - hashes];
    if head.is_empty() || head.ends_with([' ', '\t']) {
        head.trim_end().to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Headings outside code, in document order. Duplicate slugs are suffixed
/// `-1`, `-2`, … so every heading keeps a distinct anchor.
pub fn extract_headings(source: &str) -> Vec<Heading> {
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let original = lines(source);
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut headings = Vec::new();
    for (index, line) in lines(&masked).into_iter().enumerate() {
        if line.start < scanner.body_start() {
            continue;
        }
        let Some((level, marker_end)) = atx_marker(line.text) else {
            continue;
        };
        let text = strip_closing(original[index].text[marker_end..].trim());
        let base = heading_slug(&text);
        let mut slug = base.clone();
        let mut suffix = 0u32;
        while taken.contains(&slug) {
            suffix += 1;
            slug = format!("{base}-{suffix}");
        }
        taken.insert(slug.clone());
        headings.push(Heading {
            level,
            text,
            slug,
            offset: line.start,
            line: index,
        });
    }
    headings
}

/// Explicit `^anchor-id` markers outside code. The marker must close the line
/// and be preceded by whitespace, which keeps `x^2` in prose from becoming an
/// anchor.
pub fn extract_block_anchors(source: &str) -> Vec<BlockAnchor> {
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let mut anchors = Vec::new();
    for (index, line) in lines(&masked).into_iter().enumerate() {
        if line.start < scanner.body_start() {
            continue;
        }
        let trimmed = line.text.trim_end();
        let Some(caret) = trimmed.rfind('^') else {
            continue;
        };
        let id = &trimmed[caret + 1..];
        if id.is_empty()
            || !id
                .chars()
                .all(|value| value.is_alphanumeric() || matches!(value, '-' | '_'))
            || !trimmed[..caret].ends_with(char::is_whitespace)
        {
            continue;
        }
        anchors.push(BlockAnchor {
            id: id.to_owned(),
            offset: line.start + caret,
            line: index,
        });
    }
    anchors
}

/// Every anchor a link may target: heading slugs first, then block ids, in
/// document order. Heading text itself is matched case-insensitively by
/// [`crate::resolve::anchor_matches`] in addition to these normalized forms.
pub fn anchors_of(source: &str) -> Vec<String> {
    let mut anchors: Vec<String> = extract_headings(source)
        .into_iter()
        .flat_map(|heading| [heading.slug, heading.text])
        .collect();
    anchors.extend(
        extract_block_anchors(source)
            .into_iter()
            .map(|anchor| anchor.id),
    );
    anchors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_blocks_never_contribute_headings_or_anchors() {
        let source = "```\n# not a heading\n```\n# real\n\n    # indented\n";
        let headings = extract_headings(source);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "real");
        assert!(extract_block_anchors("```\nx ^inside\n```\n").is_empty());
    }

    #[test]
    fn duplicate_headings_get_distinct_slugs_in_document_order() {
        let source = "# Notes\n## Notes\n### Notes\n";
        let slugs: Vec<String> = extract_headings(source)
            .into_iter()
            .map(|h| h.slug)
            .collect();
        assert_eq!(slugs, vec!["notes", "notes-1", "notes-2"]);
    }

    #[test]
    fn a_tag_line_is_not_a_heading_and_inline_code_keeps_heading_text() {
        let headings = extract_headings("#tag\n## Use `code` here\n");
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "Use `code` here");
        assert_eq!(headings[0].slug, "use-code-here");
    }

    #[test]
    fn closing_hashes_and_unicode_slugs_follow_the_same_rule() {
        let headings = extract_headings("## 标题 ##\n### Well-Known Name ###\n");
        assert_eq!(headings[0].text, "标题");
        assert_eq!(headings[0].slug, "标题");
        assert_eq!(headings[1].text, "Well-Known Name");
        assert_eq!(headings[1].slug, "well-known-name");
    }

    #[test]
    fn explicit_block_anchors_need_whitespace_and_valid_ids() {
        let source = "para one ^block-a\nmath x^2 here\npara two ^bad!id\n";
        let anchors = extract_block_anchors(source);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].id, "block-a");
        assert_eq!(anchors[0].line, 0);
    }

    #[test]
    fn anchors_include_heading_slugs_text_and_block_ids() {
        let anchors = anchors_of("# Target\n\ntext ^block\n");
        assert_eq!(anchors, vec!["target", "Target", "block"]);
    }
}
