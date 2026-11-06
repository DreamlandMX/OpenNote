//! M05 search: the rebuildable in-memory index behind F12.
//!
//! The index is a derived artifact. [`SearchIndex::rebuild`] parses each note
//! once and [`SearchIndex::query`] never touches the file system, so the host
//! can keep a warm index for one vault and invalidate it when the note set
//! changes. The host persists validated sources in `.opennote/index.sqlite`;
//! this crate owns deterministic query semantics and does no filesystem IO.
//!
//! Offsets are UTF-16 code units in the original source, which is exactly what
//! the JavaScript editor uses for selection and highlighting. That keeps
//! Turkish, Vietnamese and astral characters from shifting a highlight by one
//! byte (F12-A02).

use onote_metadata_links::{extract_tags, parse_properties, PropertyValue, Scanner};
use serde::{Deserialize, Serialize};
use std::fmt;

/// F12 field limit: at most 500 characters, counted as Unicode scalars.
pub const MAX_QUERY_CHARS: usize = 500;
/// F12 default page size.
pub const DEFAULT_LIMIT: usize = 50;
/// F12 maximum page size.
pub const MAX_LIMIT: usize = 200;
/// Characters of context returned around the first body match.
const SNIPPET_CHARS: usize = 180;
const CURSOR_VERSION: &str = "v1";

/// One note as the index consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSource {
    pub note_id: String,
    pub relative_path: String,
    pub title: String,
    pub source: String,
}

impl SearchSource {
    pub fn new(
        note_id: impl Into<String>,
        relative_path: impl Into<String>,
        title: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            note_id: note_id.into(),
            relative_path: relative_path.into(),
            title: title.into(),
            source: source.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexState {
    Ready,
    /// The host could not build an index; a zero-hit page would be a lie.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub state: IndexState,
    pub version: u64,
    pub indexed_notes: usize,
    pub unreadable_notes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_persisted: Option<bool>,
}

/// F12 filters: OR within one class, AND across classes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchFilters {
    pub folders: Vec<String>,
    pub tags: Vec<String>,
    /// Frontmatter `type` values. A note without one matches no type filter.
    pub types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub filters: SearchFilters,
    /// Reserved for locale-sensitive folding. Matching is Unicode default
    /// case-folding today, so the field is accepted without changing results.
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

impl SearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            filters: SearchFilters::default(),
            locale: None,
            limit: None,
            cursor: None,
        }
    }
}

/// A highlight range inside `SearchItem::snippet`, in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub note_id: String,
    pub relative_path: String,
    pub title: String,
    pub snippet: String,
    pub highlights: Vec<Highlight>,
    /// UTF-16 offset in the full note source, for jumping to the first hit.
    pub source_offset: usize,
    /// 0 exact title, 1 title prefix, 2 title contains, 3 all terms in title,
    /// 4 some term in title, 5 body/tag/type only.
    pub rank: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<SearchItem>,
    pub next_cursor: Option<String>,
    pub index_status: IndexStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    Cancelled,
    IndexUnavailable,
    QueryTooLong,
    InvalidLimit,
    InvalidCursor,
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::Cancelled => formatter.write_str("search cancelled"),
            SearchError::IndexUnavailable => formatter.write_str("search index unavailable"),
            SearchError::QueryTooLong => formatter.write_str("search query too long"),
            SearchError::InvalidLimit => formatter.write_str("search limit out of range"),
            SearchError::InvalidCursor => formatter.write_str("search cursor invalid"),
        }
    }
}

impl std::error::Error for SearchError {}

#[derive(Debug, Clone)]
struct IndexedNote {
    note_id: String,
    relative_path: String,
    title: String,
    tags: Vec<String>,
    note_type: Option<String>,
    /// Body without frontmatter, exactly as written, for offsets and matching.
    body: String,
    /// Conservative prefilter only; scalar matching still verifies Unicode boundaries.
    folded_body: String,
    /// Frontmatter YAML, so F12's "properties" are searchable too. It is never
    /// used for snippets because raw YAML is not user-facing prose.
    metadata_text: String,
    /// UTF-16 offset of `metadata_text` in the full source.
    metadata_start_utf16: usize,
    /// UTF-16 offset of `body` in the full source, including any BOM.
    body_start_utf16: usize,
}

#[derive(Debug, Clone)]
pub struct SearchIndex {
    notes: Vec<IndexedNote>,
    status: IndexStatus,
}

impl SearchIndex {
    /// Builds the index from the current sources. `unreadable` lets the UI say
    /// that the index is incomplete instead of presenting zero hits as final.
    pub fn rebuild(sources: &[SearchSource], version: u64, unreadable: usize) -> Self {
        Self::rebuild_controlled(sources, version, unreadable, |_, _| true)
            .expect("unconditional rebuild cannot be cancelled")
    }

    pub fn rebuild_controlled(
        sources: &[SearchSource],
        version: u64,
        unreadable: usize,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<Self, SearchError> {
        let notes = sources
            .iter()
            .enumerate()
            .map(|(done, source)| {
                if !progress(done, sources.len()) {
                    return Err(SearchError::Cancelled);
                }
                let scanner = Scanner::new(&source.source);
                let body_start = scanner.body_start();
                let body = source.source[body_start..].to_owned();
                let folded_body = body.chars().flat_map(char::to_lowercase).collect();
                let metadata_text = scanner.frontmatter_body().unwrap_or_default().to_owned();
                let metadata_start_utf16 = source
                    .source
                    .find(&metadata_text)
                    .map(|start| utf16_len(&source.source[..start]))
                    .unwrap_or(0);
                let tags = extract_tags(&source.source)
                    .into_iter()
                    .map(|tag| tag.tag.to_lowercase())
                    .collect();
                Ok(IndexedNote {
                    note_id: source.note_id.clone(),
                    relative_path: source.relative_path.clone(),
                    title: source.title.clone(),
                    tags,
                    note_type: note_type_of(&source.source),
                    body,
                    folded_body,
                    metadata_text,
                    metadata_start_utf16,
                    body_start_utf16: utf16_len(&source.source[..body_start]),
                })
            })
            .collect::<Result<Vec<_>, SearchError>>()?;
        if !progress(sources.len(), sources.len()) {
            return Err(SearchError::Cancelled);
        }
        Ok(Self {
            status: IndexStatus {
                state: IndexState::Ready,
                version,
                indexed_notes: notes.len(),
                unreadable_notes: unreadable,
                cache_persisted: None,
            },
            notes,
        })
    }

    /// A host-side build failure. Queries return [`SearchError::IndexUnavailable`]
    /// so the UI offers a rebuild instead of rendering an empty result set.
    pub fn unavailable(version: u64, unreadable: usize) -> Self {
        Self {
            notes: Vec::new(),
            status: IndexStatus {
                state: IndexState::Unavailable,
                version,
                indexed_notes: 0,
                unreadable_notes: unreadable,
                cache_persisted: None,
            },
        }
    }

    pub fn status(&self) -> &IndexStatus {
        &self.status
    }

    pub fn set_cache_persisted(&mut self, persisted: bool) {
        self.status.cache_persisted = Some(persisted);
    }

    pub fn query(&self, request: &SearchRequest) -> Result<SearchPage, SearchError> {
        if self.status.state == IndexState::Unavailable {
            return Err(SearchError::IndexUnavailable);
        }
        if request.query.chars().count() > MAX_QUERY_CHARS {
            return Err(SearchError::QueryTooLong);
        }
        let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(SearchError::InvalidLimit);
        }
        let query = request.query.trim();
        let cursor = match request.cursor.as_deref() {
            Some(raw) => Some(parse_cursor(raw)?),
            None => None,
        };
        if let Some(cursor) = &cursor {
            if cursor.version != self.status.version
                || cursor.key != query_key(query, &request.filters)
            {
                return Err(SearchError::InvalidCursor);
            }
        }
        // F12: an empty query shows recent notes and must not scan the vault.
        if query.is_empty() {
            return Ok(SearchPage {
                items: Vec::new(),
                next_cursor: None,
                index_status: self.status.clone(),
            });
        }
        let terms: Vec<&str> = query.split_whitespace().collect();
        let folded_terms: Vec<String> = terms
            .iter()
            .map(|term| term.chars().flat_map(char::to_lowercase).collect())
            .collect();
        let key = query_key(query, &request.filters);
        let mut ranked: Vec<(u8, &IndexedNote)> = Vec::new();
        for note in &self.notes {
            if !matches_filters(note, &request.filters) {
                continue;
            }
            let Some(rank) = rank_note(note, query, &terms, &folded_terms) else {
                continue;
            };
            ranked.push((rank, note));
        }
        ranked.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.note_id.cmp(&right.1.note_id))
        });
        let offset = cursor.map_or(0, |cursor| cursor.offset);
        let page: Vec<SearchItem> = ranked
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(rank, note)| item_for(note, &terms, *rank))
            .collect();
        let next = offset + page.len();
        let next_cursor = if next < ranked.len() {
            Some(format_cursor(self.status.version, next, key))
        } else {
            None
        };
        Ok(SearchPage {
            items: page,
            next_cursor,
            index_status: self.status.clone(),
        })
    }
}

fn note_type_of(source: &str) -> Option<String> {
    let properties = parse_properties(source, &[]).ok()?;
    let property = properties.get("type")?;
    let value = match &property.value {
        PropertyValue::Text(text)
        | PropertyValue::Select(text)
        | PropertyValue::Link(text)
        | PropertyValue::Date(text)
        | PropertyValue::DateTime(text) => text.clone(),
        PropertyValue::MultiSelect(values) => values.first().cloned()?,
        PropertyValue::Number(number) => number.to_string(),
        PropertyValue::Boolean(flag) => flag.to_string(),
        PropertyValue::Unset => return None,
    };
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_lowercase())
    }
}

fn normalize_folder(folder: &str) -> String {
    folder
        .trim()
        .trim_matches(['/', '\\'])
        .replace('\\', "/")
        .to_lowercase()
}

fn normalize_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('#').to_lowercase()
}

fn matches_filters(note: &IndexedNote, filters: &SearchFilters) -> bool {
    let folders: Vec<String> = filters
        .folders
        .iter()
        .map(|folder| normalize_folder(folder))
        .filter(|folder| !folder.is_empty())
        .collect();
    if !folders.is_empty() {
        let path = note.relative_path.replace('\\', "/").to_lowercase();
        if !folders
            .iter()
            .any(|folder| path.starts_with(&format!("{folder}/")))
        {
            return false;
        }
    }
    let tags: Vec<String> = filters
        .tags
        .iter()
        .map(|tag| normalize_tag(tag))
        .filter(|tag| !tag.is_empty())
        .collect();
    if !tags.is_empty() && !tags.iter().any(|tag| note.tags.contains(tag)) {
        return false;
    }
    let types: Vec<String> = filters
        .types
        .iter()
        .map(|note_type| note_type.trim().to_lowercase())
        .filter(|note_type| !note_type.is_empty())
        .collect();
    if !types.is_empty() {
        let Some(note_type) = &note.note_type else {
            return false;
        };
        if !types.iter().any(|wanted| note_type == wanted) {
            return false;
        }
    }
    true
}

fn rank_note(
    note: &IndexedNote,
    query: &str,
    terms: &[&str],
    folded_terms: &[String],
) -> Option<u8> {
    let title = note.title.as_str();
    let all_terms_somewhere = terms.iter().zip(folded_terms).all(|(term, folded)| {
        contains_fold(title, term)
            || note.tags.iter().any(|tag| contains_fold(tag, term))
            || note
                .note_type
                .as_deref()
                .is_some_and(|note_type| contains_fold(note_type, term))
            || contains_fold(&note.metadata_text, term)
            || (note.folded_body.contains(folded) && contains_fold(&note.body, term))
    });
    if !all_terms_somewhere {
        return None;
    }
    let trimmed = title.trim();
    if eq_fold(trimmed, query) {
        return Some(0);
    }
    if starts_with_fold(trimmed, query) {
        return Some(1);
    }
    if contains_fold(trimmed, query) {
        return Some(2);
    }
    if terms.iter().all(|term| contains_fold(trimmed, term)) {
        return Some(3);
    }
    if terms.iter().any(|term| contains_fold(trimmed, term)) {
        return Some(4);
    }
    Some(5)
}

fn item_for(note: &IndexedNote, terms: &[&str], rank: u8) -> SearchItem {
    let raw_body = note.body.as_str();
    let body_term = terms
        .iter()
        .find(|term| contains_fold(raw_body, term))
        .copied();
    let metadata_term = terms
        .iter()
        .find(|term| contains_fold(&note.metadata_text, term))
        .copied();
    // Prefer a real body hit for the jump; a property-only hit points at the
    // frontmatter value so the camera still lands on matched text. `None` is
    // the real "not found" state: a hit at source offset 0 is legitimate.
    let source_offset = body_term
        .and_then(|term| find_fold_span(raw_body, term))
        .map(|(start, _)| note.body_start_utf16 + utf16_len(&raw_body[..start]))
        .or_else(|| {
            metadata_term.and_then(|term| {
                find_fold_span(&note.metadata_text, term).map(|(start, _)| {
                    note.metadata_start_utf16 + utf16_len(&note.metadata_text[..start])
                })
            })
        })
        .unwrap_or(0);
    // Sanitising only the page (at most 200 notes) keeps the index small while
    // the snippet still never shows raw HTML or a link target.
    let sanitized = sanitize_markdown(raw_body);
    let match_byte = body_term
        .or(metadata_term)
        .and_then(|term| find_fold_span(&sanitized, term));
    let characters: Vec<char> = sanitized.chars().collect();
    let match_char = match_byte.map_or(0, |(start, _)| sanitized[..start].chars().count());
    let start = match_char.saturating_sub(SNIPPET_CHARS / 3);
    let end = (start + SNIPPET_CHARS).min(characters.len());
    let start = end.saturating_sub(SNIPPET_CHARS);
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.extend(characters[start..end].iter());
    if end < characters.len() {
        snippet.push('…');
    }
    let snippet = snippet.trim().to_owned();
    let highlights = highlights_for(&snippet, terms);
    SearchItem {
        note_id: note.note_id.clone(),
        relative_path: note.relative_path.clone(),
        title: note.title.clone(),
        snippet,
        highlights,
        source_offset,
        rank,
    }
}

fn highlights_for(snippet: &str, terms: &[&str]) -> Vec<Highlight> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        if term.is_empty() {
            continue;
        }
        let mut search_from = 0;
        while search_from < snippet.len() {
            let Some((relative_start, relative_end)) =
                find_fold_span(&snippet[search_from..], term)
            else {
                break;
            };
            let start = search_from + relative_start;
            let end = search_from + relative_end;
            ranges.push((utf16_len(&snippet[..start]), utf16_len(&snippet[..end])));
            search_from = end.max(start + 1);
            if ranges.len() >= 100 {
                break;
            }
        }
    }
    ranges.sort();
    let mut merged: Vec<Highlight> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(previous) if start <= previous.end => {
                previous.end = previous.end.max(end);
            }
            _ => merged.push(Highlight { start, end }),
        }
    }
    merged
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn contains_fold(haystack: &str, needle: &str) -> bool {
    find_fold_span(haystack, needle).is_some()
}

/// Finds `needle` case-insensitively and returns its byte range in `haystack`.
fn find_fold_span(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return Some((0, 0));
    }
    for (start, _) in haystack.char_indices() {
        if let Some(end) = match_prefix(&haystack[start..], needle) {
            return Some((start, start + end));
        }
    }
    None
}

/// If `needle` matches a prefix of `haystack`, returns how many bytes of
/// `haystack` the match consumed.
fn match_prefix(haystack: &str, needle: &str) -> Option<usize> {
    let mut haystack = haystack.chars();
    let mut needle = needle.chars();
    let mut consumed = 0;
    loop {
        match (haystack.next(), needle.next()) {
            (_, None) => return Some(consumed),
            (None, Some(_)) => return None,
            (Some(left), Some(right)) => {
                if !char_eq(left, right) {
                    return None;
                }
                consumed += left.len_utf8();
            }
        }
    }
}

fn starts_with_fold(haystack: &str, needle: &str) -> bool {
    match_prefix(haystack, needle).is_some()
}

fn eq_fold(left: &str, right: &str) -> bool {
    let mut left = left.chars();
    let mut right = right.chars();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if char_eq(a, b) => {}
            _ => return false,
        }
    }
}

fn char_eq(left: char, right: char) -> bool {
    if left == right {
        return true;
    }
    if left.is_ascii() && right.is_ascii() {
        // Most text is ASCII; this avoids two ToLowercase iterators per
        // character compared during a 10k-note scan.
        return left.eq_ignore_ascii_case(&right);
    }
    let mut left = left.to_lowercase();
    let mut right = right.to_lowercase();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if a == b => {}
            _ => return false,
        }
    }
}

/// Strips the syntax that would otherwise appear in a search snippet while
/// keeping the user's words. It is deliberately conservative: no HTML, no link
/// targets, no code fences, no line noise. The editor highlight still points
/// at the untouched source through `source_offset`.
fn sanitize_markdown(source: &str) -> String {
    let characters: Vec<char> = source.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    let mut line_start = true;
    while index < characters.len() {
        let current = characters[index];
        if current == '\n' || current == '\r' {
            output.push(' ');
            line_start = true;
            index += 1;
            continue;
        }
        if line_start {
            if current == ' ' || current == '\t' {
                index += 1;
                continue;
            }
            if current == '#' {
                while index < characters.len() && characters[index] == '#' {
                    index += 1;
                }
                continue;
            }
            if current == '>' || current == '|' {
                index += 1;
                continue;
            }
            if matches!(current, '-' | '+' | '*') && next_is_space(&characters, index + 1) {
                index += 2;
                continue;
            }
            if current.is_ascii_digit() {
                let mut probe = index;
                while probe < characters.len() && characters[probe].is_ascii_digit() {
                    probe += 1;
                }
                if probe > index
                    && probe < characters.len()
                    && matches!(characters[probe], '.' | ')')
                    && next_is_space(&characters, probe + 1)
                {
                    index = probe + 2;
                    continue;
                }
            }
            line_start = false;
        }
        if current == '!' && characters.get(index + 1) == Some(&'[') {
            if let Some((label, next)) = parse_link(&characters, index + 1) {
                push_label(&mut output, &label);
                index = next;
                continue;
            }
        }
        if current == '[' {
            if let Some((label, next)) = parse_link(&characters, index) {
                push_label(&mut output, &label);
                index = next;
                continue;
            }
        }
        if current == '<' && looks_like_tag(&characters, index) {
            while index < characters.len() && characters[index] != '>' {
                index += 1;
            }
            index = (index + 1).min(characters.len());
            continue;
        }
        if matches!(current, '`' | '*' | '_' | '~') {
            index += 1;
            continue;
        }
        output.push(current);
        index += 1;
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn next_is_space(characters: &[char], index: usize) -> bool {
    characters
        .get(index)
        .is_some_and(|value| *value == ' ' || *value == '\t')
}

fn looks_like_tag(characters: &[char], index: usize) -> bool {
    characters
        .get(index + 1)
        .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?'))
}

/// Parses `[label]` or `[label](target)` starting at the opening bracket.
fn parse_link(characters: &[char], open: usize) -> Option<(String, usize)> {
    let mut index = open + 1;
    let mut label = String::new();
    while index < characters.len() {
        match characters[index] {
            ']' => break,
            '[' => return None,
            value => label.push(value),
        }
        index += 1;
    }
    if index >= characters.len() {
        return None;
    }
    index += 1;
    if characters.get(index) == Some(&'(') {
        let mut depth = 1;
        index += 1;
        while index < characters.len() && depth > 0 {
            match characters[index] {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            index += 1;
        }
    }
    Some((label, index))
}

fn push_label(output: &mut String, label: &str) {
    for character in label.chars() {
        if !matches!(character, '`' | '*' | '_' | '~' | '[' | ']') {
            output.push(character);
        }
    }
}

fn query_key(query: &str, filters: &SearchFilters) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut feed = |text: &str| {
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0x1f;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    feed(query);
    for folder in &filters.folders {
        feed(folder);
    }
    for tag in &filters.tags {
        feed(tag);
    }
    for note_type in &filters.types {
        feed(note_type);
    }
    hash
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    version: u64,
    offset: usize,
    key: u64,
}

fn format_cursor(version: u64, offset: usize, key: u64) -> String {
    format!("{CURSOR_VERSION}:{version}:{offset}:{key:016x}")
}

fn parse_cursor(raw: &str) -> Result<Cursor, SearchError> {
    let mut parts = raw.split(':');
    let version = parts.next();
    let index_version = parts.next();
    let offset = parts.next();
    let key = parts.next();
    if version != Some(CURSOR_VERSION) || parts.next().is_some() {
        return Err(SearchError::InvalidCursor);
    }
    let index_version = index_version
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(SearchError::InvalidCursor)?;
    let offset = offset
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(SearchError::InvalidCursor)?;
    let key = key
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or(SearchError::InvalidCursor)?;
    Ok(Cursor {
        version: index_version,
        offset,
        key,
    })
}
