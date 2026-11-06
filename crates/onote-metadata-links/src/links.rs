//! F10/F11 internal link and embed parsing.
//!
//! Internal links are the wiki syntax the F10 flow inserts: `[[target]]`,
//! `[[target|label]]`, `[[target#anchor]]` and the embed form `![[…]]`. Every
//! occurrence inside fenced, indented or inline code is ignored (F10-R01).

use crate::scan::Scanner;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Link,
    Embed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawLink {
    pub kind: LinkKind,
    /// Raw target as written: note id, readable path or title. Empty for a
    /// same-note `[[#anchor]]` reference.
    pub target: String,
    pub label: Option<String>,
    pub anchor: Option<String>,
    /// Byte offset of the `!` for embeds, of the first `[` otherwise.
    pub offset: usize,
    pub length: usize,
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

/// Offset of the closing `]]`, or `None` when the opener is not a complete,
/// single-line link (nested `[`, raw newline, end of file).
fn find_close(bytes: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while index + 1 < bytes.len() {
        match bytes[index] {
            b'\n' | b'[' => return None,
            b']' if bytes[index + 1] == b']' => return Some(index),
            b'\\' => index += 2,
            _ => index += 1,
        }
    }
    None
}

fn parse_inner(kind: LinkKind, inner: &str, offset: usize, length: usize) -> Option<RawLink> {
    let (target_part, label) = match inner.split_once('|') {
        Some((target, label)) => (target, Some(label.trim())),
        None => (inner, None),
    };
    let (path, anchor) = match target_part.split_once('#') {
        Some((path, anchor)) => (path.trim(), Some(anchor.trim())),
        None => (target_part.trim(), None),
    };
    let anchor = anchor.filter(|value| !value.is_empty()).map(str::to_owned);
    if path.is_empty() && anchor.is_none() {
        return None;
    }
    Some(RawLink {
        kind,
        target: path.to_owned(),
        label: label.filter(|value| !value.is_empty()).map(str::to_owned),
        anchor,
        offset,
        length,
    })
}

/// Links and embeds outside code, in document order.
pub fn extract_links(source: &str) -> Vec<RawLink> {
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let bytes = masked.as_bytes();
    let mut links = Vec::new();
    let mut index = scanner.body_start();
    while index + 1 < bytes.len() {
        if bytes[index] != b'[' || bytes[index + 1] != b'[' || escaped(bytes, index) {
            index += 1;
            continue;
        }
        let Some(close) = find_close(bytes, index + 2) else {
            index += 2;
            continue;
        };
        let (kind, start) = match index.checked_sub(1) {
            Some(previous) if bytes[previous] == b'!' && !escaped(bytes, previous) => {
                (LinkKind::Embed, previous)
            }
            _ => (LinkKind::Link, index),
        };
        if let Some(link) = parse_inner(kind, &masked[index + 2..close], start, close + 2 - start) {
            links.push(link);
        }
        index = close + 2;
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_blocks_and_inline_code_never_produce_links() {
        let source = "[[real]]\n```\n[[fenced]]\n```\n`[[inline]]`\n    [[indented]]\n";
        let targets: Vec<String> = extract_links(source)
            .into_iter()
            .map(|link| link.target)
            .collect();
        assert_eq!(targets, vec!["real"]);
    }

    #[test]
    fn label_anchor_and_embed_forms_are_parsed_with_offsets() {
        let source = "see ![[target#Section|Alias]] and [[#local]] and [[plain]]\n";
        let links = extract_links(source);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].kind, LinkKind::Embed);
        assert_eq!(links[0].target, "target");
        assert_eq!(links[0].anchor.as_deref(), Some("Section"));
        assert_eq!(links[0].label.as_deref(), Some("Alias"));
        assert_eq!(links[0].offset, source.find("![[").unwrap());
        assert_eq!(links[0].length, "![[target#Section|Alias]]".len());
        assert_eq!(links[1].target, "");
        assert_eq!(links[1].anchor.as_deref(), Some("local"));
        assert_eq!(links[2].target, "plain");
        assert!(links[2].anchor.is_none());
    }

    #[test]
    fn escaped_and_incomplete_brackets_are_not_links() {
        let source = "\\[[escaped]]\n[[unclosed\n[[a[b]]\n[[ok]]\n";
        let targets: Vec<String> = extract_links(source)
            .into_iter()
            .map(|link| link.target)
            .collect();
        assert_eq!(targets, vec!["ok"]);
    }

    #[test]
    fn an_empty_link_is_ignored() {
        assert!(extract_links("[[]]\n[[|label]]\n").is_empty());
    }
}
