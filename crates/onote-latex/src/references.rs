//! Shared TeX reference extraction for F45 inspection and F54 export.
//!
//! Both features must agree on which paths a source refers to: inspection uses
//! them to refuse out-of-root references, export uses them to compute the file
//! closure. One parser keeps a spelling from being understood in one place and
//! silently missed in the other.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReferenceKind {
    Input,
    Graphics,
    Bib,
}

impl ReferenceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Graphics => "graphics",
            Self::Bib => "bib",
        }
    }
}

/// Extracts referenced paths and their kind.
///
/// TeX allows whitespace between a control word and its argument, and
/// `\includegraphics` usually carries an optional `[width=...]` block. Matching
/// only the tight `\input{` spelling would let `\input {../outside}` or
/// `\includegraphics[width=1in]{../outside}` skip validation.
pub(crate) fn references_in(text: &str) -> Vec<(String, ReferenceKind)> {
    let mut found: Vec<(String, ReferenceKind)> = Vec::new();
    for (command, kind, options) in [
        ("\\input", ReferenceKind::Input, false),
        ("\\include", ReferenceKind::Input, false),
        ("\\includegraphics", ReferenceKind::Graphics, true),
        ("\\bibliography", ReferenceKind::Bib, false),
    ] {
        let mut rest = text;
        while let Some(start) = rest.find(command) {
            let after = &rest[start + command.len()..];
            // A longer control word such as `\inputencoding` is not this command.
            if after.starts_with(|character: char| character.is_ascii_alphabetic()) {
                rest = after;
                continue;
            }
            let Some(body) = reference_body(after, options) else {
                rest = after;
                continue;
            };
            let Some(end) = body.find('}') else {
                rest = after;
                continue;
            };
            for part in body[..end].split(',') {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    found.push((trimmed.to_owned(), kind));
                }
            }
            rest = &body[end..];
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Skips the whitespace, optional `*` and optional `[options]` blocks that TeX
/// allows before a command's braced argument, returning the text after `{`.
///
/// `\includegraphics` accepts a star, a key-value block and the legacy
/// two-block bounding-box form, so every leading `[...]` must be skipped;
/// leaving one behind would hide the path from both inspection and export.
fn reference_body<'a>(after: &'a str, options: bool) -> Option<&'a str> {
    let mut rest = after.trim_start();
    if options {
        rest = rest.strip_prefix('*').unwrap_or(rest).trim_start();
        while let Some(inner) = rest.strip_prefix('[') {
            let close = inner.find(']')?;
            rest = inner[close + 1..].trim_start();
        }
    }
    rest.strip_prefix('{')
}
