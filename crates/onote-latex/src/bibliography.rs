//! M13 BibTeX/Biber bibliography handling (F49).
//!
//! A `.bib` file is authoritative and M13 only derives an index from it, so the
//! parser must be **lossless about what it does not understand**: an unknown entry
//! type or an unparsable block keeps its original text and is reported with a
//! position, never dropped (F49-R01).
//!
//! Two further rules:
//!
//! * a duplicate citation key is a conflict shown with both sources, never a
//!   silent "take the first one" (DUPLICATE_CITE_KEY);
//! * the backend is part of the answer: BibTeX and Biber handle Unicode
//!   differently, so a citation is checked against the backend that will run
//!   (F49-R04).
//!
//! Nothing here writes a file, and nothing goes near the network (F49-R03).

use serde::{Deserialize, Serialize};

/// Longest accepted citation key (§4).
pub const MAX_CITE_KEY_CHARS: usize = 128;

/// Which tool will process the bibliography.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Backend {
    BibTeX,
    Biber,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BibTeX => "bibtex",
            Self::Biber => "biber",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bibtex" => Some(Self::BibTeX),
            "biber" => Some(Self::Biber),
            _ => None,
        }
    }
}

/// One field of an entry, in source order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibField {
    pub name: String,
    pub value: String,
}

/// A parsed entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibEntry {
    pub cite_key: String,
    /// Lower-cased entry type, e.g. `article`. Unknown types keep their name.
    pub entry_type: String,
    pub fields: Vec<BibField>,
    /// True when the type is not one this build recognises; the entry is still
    /// fully available and its source is preserved (F49-R01).
    pub unknown_type: bool,
    /// Byte offset of the entry's `@` in the source.
    pub offset: usize,
    /// The entry text exactly as written.
    pub raw: String,
}

impl BibEntry {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|field| field.name.eq_ignore_ascii_case(name))
            .map(|field| field.value.as_str())
    }

    pub fn author(&self) -> Option<&str> {
        self.field("author")
    }

    pub fn title(&self) -> Option<&str> {
        self.field("title")
    }

    pub fn year(&self) -> Option<&str> {
        self.field("year")
    }
}

/// A block the parser could not turn into an entry; its text is kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibError {
    pub offset: usize,
    pub raw: String,
    /// A message key the UI resolves, never an English sentence.
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bibliography {
    pub entries: Vec<BibEntry>,
    /// Unparsable blocks, kept so nothing is lost (F49-R01).
    pub errors: Vec<BibError>,
    /// Citation keys that appear more than once, with every source offset.
    pub duplicates: Vec<DuplicateKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateKey {
    pub cite_key: String,
    /// Offsets of every entry claiming this key, in source order.
    pub offsets: Vec<usize>,
}

impl Bibliography {
    pub fn entry(&self, cite_key: &str) -> Option<&BibEntry> {
        self.entries.iter().find(|entry| entry.cite_key == cite_key)
    }

    /// True when a key is used more than once, so the caller must not pick one.
    pub fn is_duplicate(&self, cite_key: &str) -> bool {
        self.duplicates
            .iter()
            .any(|entry| entry.cite_key == cite_key)
    }

    /// Keys that may be offered as unambiguous choices.
    pub fn unique_keys(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| !self.is_duplicate(&entry.cite_key))
            .map(|entry| entry.cite_key.as_str())
            .collect()
    }
}

/// Entry types this build recognises.
const KNOWN_TYPES: &[&str] = &[
    "article",
    "book",
    "booklet",
    "conference",
    "inbook",
    "incollection",
    "inproceedings",
    "manual",
    "mastersthesis",
    "misc",
    "phdthesis",
    "proceedings",
    "techreport",
    "unpublished",
    "online",
    "patent",
    "periodical",
    "standard",
];

/// Finds the matching `}` for the `{` at `open`, skipping quoted braces.
fn matching_close(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i64;
    let mut index = open;
    let mut in_quotes = false;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => in_quotes = !in_quotes,
            b'{' if !in_quotes => depth += 1,
            b'}' if !in_quotes => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            b'@' if !in_quotes => {}
            _ => {}
        }
        index += 1;
    }
    None
}

/// Parses the fields of an entry body.
///
/// Values keep their braces and quoting as written, because a `.bib` value may
/// contain protected capitals or a macro call and rewriting it would change what
/// the bibliography prints.
fn parse_fields(body: &str) -> Vec<BibField> {
    let mut fields: Vec<BibField> = Vec::new();
    let bytes = body.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        // Skip separators and whitespace.
        while index < bytes.len() && matches!(bytes[index], b',' | b' ' | b'\t' | b'\r' | b'\n') {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'_'
                || bytes[index] == b'-')
        {
            index += 1;
        }
        if index == name_start {
            // Nothing that looks like a name here; skip one byte and continue so a
            // malformed field cannot loop forever.
            index += 1;
            continue;
        }
        let name = body[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            continue;
        }
        index += 1;
        while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b'\r' | b'\n') {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let value_start;
        let value_end;
        if bytes[index] == b'{' {
            let Some(close) = matching_close(body, index) else {
                // An unbalanced value: keep what is left as the value so the text
                // is not silently truncated.
                value_start = index;
                value_end = body.len();
                fields.push(BibField {
                    name,
                    value: body[value_start..value_end].to_owned(),
                });
                break;
            };
            value_start = index + 1;
            value_end = close;
            index = close + 1;
        } else if bytes[index] == b'"' {
            let Some(close) = body[index + 1..].find('"') else {
                value_start = index;
                value_end = body.len();
                fields.push(BibField {
                    name,
                    value: body[value_start..value_end].to_owned(),
                });
                break;
            };
            value_start = index + 1;
            value_end = index + 1 + close;
            index = index + 1 + close + 1;
        } else {
            // A bare value ends at the next comma or closing brace.
            value_start = index;
            while index < bytes.len() && bytes[index] != b',' && bytes[index] != b'}' {
                index += 1;
            }
            value_end = index;
        }
        fields.push(BibField {
            name,
            value: body[value_start..value_end].trim().to_owned(),
        });
    }
    fields
}

/// Parses a `.bib` file.
///
/// Unrecognised or unbalanced entries are reported with their offset and kept
/// verbatim, so a rebuild never loses a field the parser did not model.
pub fn parse(source: &str) -> Bibliography {
    let mut entries: Vec<BibEntry> = Vec::new();
    let mut errors: Vec<BibError> = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'@' {
            index += 1;
            continue;
        }
        let offset = index;
        index += 1;
        while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
            index += 1;
        }
        let type_start = index;
        while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
            index += 1;
        }
        let entry_type = source[type_start..index].to_ascii_lowercase();
        if entry_type.is_empty() {
            errors.push(BibError {
                offset,
                raw: source[offset..(offset + 1).min(source.len())].to_owned(),
                message_key: "error.bibEntryMalformed".to_owned(),
            });
            continue;
        }
        // `@comment` and `@preamble` are not entries but must still be preserved in
        // the source; they are skipped without being reported as errors.
        while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b'\r' | b'\n') {
            index += 1;
        }
        if index >= bytes.len() || (bytes[index] != b'{' && bytes[index] != b'(') {
            errors.push(BibError {
                offset,
                raw: source[offset..index.min(source.len())].to_owned(),
                message_key: "error.bibEntryMalformed".to_owned(),
            });
            continue;
        }
        let open = index;
        let Some(close) = matching_close(source, open) else {
            // An unterminated entry: keep the rest of the file as the failed block
            // so the user can see what is broken.
            errors.push(BibError {
                offset,
                raw: source[offset..].to_owned(),
                message_key: "error.bibEntryUnterminated".to_owned(),
            });
            break;
        };
        let body = &source[open + 1..close];
        let raw = source[offset..=close].to_owned();
        index = close + 1;
        if entry_type == "comment" || entry_type == "preamble" || entry_type == "string" {
            continue;
        }
        // The cite key is the text before the first comma.
        let Some(comma) = body.find(',') else {
            errors.push(BibError {
                offset,
                raw,
                message_key: "error.bibEntryNoKey".to_owned(),
            });
            continue;
        };
        let cite_key = body[..comma].trim().to_owned();
        if cite_key.is_empty() {
            errors.push(BibError {
                offset,
                raw,
                message_key: "error.bibEntryNoKey".to_owned(),
            });
            continue;
        }
        entries.push(BibEntry {
            cite_key,
            unknown_type: !KNOWN_TYPES.contains(&entry_type.as_str()),
            entry_type,
            fields: parse_fields(&body[comma + 1..]),
            offset,
            raw,
        });
    }

    // Duplicates are collected with every offset so the UI can show both sources.
    // Collect every offset per key once. An earlier version both accumulated on a
    // repeat and stored the full list on first sight, so a duplicated key reported
    // three offsets for two entries.
    let mut by_key: Vec<DuplicateKey> = Vec::new();
    for entry in &entries {
        match by_key
            .iter_mut()
            .find(|duplicate| duplicate.cite_key == entry.cite_key)
        {
            Some(existing) => existing.offsets.push(entry.offset),
            None => by_key.push(DuplicateKey {
                cite_key: entry.cite_key.clone(),
                offsets: vec![entry.offset],
            }),
        }
    }
    let mut duplicates: Vec<DuplicateKey> = by_key
        .into_iter()
        .filter(|entry| entry.offsets.len() > 1)
        .collect();
    duplicates.sort_by(|left, right| left.offsets.cmp(&right.offsets));

    Bibliography {
        entries,
        errors,
        duplicates,
    }
}

/// Whether a citation key is acceptable (§4).
pub fn valid_cite_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().count() <= MAX_CITE_KEY_CHARS
        && !key.chars().any(|character| character.is_whitespace() || character.is_control())
        // BibTeX treats these as syntax inside a key.
        && !key.chars().any(|character| matches!(character, '{' | '}' | ',' | '\\'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CitationIssue {
    UnknownKey(String),
    /// The key exists more than once, so citing it is ambiguous
    /// (DUPLICATE_CITE_KEY).
    DuplicateKey {
        cite_key: String,
        offsets: Vec<usize>,
    },
    InvalidKey(String),
    NoKeys,
}

/// A plan for inserting one `\cite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationPlan {
    /// The command text to insert, or empty when an issue blocks it.
    pub fragment: String,
    pub issues: Vec<CitationIssue>,
    /// True when the project already loads a bibliography package.
    pub has_bibliography_package: bool,
    /// The backend the build profile selected; reported so a mismatch is visible.
    pub backend: Backend,
}

/// Plans inserting `\cite{...}` for the given keys.
///
/// A duplicate key is refused with both offsets rather than resolved silently, and
/// an unknown key is refused because a citation to nothing prints `[?]` in the
/// PDF while looking successful.
pub fn plan_citation(
    bibliography: &Bibliography,
    cite_keys: &[String],
    backend: Backend,
    preamble: &str,
) -> CitationPlan {
    let mut issues: Vec<CitationIssue> = Vec::new();
    if cite_keys.is_empty() {
        issues.push(CitationIssue::NoKeys);
    }
    for key in cite_keys {
        if !valid_cite_key(key) {
            issues.push(CitationIssue::InvalidKey(key.clone()));
            continue;
        }
        if let Some(duplicate) = bibliography
            .duplicates
            .iter()
            .find(|duplicate| duplicate.cite_key == *key)
        {
            // DUPLICATE_CITE_KEY: report the conflict with every source offset.
            issues.push(CitationIssue::DuplicateKey {
                cite_key: key.clone(),
                offsets: duplicate.offsets.clone(),
            });
            continue;
        }
        if bibliography.entry(key).is_none() {
            issues.push(CitationIssue::UnknownKey(key.clone()));
        }
    }
    let has_bibliography_package =
        preamble.contains("\\bibliography{") || preamble.contains("\\addbibresource");
    CitationPlan {
        fragment: if issues.is_empty() {
            format!("\\cite{{{}}}\n", cite_keys.join(","))
        } else {
            String::new()
        },
        issues,
        has_bibliography_package,
        backend,
    }
}

/// How many entries hold characters a BibTeX run cannot handle safely.
///
/// BibTeX is 7-bit oriented: a Unicode author with the BibTeX backend needs the
/// project to declare an input encoding or switch to Biber, so this counts them
/// rather than letting the build fail later (F49-R04).
pub fn unicode_entries(bibliography: &Bibliography, backend: Backend) -> Vec<&BibEntry> {
    if backend == Backend::Biber {
        // Biber handles UTF-8 natively.
        return Vec::new();
    }
    bibliography
        .entries
        .iter()
        .filter(|entry| entry.fields.iter().any(|field| !field.value.is_ascii()))
        .collect()
}

/// A multi-file rename plan for a citation key (F49-R02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePlan {
    pub old_key: String,
    pub new_key: String,
    /// Number of `\cite` occurrences that would change.
    pub citations: usize,
    /// Files the plan touches; empty when the new key is invalid.
    pub issues: Vec<CitationIssue>,
}

/// Plans a citation-key rename across the given file texts.
///
/// The caller supplies each file's revision; a mismatch is refused so a rename is
/// never applied to a file that changed since it was read (F49-R02).
pub fn plan_rename(
    bibliography: &Bibliography,
    sources: &[(String, String)],
    old_key: &str,
    new_key: &str,
    expected_revisions: &[(String, u64)],
    actual_revisions: &[(String, u64)],
) -> RenamePlan {
    let mut issues: Vec<CitationIssue> = Vec::new();
    if !valid_cite_key(new_key) {
        issues.push(CitationIssue::InvalidKey(new_key.to_owned()));
    }
    if bibliography.entry(old_key).is_none() {
        issues.push(CitationIssue::UnknownKey(old_key.to_owned()));
    }
    if bibliography.entry(new_key).is_some() {
        // Renaming onto an existing key would merge two entries silently.
        issues.push(CitationIssue::DuplicateKey {
            cite_key: new_key.to_owned(),
            offsets: bibliography
                .entry(new_key)
                .map(|entry| vec![entry.offset])
                .unwrap_or_default(),
        });
    }
    for (path, expected) in expected_revisions {
        match actual_revisions
            .iter()
            .find(|(name, _)| name == path)
            .map(|(_, revision)| *revision)
        {
            Some(actual) if actual == *expected => {}
            _ => {
                // A changed file means the plan is stale for that file.
                issues.push(CitationIssue::UnknownKey(format!("{path}@stale")));
            }
        }
    }
    let citations = sources
        .iter()
        .map(|(_, text)| count_citations(text, old_key))
        .sum();
    RenamePlan {
        old_key: old_key.to_owned(),
        new_key: new_key.to_owned(),
        citations,
        issues,
    }
}

/// Counts `\cite`-family occurrences of one key in a text.
fn count_citations(text: &str, key: &str) -> usize {
    let mut count = 0usize;
    for command in [
        "\\cite{",
        "\\citep{",
        "\\citet{",
        "\\parencite{",
        "\\textcite{",
        "\\autocite{",
    ] {
        let mut rest = text;
        while let Some(start) = rest.find(command) {
            let after = &rest[start + command.len()..];
            let Some(end) = after.find('}') else {
                break;
            };
            let keys: Vec<&str> = after[..end].split(',').map(str::trim).collect();
            if keys.iter().any(|candidate| *candidate == key) {
                count += 1;
            }
            rest = &after[end..];
        }
    }
    count
}

/// Citation keys used in the given sources that the bibliography does not define.
///
/// F49-A02: deleting a cited entry must report the missing citations. The `\cite`
/// text is never removed from the source, so this only reports.
pub fn dangling_citations(bibliography: &Bibliography, sources: &[String]) -> Vec<String> {
    let mut referenced: Vec<String> = Vec::new();
    for text in sources {
        for command in [
            "\\cite{",
            "\\citep{",
            "\\citet{",
            "\\parencite{",
            "\\textcite{",
            "\\autocite{",
        ] {
            let mut rest = text.as_str();
            while let Some(start) = rest.find(command) {
                let after = &rest[start + command.len()..];
                let Some(end) = after.find('}') else {
                    break;
                };
                for key in after[..end].split(',') {
                    let key = key.trim();
                    if !key.is_empty() && !referenced.iter().any(|existing| existing == key) {
                        referenced.push(key.to_owned());
                    }
                }
                rest = &after[end..];
            }
        }
    }
    let mut dangling: Vec<String> = referenced
        .into_iter()
        .filter(|key| bibliography.entry(key).is_none())
        .collect();
    dangling.sort();
    dangling
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = "@article{smith2020,\n  author = {Smith, John},\n  title = {A {G}reat Paper},\n  year = {2020},\n}\n\
@book{doe2019,\n  author = {Doe, Jane},\n  title = {Book},\n}\n";

    #[test]
    fn entries_parse_with_their_fields_in_source_order() {
        let bibliography = parse(BIB);
        assert_eq!(bibliography.entries.len(), 2);
        let article = bibliography.entry("smith2020").expect("article");
        assert_eq!(article.entry_type, "article");
        assert_eq!(article.author(), Some("Smith, John"));
        assert_eq!(article.year(), Some("2020"));
        // Protected capitals keep their braces: rewriting them would change output.
        assert_eq!(article.title(), Some("A {G}reat Paper"));
        assert!(!article.unknown_type);
    }

    #[test]
    fn an_unknown_entry_type_is_kept_and_flagged() {
        // F49-R01: unknown types are preserved, not deleted.
        let bibliography = parse("@dataset{x1,\n  title = {Data},\n}\n");
        let entry = bibliography.entry("x1").expect("entry");
        assert!(entry.unknown_type);
        assert_eq!(entry.title(), Some("Data"));
        assert!(entry.raw.contains("@dataset"));
    }

    #[test]
    fn an_unterminated_entry_is_reported_with_its_text() {
        let bibliography = parse("@article{broken,\n  title = {No end\n");
        assert!(bibliography.entries.is_empty());
        assert_eq!(bibliography.errors.len(), 1);
        assert!(bibliography.errors[0].raw.contains("@article{broken"));
        assert_eq!(
            bibliography.errors[0].message_key,
            "error.bibEntryUnterminated"
        );
    }

    #[test]
    fn an_entry_without_a_key_is_reported() {
        let bibliography = parse("@article{\n}\n");
        assert!(bibliography.entries.is_empty());
        assert!(bibliography
            .errors
            .iter()
            .any(|error| error.message_key.contains("NoKey")));
    }

    #[test]
    fn comment_and_preamble_blocks_are_skipped_not_reported() {
        let bibliography = parse("@comment{ignore me}\n@article{a,\n title={T},\n}\n");
        assert_eq!(bibliography.entries.len(), 1);
        assert!(bibliography.errors.is_empty());
    }

    #[test]
    fn a_duplicate_key_is_reported_with_every_offset() {
        // DUPLICATE_CITE_KEY: never silently take the first entry.
        let source = "@article{dup,\n title={A},\n}\n@book{dup,\n title={B},\n}\n";
        let bibliography = parse(source);
        assert_eq!(bibliography.entries.len(), 2);
        assert_eq!(bibliography.duplicates.len(), 1);
        assert_eq!(bibliography.duplicates[0].offsets.len(), 2);
        assert!(bibliography.is_duplicate("dup"));
        assert!(bibliography.unique_keys().is_empty());
    }

    #[test]
    fn a_quoted_value_keeps_its_content() {
        let bibliography = parse("@article{a,\n title = \"Quoted title\",\n}\n");
        assert_eq!(
            bibliography.entry("a").and_then(|entry| entry.title()),
            Some("Quoted title")
        );
    }

    #[test]
    fn a_bare_value_ends_at_the_comma() {
        let bibliography = parse("@article{a,\n year = 2020,\n}\n");
        assert_eq!(
            bibliography.entry("a").and_then(|entry| entry.year()),
            Some("2020")
        );
    }

    #[test]
    fn braces_inside_a_value_do_not_end_the_entry() {
        let bibliography = parse("@article{a,\n title = {Nested {Brace} Here},\n year = {1},\n}\n");
        let entry = bibliography.entry("a").expect("entry");
        assert_eq!(entry.title(), Some("Nested {Brace} Here"));
        assert_eq!(entry.year(), Some("1"));
    }

    #[test]
    fn citation_keys_are_validated_for_the_documented_rules() {
        assert!(valid_cite_key("smith2020"));
        assert!(valid_cite_key("smith:2020-a"));
        assert!(!valid_cite_key(""));
        assert!(!valid_cite_key("has space"));
        assert!(!valid_cite_key("has{brace"));
        assert!(!valid_cite_key("has,comma"));
        assert!(!valid_cite_key(&"k".repeat(129)));
    }

    #[test]
    fn a_citation_plan_inserts_one_command_for_several_keys() {
        let bibliography = parse(BIB);
        let plan = plan_citation(
            &bibliography,
            &["smith2020".to_owned(), "doe2019".to_owned()],
            Backend::BibTeX,
            "\\bibliography{refs}",
        );
        assert!(plan.issues.is_empty());
        assert_eq!(plan.fragment, "\\cite{smith2020,doe2019}\n");
        assert!(plan.has_bibliography_package);
        assert_eq!(plan.backend, Backend::BibTeX);
    }

    #[test]
    fn a_citation_to_an_unknown_key_is_refused() {
        let bibliography = parse(BIB);
        let plan = plan_citation(&bibliography, &["nope".to_owned()], Backend::BibTeX, "");
        assert!(plan.fragment.is_empty());
        assert!(plan
            .issues
            .contains(&CitationIssue::UnknownKey("nope".to_owned())));
        // Without a bibliography command the build would not resolve anything.
        assert!(!plan.has_bibliography_package);
    }

    #[test]
    fn a_citation_to_a_duplicate_key_reports_both_offsets() {
        let bibliography = parse("@article{dup,\n title={A},\n}\n@book{dup,\n title={B},\n}\n");
        let plan = plan_citation(&bibliography, &["dup".to_owned()], Backend::Biber, "");
        assert!(plan.fragment.is_empty());
        match plan.issues.first() {
            Some(CitationIssue::DuplicateKey { offsets, .. }) => assert_eq!(offsets.len(), 2),
            other => panic!("expected a duplicate issue, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_citation_list_is_refused() {
        let bibliography = parse(BIB);
        let plan = plan_citation(&bibliography, &[], Backend::BibTeX, "");
        assert_eq!(plan.issues, vec![CitationIssue::NoKeys]);
    }

    #[test]
    fn unicode_fields_are_flagged_for_bibtex_but_not_for_biber() {
        // F49-R04: the backend decides whether Unicode is safe.
        let source = "@article{cn,\n author = {张三},\n title = {标题},\n}\n";
        let bibliography = parse(source);
        assert_eq!(unicode_entries(&bibliography, Backend::BibTeX).len(), 1);
        assert!(unicode_entries(&bibliography, Backend::Biber).is_empty());
    }

    #[test]
    fn a_rename_plan_counts_citations_and_refuses_a_stale_file() {
        // F49-R02: a rename is a multi-file plan with version checks.
        let bibliography = parse(BIB);
        let sources = vec![
            (
                "main.tex".to_owned(),
                "\\cite{smith2020} and \\citep{smith2020}".to_owned(),
            ),
            ("other.tex".to_owned(), "\\cite{doe2019}".to_owned()),
        ];
        let plan = plan_rename(
            &bibliography,
            &sources,
            "smith2020",
            "smith2021",
            &[("main.tex".to_owned(), 3)],
            &[("main.tex".to_owned(), 3)],
        );
        assert!(plan.issues.is_empty());
        assert_eq!(plan.citations, 2);

        // A file that changed since it was read blocks the rename.
        let stale = plan_rename(
            &bibliography,
            &sources,
            "smith2020",
            "smith2021",
            &[("main.tex".to_owned(), 3)],
            &[("main.tex".to_owned(), 4)],
        );
        assert!(!stale.issues.is_empty());
    }

    #[test]
    fn a_rename_onto_an_existing_key_is_refused() {
        let bibliography = parse(BIB);
        let plan = plan_rename(&bibliography, &[], "smith2020", "doe2019", &[], &[]);
        // Merging two entries silently would lose a source.
        assert!(!plan.issues.is_empty());
    }

    #[test]
    fn an_invalid_new_key_is_refused() {
        let bibliography = parse(BIB);
        let plan = plan_rename(&bibliography, &[], "smith2020", "has space", &[], &[]);
        assert!(plan
            .issues
            .iter()
            .any(|issue| matches!(issue, CitationIssue::InvalidKey(_))));
    }

    #[test]
    fn backends_round_trip_and_an_unknown_one_is_rejected() {
        assert_eq!(Backend::parse("bibtex"), Some(Backend::BibTeX));
        assert_eq!(Backend::parse("biber"), Some(Backend::Biber));
        assert_eq!(Backend::parse("biblatex"), None);
    }

    #[test]
    fn an_empty_bibliography_parses_to_nothing() {
        let bibliography = parse("");
        assert!(bibliography.entries.is_empty());
        assert!(bibliography.errors.is_empty());
        assert!(bibliography.duplicates.is_empty());
    }
}
