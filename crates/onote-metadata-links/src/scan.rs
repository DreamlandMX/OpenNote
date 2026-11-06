//! Shared source scanning for M04.
//!
//! Every position in this crate is a byte offset into the original source
//! string: the editor needs raw offsets for highlighting, and byte offsets are
//! stable under UTF-8 without a second index. Frontmatter and code regions are
//! classified once here so tags, headings, anchors and links all agree on what
//! is content.

/// A half-open byte range `[start, end)` into the note source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }
}

/// Where the leading YAML frontmatter block sits, if the source has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frontmatter {
    /// No `---` block at the very start of the file.
    None,
    /// A closed block; the span covers the block body between the markers.
    Closed(Span),
    /// An opened block that never closes before end of file. The body is still
    /// exposed for source-level repair, but structured parsing must refuse it.
    Unterminated(Span),
}

impl Frontmatter {
    pub fn body(&self) -> Option<Span> {
        match self {
            Frontmatter::None => None,
            Frontmatter::Closed(span) | Frontmatter::Unterminated(span) => Some(*span),
        }
    }
}

/// One physical line; `text` excludes the line terminator.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Line<'a> {
    pub start: usize,
    pub end: usize,
    pub text: &'a str,
}

/// Split `source` into lines without allocating per line. The returned `end`
/// is the offset of the terminating `\n` (or the source length for the last
/// line); a trailing `\r` is stripped from `text` but stays inside the span.
pub(crate) fn lines(source: &str) -> Vec<Line<'_>> {
    let mut result = Vec::new();
    let mut start = 0usize;
    loop {
        let end = match source[start..].find('\n') {
            Some(index) => start + index,
            None => source.len(),
        };
        let text = source[start..end]
            .strip_suffix('\r')
            .unwrap_or(&source[start..end]);
        result.push(Line { start, end, text });
        if end == source.len() {
            return result;
        }
        start = end + 1;
    }
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let count = line.chars().take_while(|c| *c == marker).count();
    (count >= 3).then_some((marker, count))
}

/// A closing fence carries only the marker run and optional whitespace; an
/// opening fence may add an info string.
fn fence_close(line: &str, marker: char, size: usize) -> bool {
    let count = line.chars().take_while(|c| *c == marker).count();
    count >= size && line[count..].trim().is_empty()
}

/// Four leading spaces or a tab is an indented code block. This matches the
/// existing attachment-reference scanner in M08 so both agree on what is code.
fn indented_code(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

fn inline_code_spans(line: &Line<'_>) -> Vec<Span> {
    let bytes = line.text.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }
        let mut run = 0usize;
        while index + run < bytes.len() && bytes[index + run] == b'`' {
            run += 1;
        }
        let mut cursor = index + run;
        let mut closed = None;
        while cursor < bytes.len() {
            if bytes[cursor] == b'`' {
                let mut close = 0usize;
                while cursor + close < bytes.len() && bytes[cursor + close] == b'`' {
                    close += 1;
                }
                if close == run {
                    closed = Some(cursor + close);
                    break;
                }
                cursor += close;
                continue;
            }
            cursor += 1;
        }
        match closed {
            Some(end) => {
                spans.push(Span {
                    start: line.start + index,
                    end: line.start + end,
                });
                index = end;
            }
            // An unmatched run is literal text, never a code span.
            None => index += run,
        }
    }
    spans
}

/// Classifies frontmatter and code regions once per source.
#[derive(Debug, Clone)]
pub struct Scanner<'a> {
    source: &'a str,
    frontmatter: Frontmatter,
    body_start: usize,
    code: Vec<Span>,
}

impl<'a> Scanner<'a> {
    pub fn new(source: &'a str) -> Self {
        let bom_len = if source.starts_with('\u{FEFF}') {
            '\u{FEFF}'.len_utf8()
        } else {
            0
        };
        let all = lines(source);
        let mut frontmatter = Frontmatter::None;
        let mut body_start = bom_len;
        if let Some(opening) = all.first() {
            // A byte-order mark is not whitespace, so it must be dropped before
            // the marker comparison or a BOM file would never get frontmatter.
            let first_text = &opening.text[bom_len.min(opening.text.len())..];
            if first_text.trim() == "---" {
                let closing = all
                    .iter()
                    .filter(|line| line.start > opening.start)
                    .find(|line| {
                        let core = line.text.trim_end_matches([' ', '\t']);
                        core == "---" || core == "..."
                    });
                match closing {
                    Some(line) => {
                        frontmatter = Frontmatter::Closed(Span {
                            start: (opening.end + 1).min(source.len()),
                            end: line.start,
                        });
                        body_start = (line.end + 1).min(source.len());
                    }
                    None => {
                        frontmatter = Frontmatter::Unterminated(Span {
                            start: (opening.end + 1).min(source.len()),
                            end: source.len(),
                        });
                        body_start = source.len();
                    }
                }
            }
        }
        let mut code = Vec::new();
        let mut fence: Option<(char, usize)> = None;
        for line in all.iter().filter(|line| line.start >= body_start) {
            let trimmed = line.text.trim_start_matches([' ', '\t']);
            // Whole-line code spans swallow their newline so that no content
            // range is ever a lone line terminator between two code lines.
            let whole_line = Span {
                start: line.start,
                end: (line.end + 1).min(source.len()),
            };
            if let Some((marker, count)) = fence_marker(trimmed) {
                match fence {
                    Some((open, size)) if open == marker && fence_close(trimmed, marker, size) => {
                        fence = None;
                    }
                    None => fence = Some((marker, count)),
                    _ => (),
                }
                code.push(whole_line);
                continue;
            }
            if fence.is_some() || indented_code(line.text) {
                code.push(whole_line);
                continue;
            }
            code.extend(inline_code_spans(line));
        }
        let mut merged: Vec<Span> = Vec::with_capacity(code.len());
        for span in code {
            match merged.last_mut() {
                Some(previous) if span.start <= previous.end => {
                    previous.end = previous.end.max(span.end);
                }
                _ => merged.push(span),
            }
        }
        Self {
            source,
            frontmatter,
            body_start,
            code: merged,
        }
    }

    pub fn source(&self) -> &'a str {
        self.source
    }

    pub fn frontmatter(&self) -> Frontmatter {
        self.frontmatter
    }

    /// Frontmatter body text. Unterminated blocks still expose their text so
    /// the user can repair it; callers that need valid YAML must check the
    /// [`Frontmatter`] state first.
    pub fn frontmatter_body(&self) -> Option<&'a str> {
        self.frontmatter
            .body()
            .map(|span| &self.source[span.start..span.end])
    }

    /// First byte offset that is not frontmatter (BOM excluded).
    pub fn body_start(&self) -> usize {
        self.body_start
    }

    pub fn code_spans(&self) -> &[Span] {
        &self.code
    }

    pub fn in_code(&self, offset: usize) -> bool {
        self.code.iter().any(|span| span.contains(offset))
    }

    /// A copy of the source where every code span is replaced by spaces of the
    /// same byte length. Offsets stay valid, so callers can scan the masked
    /// text and report offsets into the original source. Line breaks survive
    /// masking so the masked text keeps the original line structure.
    pub fn masked_source(&self) -> String {
        let mut masked = String::with_capacity(self.source.len());
        let mut cursor = 0usize;
        for span in &self.code {
            let start = span.start.min(self.source.len());
            let end = span.end.min(self.source.len());
            if start > cursor {
                masked.push_str(&self.source[cursor..start]);
            }
            if end > start {
                for byte in self.source[start..end].bytes() {
                    masked.push(if byte == b'\n' { '\n' } else { ' ' });
                }
            }
            cursor = cursor.max(end);
        }
        if cursor < self.source.len() {
            masked.push_str(&self.source[cursor..]);
        }
        masked
    }

    /// Content ranges are body bytes minus code spans, in ascending order.
    pub fn content_ranges(&self) -> Vec<Span> {
        let mut ranges = Vec::new();
        let mut cursor = self.body_start;
        for span in &self.code {
            if span.start > cursor {
                ranges.push(Span {
                    start: cursor,
                    end: span.start.min(self.source.len()),
                });
            }
            cursor = cursor.max(span.end);
        }
        if cursor < self.source.len() {
            ranges.push(Span {
                start: cursor,
                end: self.source.len(),
            });
        }
        ranges
    }

    /// True when the offset is ordinary body content: not frontmatter, not code.
    pub fn is_content(&self, offset: usize) -> bool {
        offset >= self.body_start && !self.in_code(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_spans_and_body_start_are_byte_exact() {
        let source = "\u{FEFF}---\r\ntitle: a\r\n---\r\nbody\r\n";
        let scanner = Scanner::new(source);
        let body = scanner.frontmatter_body().unwrap();
        assert_eq!(body, "title: a\r\n");
        assert_eq!(&source[scanner.body_start()..], "body\r\n");
    }

    #[test]
    fn unterminated_frontmatter_stays_repairable_and_leaves_no_body() {
        let source = "---\ntitle: a\n";
        let scanner = Scanner::new(source);
        assert!(matches!(
            scanner.frontmatter(),
            Frontmatter::Unterminated(_)
        ));
        assert_eq!(scanner.frontmatter_body(), Some("title: a\n"));
        assert_eq!(scanner.body_start(), source.len());
    }

    #[test]
    fn fenced_inline_and_indented_code_are_excluded_from_content() {
        let source = "body `code` tail\n```\nfenced\n```\n    indented\nplain\n";
        let scanner = Scanner::new(source);
        assert!(scanner.in_code(source.find("code").unwrap()));
        assert!(scanner.in_code(source.find("fenced").unwrap()));
        assert!(scanner.in_code(source.find("indented").unwrap()));
        assert!(!scanner.in_code(source.find("tail").unwrap()));
        assert!(!scanner.in_code(source.find("plain").unwrap()));
        let content: Vec<&str> = scanner
            .content_ranges()
            .into_iter()
            .map(|span| &source[span.start..span.end])
            .collect();
        assert_eq!(content, vec!["body ", " tail\n", "plain\n"]);
    }

    #[test]
    fn a_longer_fence_run_inside_a_fence_is_content_until_it_closes() {
        let source = "````\n```\nstill code\n````\nafter\n";
        let scanner = Scanner::new(source);
        assert!(scanner.in_code(source.find("still code").unwrap()));
        assert!(!scanner.in_code(source.find("after").unwrap()));
    }

    #[test]
    fn masked_source_keeps_every_byte_offset() {
        let source = "a `code` b\n#tag\n";
        let masked = Scanner::new(source).masked_source();
        assert_eq!(masked.len(), source.len());
        assert_eq!(masked, "a        b\n#tag\n");
        assert_eq!(masked.find("#tag"), source.find("#tag"));
    }
}
