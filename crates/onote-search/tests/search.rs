use onote_search::{
    IndexState, SearchError, SearchFilters, SearchIndex, SearchRequest, SearchSource,
    DEFAULT_LIMIT, MAX_LIMIT, MAX_QUERY_CHARS,
};

fn note(id: &str, path: &str, title: &str, source: &str) -> SearchSource {
    SearchSource::new(id, path, title, source)
}

fn index(notes: &[SearchSource]) -> SearchIndex {
    SearchIndex::rebuild(notes, 7, 0)
}

fn ids(page: &onote_search::SearchPage) -> Vec<&str> {
    page.items
        .iter()
        .map(|item| item.note_id.as_str())
        .collect()
}

fn utf16_slice(text: &str, start: usize, end: usize) -> String {
    text.encode_utf16()
        .skip(start)
        .take(end - start)
        .collect::<Vec<u16>>()
        .iter()
        .map(|unit| char::from_u32(u32::from(*unit)).unwrap_or('\u{FFFD}'))
        .collect()
}

#[test]
fn ranks_title_exact_prefix_body_and_breaks_ties_by_id() {
    let notes = [
        note("n4", "notes/body.md", "Other", "alpha body"),
        note("n3", "notes/contains.md", "The Alpha guide", "body"),
        note("n2", "notes/prefix.md", "Alphabet", "body"),
        note("n1", "notes/exact.md", "alpha", "body"),
        note("n9", "notes/exact2.md", "Alpha", "body"),
    ];
    let page = index(&notes).query(&SearchRequest::new("alpha")).unwrap();
    assert_eq!(ids(&page), ["n1", "n9", "n2", "n3", "n4"]);
    assert_eq!(page.items[0].rank, 0);
    assert_eq!(page.items[1].rank, 0);
    assert_eq!(page.items[2].rank, 1);
    assert_eq!(page.items[3].rank, 2);
    assert_eq!(page.items[4].rank, 5);
}

#[test]
fn matches_case_insensitively_and_requires_every_term() {
    let page = index(&[note("n1", "notes/a.md", "Groceries", "Buy milk and bread")])
        .query(&SearchRequest::new("MILK bread"))
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let missed = index(&[note("n1", "notes/a.md", "Groceries", "Buy milk")])
        .query(&SearchRequest::new("milk bread"))
        .unwrap();
    assert!(missed.items.is_empty());
}

#[test]
fn finds_cjk_substrings_without_word_boundaries() {
    let page = index(&[
        note("n1", "notes/a.md", "旅行计划", "明天去京都看红叶"),
        note("n2", "notes/b.md", "other", "no match"),
    ])
    .query(&SearchRequest::new("京都"))
    .unwrap();
    assert_eq!(ids(&page), ["n1"]);
    assert!(page.items[0].snippet.contains("京都"));
    assert!(!page.items[0].highlights.is_empty());
}

#[test]
fn properties_are_searchable_but_never_quoted_as_prose() {
    let source = "---\ntitle: Quarterly review\ntags: [finance]\nteam: platform\n---\n\nNumbers are in the shared sheet.";
    let page = index(&[note("n1", "notes/q.md", "Quarterly review", source)])
        .query(&SearchRequest::new("platform"))
        .unwrap();
    assert_eq!(ids(&page), ["n1"]);
    let item = &page.items[0];
    assert!(
        !item.snippet.contains("team:"),
        "frontmatter must not leak into the sanitized snippet"
    );
    let offset = item.source_offset;
    let units: Vec<u16> = source.encode_utf16().collect();
    let matched: String = units[offset..offset + 8]
        .iter()
        .map(|unit| char::from_u32(u32::from(*unit)).unwrap())
        .collect();
    assert_eq!(matched, "platform");
}

#[test]
fn utf16_offsets_stay_aligned_for_turkish_vietnamese_and_astral_text() {
    let turkish = "Başlık\n\nİstanbul'da düğün var 😀 son";
    let page = index(&[note("n1", "notes/tr.md", "Başlık", turkish)])
        .query(&SearchRequest::new("düğün"))
        .unwrap();
    let item = &page.items[0];
    let highlight = item.highlights[0];
    assert_eq!(
        utf16_slice(&item.snippet, highlight.start, highlight.end),
        "düğün"
    );
    let source_utf16: Vec<u16> = turkish.encode_utf16().collect();
    let offset = item.source_offset;
    let matched: String = source_utf16[offset..offset + 5]
        .iter()
        .map(|unit| char::from_u32(u32::from(*unit)).unwrap())
        .collect();
    assert_eq!(matched, "düğün");

    let astral = "a😀b need";
    let page = index(&[note("n2", "notes/emoji.md", "emoji", astral)])
        .query(&SearchRequest::new("need"))
        .unwrap();
    assert_eq!(page.items[0].source_offset, 5);
    assert_eq!(
        utf16_slice(
            &page.items[0].snippet,
            page.items[0].highlights[0].start,
            page.items[0].highlights[0].end
        ),
        "need"
    );
}

#[test]
fn sanitises_snippets_and_never_returns_markup_or_targets() {
    let source = "---\ntags: [secret]\n---\n\n# Heading\n\nSee [the docs](https://evil.example/x) and <script>alert(1)</script>.\n\n`code` **bold**";
    let page = index(&[note("n1", "notes/a.md", "Heading", source)])
        .query(&SearchRequest::new("docs"))
        .unwrap();
    let item = &page.items[0];
    assert!(item.snippet.contains("the docs"));
    assert!(!item.snippet.contains("https://"));
    assert!(!item.snippet.contains("<script>"));
    assert!(!item.snippet.contains('\n'));
    assert!(!item.snippet.contains('#'));
    assert!(!item.snippet.contains("**"));
    let highlight = item.highlights[0];
    assert_eq!(
        utf16_slice(&item.snippet, highlight.start, highlight.end),
        "docs"
    );
}

#[test]
fn filters_are_or_within_a_class_and_and_across_classes() {
    let notes = [
        note(
            "n1",
            "notes/work/a.md",
            "A",
            "---\ntags: [project]\ntype: task\n---\nalpha",
        ),
        note(
            "n2",
            "notes/work/b.md",
            "B",
            "---\ntags: [project]\ntype: note\n---\nalpha",
        ),
        note(
            "n3",
            "notes/home/c.md",
            "C",
            "---\ntags: [personal]\ntype: task\n---\nalpha",
        ),
    ];
    let mut request = SearchRequest::new("alpha");
    request.filters = SearchFilters {
        folders: vec!["notes/work".into(), "notes/home".into()],
        tags: vec!["#PROJECT".into()],
        types: vec!["TASK".into()],
    };
    let page = index(&notes).query(&request).unwrap();
    assert_eq!(ids(&page), ["n1"]);

    request.filters.folders = vec!["notes/work".into()];
    request.filters.tags.clear();
    request.filters.types.clear();
    assert_eq!(ids(&index(&notes).query(&request).unwrap()), ["n1", "n2"]);

    request.filters.folders.clear();
    request.filters.tags = vec!["missing".into()];
    assert!(index(&notes).query(&request).unwrap().items.is_empty());
}

#[test]
fn pagination_cursor_is_bound_to_the_query_and_index_version() {
    let notes = [
        note("n1", "notes/a.md", "alpha one", "alpha"),
        note("n2", "notes/b.md", "alpha two", "alpha"),
        note("n3", "notes/c.md", "alpha three", "alpha"),
    ];
    let index = index(&notes);
    let mut request = SearchRequest::new("alpha");
    request.limit = Some(2);
    let first = index.query(&request).unwrap();
    assert_eq!(ids(&first), ["n1", "n2"]);
    let cursor = first.next_cursor.expect("more results");

    request.cursor = Some(cursor.clone());
    let second = index.query(&request).unwrap();
    assert_eq!(ids(&second), ["n3"]);
    assert_eq!(second.next_cursor, None);

    request.cursor = Some(cursor.clone());
    request.query = "alpha two".into();
    assert_eq!(index.query(&request), Err(SearchError::InvalidCursor));

    request.query = "alpha".into();
    request.cursor = Some(cursor);
    let rebuilt = SearchIndex::rebuild(&notes, 8, 0);
    assert_eq!(rebuilt.query(&request), Err(SearchError::InvalidCursor));

    for damaged in ["", "v2:7:1:00", "v1:x:1:00", "v1:7:1:zz", "v1:7:1"] {
        request.cursor = Some(damaged.to_owned());
        assert_eq!(index.query(&request), Err(SearchError::InvalidCursor));
    }
}

#[test]
fn validates_query_length_and_page_size() {
    let index = index(&[note("n1", "notes/a.md", "A", "body")]);
    let long = "字".repeat(MAX_QUERY_CHARS + 1);
    assert_eq!(
        index.query(&SearchRequest::new(&long)),
        Err(SearchError::QueryTooLong)
    );
    let longest = "字".repeat(MAX_QUERY_CHARS);
    assert!(index.query(&SearchRequest::new(&longest)).is_ok());

    let mut request = SearchRequest::new("body");
    request.limit = Some(0);
    assert_eq!(index.query(&request), Err(SearchError::InvalidLimit));
    request.limit = Some(MAX_LIMIT + 1);
    assert_eq!(index.query(&request), Err(SearchError::InvalidLimit));
    request.limit = None;
    assert_eq!(
        index.query(&request).unwrap().items.len(),
        1,
        "default limit is {DEFAULT_LIMIT}"
    );
}

#[test]
fn empty_query_returns_no_items_and_no_cursor() {
    let page = index(&[note("n1", "notes/a.md", "A", "body")])
        .query(&SearchRequest::new("   "))
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.index_status.state, IndexState::Ready);
    assert_eq!(page.index_status.indexed_notes, 1);
}

#[test]
fn unavailable_index_is_an_error_not_an_empty_result() {
    let index = SearchIndex::unavailable(3, 2);
    assert_eq!(index.status().state, IndexState::Unavailable);
    assert_eq!(
        index.query(&SearchRequest::new("anything")),
        Err(SearchError::IndexUnavailable)
    );
}

#[test]
fn reports_utf16_source_offsets_after_frontmatter() {
    let source = "---\ntitle: A\n---\n\nalpha body";
    let page = index(&[note("n1", "notes/a.md", "A", source)])
        .query(&SearchRequest::new("body"))
        .unwrap();
    let offset = page.items[0].source_offset;
    let units: Vec<u16> = source.encode_utf16().collect();
    let matched: String = units[offset..offset + 4]
        .iter()
        .map(|unit| char::from_u32(u32::from(*unit)).unwrap())
        .collect();
    assert_eq!(matched, "body");
}

#[test]
fn body_prefilter_preserves_scalar_case_matching_and_original_offsets() {
    let source = "🧪 ΟΣ and Kelvin and İSTANBUL";
    let index = index(&[note("n1", "notes/a.md", "Z", source)]);
    for (query, expected) in [("οσ", "ΟΣ"), ("kelvin", "Kelvin"), ("İstanbul", "İSTANBUL")]
    {
        let page = index.query(&SearchRequest::new(query)).unwrap();
        assert_eq!(page.items.len(), 1, "{query}");
        assert_eq!(page.items[0].rank, 5);
        let start = page.items[0].source_offset;
        assert_eq!(
            utf16_slice(source, start, start + expected.encode_utf16().count()),
            expected
        );
        let highlight = page.items[0].highlights.first().unwrap();
        assert_eq!(
            utf16_slice(&page.items[0].snippet, highlight.start, highlight.end),
            expected
        );
    }
    for query in ["ος", "istanbul", "i\u{0307}stanbul", "absent-token"] {
        assert!(
            index
                .query(&SearchRequest::new(query))
                .unwrap()
                .items
                .is_empty(),
            "{query}"
        );
    }
}

#[test]
fn controlled_rebuild_never_returns_a_partial_index_and_keeps_normal_semantics() {
    let notes = [
        note("a", "a.md", "A", "alpha"),
        note("b", "b.md", "B", "alpha beta"),
    ];
    for stop in [0, 1, 2] {
        let mut seen = Vec::new();
        let result = SearchIndex::rebuild_controlled(&notes, 9, 0, |done, total| {
            seen.push((done, total));
            done < stop
        });
        assert!(matches!(result, Err(SearchError::Cancelled)));
        assert_eq!(seen.last(), Some(&(stop, 2)));
    }
    let actual = SearchIndex::rebuild_controlled(&notes, 9, 0, |_, _| true).unwrap();
    assert_eq!(
        actual.query(&SearchRequest::new("alpha")),
        SearchIndex::rebuild(&notes, 9, 0).query(&SearchRequest::new("alpha"))
    );
}
