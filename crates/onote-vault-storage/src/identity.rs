use onote_domain::{ErrorCode, ServiceError};
use yaml_rust2::{Yaml, YamlLoader};

use crate::errors::{self, SeaRes};

/// Frontmatter identity handling. Only the reserved `opennote-id` key is ever
/// inserted/validated; unknown YAML lines are copied verbatim, never
/// reserialized or reordered. The YAML itself is interpreted by yaml-rust2's
/// `YamlLoader` (which rejects duplicate mapping keys), and the same parser is
/// reused by scan and by explicit create/save composition — there is no custom
/// event walker and no reserialization anywhere.
pub(crate) enum Frontmatter {
    /// No leading frontmatter block (BOM-only or other body-first text).
    NoBlock,
    /// A closed `---`/`...` block immediately after the (optional) BOM.
    WellFormed {
        /// Byte offset in the original source right after the opening marker
        /// line terminator: where a missing identity line is inserted.
        insert_at: usize,
        id: BlockId,
    },
    /// Unterminated block, YAML that fails to load (including duplicate
    /// mapping keys), multiple documents inside one block, or a top level
    /// that is neither a mapping nor empty/comment-only.
    Malformed,
}

pub(crate) enum BlockId {
    /// No top-level `opennote-id` key.
    None,
    /// Top-level `opennote-id` scalar value that is a valid UUID,
    /// canonicalized to the hyphenated lowercase form.
    Valid(String),
    /// Top-level `opennote-id` present but missing, non-scalar, or not a
    /// valid UUID: ordinary known-reserved slot, reject and authorize nothing.
    Invalid,
}

const BOM: &str = "\u{FEFF}";
const RESERVED_ID: &str = "opennote-id";

/// The single frontmatter parser shared by scan and compose. All positions
/// are byte offsets; a leading Unicode BOM is skipped by its exact 3-byte
/// length, never via character counting.
pub(crate) fn parse_frontmatter(source: &str) -> Frontmatter {
    let rest = match source.strip_prefix(BOM) {
        Some(rest) => rest,
        None => source,
    };
    let bom_len = source.len() - rest.len(); // 0 or 3 bytes
    let (first_line, after_first) = split_line_at(rest, 0);
    if first_line.trim_end_matches('\r').trim() != "---" {
        return Frontmatter::NoBlock;
    }
    let mut byte_pos = after_first;
    loop {
        if byte_pos >= rest.len() {
            // The block was never closed before the end of the file.
            return Frontmatter::Malformed;
        }
        let (line, next) = split_line_at(rest, byte_pos);
        if is_marker(line) {
            // Block body: everything between the opening and closing marker
            // lines, verbatim; the closing marker is never forwarded as YAML.
            let block_body = &rest[after_first..byte_pos];
            let id = match YamlLoader::load_from_str(block_body) {
                Err(_) => return Frontmatter::Malformed,
                Ok(docs) => match docs.len() {
                    // empty or comment-only block
                    0 => BlockId::None,
                    1 => match &docs[0] {
                        Yaml::Hash(entries) => {
                            let mut id = BlockId::None;
                            for (key, value) in entries.iter() {
                                if key.as_str() == Some(RESERVED_ID) {
                                    // exactly the top-level slot is
                                    // reserved; nested `opennote-id`
                                    // keys are ordinary unknown YAML
                                    id = match value.as_str() {
                                        Some(raw) => match uuid::Uuid::parse_str(raw) {
                                            Ok(parsed) => BlockId::Valid(parsed.to_string()),
                                            Err(_) => BlockId::Invalid,
                                        },
                                        // missing, integer/float/bool or
                                        // collection value: not a scalar
                                        // UUID
                                        None => BlockId::Invalid,
                                    };
                                    break;
                                }
                            }
                            id
                        }
                        // a whole-document scalar/sequence/null is not
                        // "one mapping"
                        _ => return Frontmatter::Malformed,
                    },
                    // multiple documents inside a single block
                    _ => return Frontmatter::Malformed,
                },
            };
            return Frontmatter::WellFormed {
                insert_at: bom_len + after_first,
                id,
            };
        }
        byte_pos = next;
    }
}

/// A closing marker must sit at column zero: leading horizontal whitespace
/// keeps the line as block-scalar content (e.g. a `---` line inside a
/// `description: |` literal). An optional `\r` and trailing horizontal
/// whitespace are tolerated.
fn is_marker(line: &str) -> bool {
    let core = line
        .strip_suffix('\r')
        .unwrap_or(line)
        .trim_end_matches([' ', '\t']);
    core == "---" || core == "..."
}

/// Split off one line starting at `byte_pos`; returns the line without its
/// terminator and the byte position directly after the terminator.
fn split_line_at(s: &str, byte_pos: usize) -> (&str, usize) {
    let remainder = &s[byte_pos..];
    match remainder.find('\n') {
        Some(nl) => (&remainder[..nl], byte_pos + nl + 1),
        None => (remainder, s.len()),
    }
}

fn detect_eol(source: &str) -> &'static str {
    if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Line-ending style of a whole document. A lone `\r` (not immediately
/// followed by `\n`) is data, not a terminator: it never participates in a
/// uniform restore target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEol {
    /// No `\r` byte anywhere.
    Lf,
    /// Every terminator is exactly `\r\n`; no bare `\n`, no lone `\r`.
    Crlf,
    /// Bare `\n` mixed with `\r\n`, or a lone `\r` anywhere.
    Mixed,
}

fn classify_eol(text: &str) -> TextEol {
    let bytes = text.as_bytes();
    let mut has_crlf = false;
    let mut has_bare_lf = false;
    let mut has_lone_cr = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if i + 1 < bytes.len() && bytes[i + 1] == b'\n' => {
                has_crlf = true;
                i += 2;
                continue;
            }
            b'\r' => has_lone_cr = true,
            b'\n' => has_bare_lf = true,
            _ => {}
        }
        i += 1;
    }
    if has_lone_cr || (has_crlf && has_bare_lf) {
        TextEol::Mixed
    } else if has_crlf {
        TextEol::Crlf
    } else {
        TextEol::Lf
    }
}

fn has_lone_cr(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(i, b)| *b == b'\r' && (i + 1 == bytes.len() || bytes[i + 1] != b'\n'))
}

/// Only for a uniformly classified style; a mixed style has no single
/// terminator to insert and every insertion-needing path refuses instead.
fn eol_string(eol: TextEol) -> Option<&'static str> {
    match eol {
        TextEol::Lf => Some("\n"),
        TextEol::Crlf => Some("\r\n"),
        TextEol::Mixed => None,
    }
}

fn with_reference_bom(has_bom: bool, body: String) -> String {
    if has_bom {
        format!("{BOM}{body}")
    } else {
        body
    }
}

fn line_endings_unsupported() -> ServiceError {
    errors::err(ErrorCode::InvalidInput, "error.lineEndingsUnsupported", &[])
}

/// Byte-offset insertion; offsets come from `parse_frontmatter` and always
/// land on a char boundary. `eol` keeps a legacy file's own line endings.
fn insert_identity(source: &str, insert_at: usize, note_id: &str, eol: &str) -> String {
    let (head, tail) = source.split_at(insert_at);
    format!("{head}opennote-id: {note_id}{eol}{tail}")
}

fn first_non_ws(text: &str) -> Option<usize> {
    text.find(|c: char| c != ' ' && c != '\t' && c != '\r' && c != '\n')
}

/// Insert the identity so the frontmatter still parses: a root flow mapping
/// takes the entry right after its opening `{`, preserving every original
/// raw byte (a plain prepended key line would invalidate the flow); anything
/// else uses the plain leading-key prepend. Composed artifacts are verified
/// with the shared parser before any successful return, so no caller can
/// persist invalid frontmatter from an unsupported shape either.
fn identity_insert(source: &str, insert_at: usize, note_id: &str, eol: &str) -> String {
    if let Some(rel) = first_non_ws(&source[insert_at..]) {
        let brace_at = insert_at + rel;
        if source[brace_at..].starts_with('{') {
            let (head, tail) = source.split_at(brace_at + 1);
            return format!("{head}opennote-id: {note_id},{tail}");
        }
    }
    insert_identity(source, insert_at, note_id, eol)
}

/// Verify a composed artifact with the shared parser: it must re-parse as
/// exactly this note's canonical identity.
fn verified(composed: String, note_id: &str) -> SeaRes<String> {
    match parse_frontmatter(&composed) {
        Frontmatter::WellFormed {
            id: BlockId::Valid(value),
            ..
        } if value == note_id => Ok(composed),
        _ => Err(malformed_error()),
    }
}

/// Sentinel UUID for dry-running the exact insertion compose would perform.
const PROBE_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Whether an identity could be inserted into `source` without ever producing
/// invalid frontmatter: the same insertion compose performs, checked by the
/// same parser. Scan marks parseable identity-less blocks read-only when this
/// fails, before any explicit composition is ever attempted.
pub(crate) fn permits_identity_insertion(source: &str) -> bool {
    let insert_at = match parse_frontmatter(source) {
        Frontmatter::WellFormed {
            insert_at,
            id: BlockId::None,
        } => insert_at,
        _ => return false,
    };
    verified(
        identity_insert(source, insert_at, PROBE_ID, detect_eol(source)),
        PROBE_ID,
    )
    .is_ok()
}

/// New-note artifact contract: UTF-8 without BOM, LF line endings.
fn normalize_new_note(source: &str) -> String {
    let stripped = match source.strip_prefix(BOM) {
        Some(rest) => rest,
        None => source,
    };
    stripped.replace("\r\n", "\n").replace('\r', "\n")
}

/// Insert `note_id` frontmatter identity into `source` for a brand-new note,
/// rejecting foreign, malformed or invalid-reserved frontmatter blocks. The
/// composed artifact carries no BOM and uses LF line endings.
pub(crate) fn compose_for_create(source: &str, note_id: &str) -> SeaRes<String> {
    let normalized = normalize_new_note(source);
    match parse_frontmatter(&normalized) {
        Frontmatter::NoBlock => verified(
            format!("---\nopennote-id: {note_id}\n---\n{normalized}"),
            note_id,
        ),
        Frontmatter::WellFormed { insert_at, id } => match id {
            BlockId::None => verified(
                identity_insert(&normalized, insert_at, note_id, "\n"),
                note_id,
            ),
            BlockId::Valid(_) => Err(errors::err(
                ErrorCode::InvalidInput,
                "error.identityForeign",
                &[],
            )),
            BlockId::Invalid => Err(malformed_error()),
        },
        Frontmatter::Malformed => Err(malformed_error()),
    }
}

/// Save composition: the embedded identity, if any, must match the storage
/// note ID; a missing ID (legacy note) is inserted on this explicit save.
/// Malformed YAML, duplicate mapping keys and an invalid reserved id are
/// rejected; nothing unknown is touched.
///
/// Format policy: the on-disk `reference` text is authoritative for the
/// artifact's UTF-8 BOM presence and its LF/CRLF style, even when the caller
/// supplies LF text without a BOM (editor models normalize CRLF to LF).
/// Against a uniform reference, a uniformly classified caller text is taken
/// as-is and a mixed LF/CRLF caller (bare `\n` alongside `\r\n`, no lone
/// `\r`) is restored to the reference style with deterministic, lossless
/// terminator mapping; a caller containing a lone `\r` is refused there and
/// never restyled. A mixed-terminator reference is accepted only when the
/// caller content is byte-identical after BOM stripping (verbatim
/// pass-through; identity-less slots must refuse because no single
/// terminator exists to insert) and otherwise refused typed. No Unicode
/// normalization, no YAML reserialization.
pub(crate) fn compose_for_save(source: &str, note_id: &str, reference: &str) -> SeaRes<String> {
    let ref_bom = reference.starts_with(BOM);
    let ref_eol = classify_eol(reference);
    let rest = match source.strip_prefix(BOM) {
        Some(stripped) => stripped,
        None => source,
    };

    let aligned: String = if ref_eol == TextEol::Mixed {
        let ref_body = match reference.strip_prefix(BOM) {
            Some(stripped) => stripped,
            None => reference,
        };
        if rest != ref_body {
            return Err(line_endings_unsupported());
        }
        rest.to_owned()
    } else if has_lone_cr(rest) {
        return Err(line_endings_unsupported());
    } else {
        match classify_eol(rest) {
            TextEol::Lf => match ref_eol {
                TextEol::Crlf => rest.replace('\n', "\r\n"),
                _ => rest.to_owned(),
            },
            TextEol::Crlf => match ref_eol {
                TextEol::Lf => rest.replace("\r\n", "\n"),
                _ => rest.to_owned(),
            },
            TextEol::Mixed => match ref_eol {
                TextEol::Crlf => rest.replace("\r\n", "\n").replace('\n', "\r\n"),
                _ => rest.replace("\r\n", "\n"),
            },
        }
    };

    match parse_frontmatter(&aligned) {
        Frontmatter::NoBlock => {
            let eol = eol_string(ref_eol).ok_or_else(line_endings_unsupported)?;
            verified(
                with_reference_bom(
                    ref_bom,
                    format!("---{eol}opennote-id: {note_id}{eol}---{eol}{aligned}"),
                ),
                note_id,
            )
        }
        Frontmatter::WellFormed { insert_at, id } => match id {
            BlockId::Valid(value) => {
                if value != note_id {
                    return Err(errors::err(
                        ErrorCode::InvalidInput,
                        "error.identityMismatch",
                        &[],
                    ));
                }
                Ok(with_reference_bom(ref_bom, aligned))
            }
            BlockId::None => {
                let eol = eol_string(ref_eol).ok_or_else(line_endings_unsupported)?;
                verified(
                    with_reference_bom(ref_bom, identity_insert(&aligned, insert_at, note_id, eol)),
                    note_id,
                )
            }
            BlockId::Invalid => Err(malformed_error()),
        },
        Frontmatter::Malformed => Err(malformed_error()),
    }
}

fn malformed_error() -> ServiceError {
    errors::err(ErrorCode::InvalidInput, "error.frontmatterMalformed", &[])
}

pub(crate) fn compose_for_copy(source: &str, from: &str, to: &str) -> SeaRes<String> {
    use yaml_rust2::scanner::{Scanner, TScalarStyle, TokenType};
    let normalized = normalize_new_note(source);
    let insert_at = match parse_frontmatter(&normalized) {
        Frontmatter::NoBlock
        | Frontmatter::WellFormed {
            id: BlockId::None, ..
        } => return compose_for_create(source, to),
        Frontmatter::WellFormed {
            insert_at,
            id: BlockId::Valid(id),
        } if id == from => insert_at,
        _ => return Err(malformed_error()),
    };
    let mut end = insert_at;
    loop {
        let (line, next) = split_line_at(&normalized, end);
        if is_marker(line) {
            break;
        }
        end = next;
    }
    let block = &normalized[insert_at..end];
    let mut original = YamlLoader::load_from_str(block)
        .map_err(|_| malformed_error())?
        .remove(0);
    let Yaml::Hash(ref mut entries) = original else {
        return Err(malformed_error());
    };
    entries.remove(&Yaml::String(RESERVED_ID.into()));
    for token in Scanner::new(block.chars()) {
        let start = token.0.index();
        let range = match token.1 {
            TokenType::Alias(name) => Some(start..start + name.len() + 1),
            TokenType::Scalar(style, value)
                if uuid::Uuid::parse_str(&value).is_ok_and(|id| id.to_string() == from) =>
            {
                match style {
                    TScalarStyle::SingleQuoted | TScalarStyle::DoubleQuoted => {
                        quoted_end(block, start).map(|end| start..end)
                    }
                    _ => block[start..]
                        .find(&value)
                        .map(|offset| start + offset..start + offset + value.len()),
                }
            }
            _ => None,
        };
        let Some(range) = range else {
            continue;
        };
        if !block.is_char_boundary(range.start) || !block.is_char_boundary(range.end) {
            continue;
        }
        let mut candidate = normalized.clone();
        candidate.replace_range(insert_at + range.start..insert_at + range.end, to);
        if !matches!(parse_frontmatter(&candidate), Frontmatter::WellFormed { id: BlockId::Valid(ref id), .. } if id == to)
        {
            continue;
        }
        let new_end = end - range.len() + to.len();
        let Ok(mut docs) = YamlLoader::load_from_str(&candidate[insert_at..new_end]) else {
            continue;
        };
        let Some(Yaml::Hash(mut copied)) = docs.pop() else {
            continue;
        };
        copied.remove(&Yaml::String(RESERVED_ID.into()));
        // Rebinding a shared anchor must never change unknown properties.
        if Yaml::Hash(copied) == original {
            return verified(candidate, to);
        }
    }
    Err(errors::err(
        ErrorCode::InvalidInput,
        "error.identityCopyUnsupported",
        &[],
    ))
}

fn quoted_end(source: &str, start: usize) -> Option<usize> {
    let quote = *source.as_bytes().get(start)?;
    let mut chars = source[start + 1..].char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if quote == b'"' && ch == '\\' {
            chars.next();
            continue;
        }
        if ch as u32 == quote as u32 {
            if quote == b'\'' && chars.peek().is_some_and(|(_, next)| *next == '\'') {
                chars.next();
                continue;
            }
            return Some(start + 1 + offset + ch.len_utf8());
        }
    }
    None
}
