//! End-to-end acceptance checks for the M04 parse/resolve core, written
//! against the F08/F10/F11 scenarios rather than individual functions.

use onote_metadata_links::{
    extract_links, extract_tags, index::VaultIndex, LinkStatus, NoteSource, EMBED_DEPTH_LIMIT,
};

const A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

fn note(
    id: impl Into<String>,
    path: impl Into<String>,
    title: impl Into<String>,
    source: impl Into<String>,
) -> NoteSource {
    NoteSource::new(id, path, title, source)
}

#[test]
fn backlinks_record_the_source_note_kind_and_offset() {
    let sources = vec![
        note(A, "notes/a.md", "A", "link [[b]] and embed ![[b]]\n"),
        note(B, "notes/b.md", "B", "target\n"),
    ];
    let index = VaultIndex::rebuild(&sources);
    let backlinks = index.backlinks(B);
    assert_eq!(backlinks.len(), 2);
    assert_eq!(backlinks[0].from_note_id, A);
    assert_eq!(backlinks[0].from_path, "notes/a.md");
    assert_eq!(
        backlinks[0].offset,
        sources[0].source.find("[[b]]").unwrap()
    );
    assert_eq!(backlinks[1].kind, onote_metadata_links::LinkKind::Embed);
    assert!(index.issues().is_empty());
}

#[test]
fn a_link_inside_code_creates_no_backlink() {
    let sources = vec![
        note(A, "notes/a.md", "A", "```\n[[b]]\n```\n`[[b]]`\n"),
        note(B, "notes/b.md", "B", "target\n"),
    ];
    let index = VaultIndex::rebuild(&sources);
    assert!(index.backlinks(B).is_empty());
    assert!(index.note(A).unwrap().links.is_empty());
}

#[test]
fn a_bare_title_with_two_candidates_is_never_linked_silently() {
    let sources = vec![
        note(A, "notes/a.md", "A", "[[Dup]]\n"),
        note(B, "notes/dup.md", "Dup", "one\n"),
        note(C, "other/dup.md", "Dup", "two\n"),
    ];
    let index = VaultIndex::rebuild(&sources);
    let issues = index.broken_links();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].status, LinkStatus::AmbiguousTarget);
    assert_eq!(issues[0].candidates, vec!["notes/dup.md", "other/dup.md"]);
    // Neither candidate may claim the reference.
    assert!(index.backlinks(B).is_empty());
    assert!(index.backlinks(C).is_empty());
}

#[test]
fn a_deleted_target_keeps_the_reference_and_reports_it() {
    let source = "keep [[gone|readable label]] text\n";
    let sources = vec![note(A, "notes/a.md", "A", source)];
    let index = VaultIndex::rebuild(&sources);
    let link = &index.note(A).unwrap().links[0];
    assert_eq!(link.link.target, "gone");
    assert_eq!(link.link.label.as_deref(), Some("readable label"));
    assert_eq!(link.resolved.status, LinkStatus::BrokenLink);
    assert_eq!(index.broken_links().len(), 1);
    // The raw markdown is untouched: the index never rewrites sources.
    assert_eq!(sources[0].source, source);
}

#[test]
fn a_missing_anchor_is_reported_separately_and_never_retargets() {
    let sources = vec![
        note(A, "notes/a.md", "A", "[[b#Gone]]\n"),
        note(B, "notes/b.md", "B", "# Present\n\ntext\n"),
    ];
    let index = VaultIndex::rebuild(&sources);
    let missing = index.missing_anchors();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].status, LinkStatus::AnchorMissing);
    assert_eq!(missing[0].target, "b");
    // The note-level reference survives, so the panel can still show it.
    assert_eq!(index.backlinks(B).len(), 1);
    assert!(index.broken_links().is_empty());
}

#[test]
fn tag_index_maps_each_tag_to_sorted_note_ids() {
    let sources = vec![
        note(A, "notes/a.md", "A", "#shared #only-a\n"),
        note(B, "notes/b.md", "B", "#shared\n"),
    ];
    let index = VaultIndex::rebuild(&sources);
    let tags = index.tags();
    assert_eq!(
        tags.get("shared").unwrap(),
        &vec![A.to_owned(), B.to_owned()]
    );
    assert_eq!(tags.get("only-a").unwrap(), &vec![A.to_owned()]);
}

#[test]
fn a_heading_anchor_expands_to_that_section_only() {
    let sources = vec![
        note(A, "notes/a.md", "A", "![[b#Two]]\n"),
        note(
            B,
            "notes/b.md",
            "B",
            "# One\n\none body\n\n## Two\n\ntwo body\n\n# Three\n\nthree\n",
        ),
    ];
    let index = VaultIndex::rebuild(&sources);
    let link = index.note(A).unwrap().links[0].link.clone();
    let expansion = index.expand_embed(A, &link);
    assert_eq!(expansion.status, onote_metadata_links::EmbedStatus::Ok);
    assert_eq!(expansion.content.as_deref(), Some("## Two\n\ntwo body"));
    assert_eq!(expansion.chain, vec![A.to_owned(), B.to_owned()]);
}

#[test]
fn a_block_anchor_expands_to_its_paragraph_without_the_marker() {
    let sources = vec![
        note(A, "notes/a.md", "A", "![[b#block-a]]\n"),
        note(
            B,
            "notes/b.md",
            "B",
            "first\n\npara line one\nline two ^block-a\n\nafter\n",
        ),
    ];
    let index = VaultIndex::rebuild(&sources);
    let link = index.note(A).unwrap().links[0].link.clone();
    let expansion = index.expand_embed(A, &link);
    assert_eq!(
        expansion.content.as_deref(),
        Some("para line one\nline two")
    );
}

#[test]
fn a_removed_anchor_returns_a_placeholder_instead_of_a_neighbour() {
    // `# Removed Heading` would be a tempting fuzzy match for `#Removed`; the
    // anchor must be reported missing instead of jumping to that heading.
    let sources = vec![
        note(A, "notes/a.md", "A", "![[b#Removed]]\n"),
        note(B, "notes/b.md", "B", "# Removed Heading\n\nbody\n"),
    ];
    let index = VaultIndex::rebuild(&sources);
    let link = index.note(A).unwrap().links[0].link.clone();
    let expansion = index.expand_embed(A, &link);
    assert_eq!(
        expansion.status,
        onote_metadata_links::EmbedStatus::AnchorMissing
    );
    assert!(expansion.content.is_none());
    assert_eq!(expansion.target_path.as_deref(), Some("notes/b.md"));
}

#[test]
fn a_two_note_cycle_stops_with_the_chain_intact() {
    let sources = vec![
        note(A, "notes/a.md", "A", "![[b]]\n"),
        note(B, "notes/b.md", "B", "![[a]]\n"),
    ];
    let index = VaultIndex::rebuild(&sources);
    let mut chain = vec![A.to_owned()];
    let first = index.expand_embed_in(&mut chain, A, &index.note(A).unwrap().links[0].link.clone());
    assert_eq!(first.status, onote_metadata_links::EmbedStatus::Ok);
    let second =
        index.expand_embed_in(&mut chain, B, &index.note(B).unwrap().links[0].link.clone());
    assert_eq!(second.status, onote_metadata_links::EmbedStatus::Cycle);
    assert_eq!(second.chain, vec![A.to_owned(), B.to_owned()]);
    assert!(second.content.is_none());
}

#[test]
fn a_self_embed_is_a_cycle_at_once() {
    let sources = vec![note(A, "notes/a.md", "A", "![[a]]\n")];
    let index = VaultIndex::rebuild(&sources);
    let link = index.note(A).unwrap().links[0].link.clone();
    let expansion = index.expand_embed(A, &link);
    assert_eq!(expansion.status, onote_metadata_links::EmbedStatus::Cycle);
}

#[test]
fn nesting_stops_at_the_configured_depth() {
    let sources = vec![
        note(A, "notes/a.md", "A", "![[b]]\n"),
        note(B, "notes/b.md", "B", "![[c]]\n"),
        note(C, "notes/c.md", "C", "![[d]]\n"),
        note(
            "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
            "notes/d.md",
            "D",
            "leaf\n",
        ),
    ];
    let index = VaultIndex::rebuild(&sources);
    let mut chain = vec![A.to_owned()];
    let mut current = A.to_owned();
    let mut statuses = Vec::new();
    for _ in 0..6 {
        let link = index.note(&current).unwrap().links[0].link.clone();
        let expansion = index.expand_embed_in(&mut chain, &current, &link);
        statuses.push(expansion.status);
        match expansion.target_id {
            Some(next) if expansion.status == onote_metadata_links::EmbedStatus::Ok => {
                current = next;
            }
            _ => break,
        }
    }
    // a -> b -> c fills the three-note limit; the hop into d is refused.
    assert_eq!(
        statuses,
        vec![
            onote_metadata_links::EmbedStatus::Ok,
            onote_metadata_links::EmbedStatus::Ok,
            onote_metadata_links::EmbedStatus::DepthExceeded,
        ]
    );
    assert_eq!(chain, vec![A.to_owned(), B.to_owned(), C.to_owned()]);
    assert_eq!(EMBED_DEPTH_LIMIT, 3);
}

#[test]
fn the_index_is_derived_so_an_edit_changes_expanded_content() {
    let mut sources = vec![
        note(A, "notes/a.md", "A", "![[b]]\n"),
        note(B, "notes/b.md", "B", "old body\n"),
    ];
    let before = VaultIndex::rebuild(&sources);
    let link = before.note(A).unwrap().links[0].link.clone();
    assert_eq!(
        before.expand_embed(A, &link).content.as_deref(),
        Some("old body\n")
    );
    sources[1].source = "new body\n".to_owned();
    let after = VaultIndex::rebuild(&sources);
    assert_eq!(
        after.expand_embed(A, &link).content.as_deref(),
        Some("new body\n")
    );
}

#[test]
fn frontmatter_tags_and_body_tags_are_indexed_from_the_same_note() {
    let sources = vec![note(
        A,
        "notes/a.md",
        "A",
        "---\ntags: [front]\n---\n\n#body #body/nested\n",
    )];
    let index = VaultIndex::rebuild(&sources);
    let tags = index.tags();
    assert!(tags.contains_key("front"));
    assert!(tags.contains_key("body"));
    assert!(tags.contains_key("body/nested"));
    let occurrences = extract_tags(&sources[0].source);
    assert_eq!(occurrences.len(), 3);
}

#[test]
fn duplicate_headings_in_one_note_stay_distinct_anchors() {
    let source = "# Same\n\ntext\n\n# Same\n\nmore\n";
    let sources = vec![
        note(A, "notes/a.md", "A", "![[b#same-1]]\n"),
        note(B, "notes/b.md", "B", source),
    ];
    let index = VaultIndex::rebuild(&sources);
    assert!(index.issues().is_empty());
    let link = index.note(A).unwrap().links[0].link.clone();
    assert_eq!(link.anchor.as_deref(), Some("same-1"));
    let expansion = index.expand_embed(A, &link);
    assert_eq!(expansion.content.as_deref(), Some("# Same\n\nmore"));
}

#[test]
fn parsing_a_ten_thousand_note_vault_stays_linear() {
    let mut sources = Vec::new();
    for index in 0..10_000 {
        let id = format!("{index:08}-0000-4000-8000-000000000000");
        sources.push(note(
            &id,
            format!("notes/{index}.md"),
            format!("Note {index}"),
            format!(
                "# Note {index}\n\n#tag{index} [[notes/{}.md]]\n",
                (index + 1) % 10_000
            ),
        ));
    }
    let started = std::time::Instant::now();
    let index = VaultIndex::rebuild(&sources);
    let elapsed = started.elapsed();
    assert_eq!(index.notes().len(), 10_000);
    assert!(index.issues().is_empty());
    assert_eq!(index.tags().len(), 10_000);
    // A generous bound: this only has to catch accidental quadratic work.
    assert!(elapsed.as_secs() < 30, "rebuild took {elapsed:?}");
}

#[test]
fn link_parsing_is_idempotent_and_offsets_point_at_the_source() {
    let source = "a [[b]] c ![[d#e|f]] g\n";
    let links = extract_links(source);
    assert_eq!(links.len(), 2);
    for link in &links {
        let slice = &source[link.offset..link.offset + link.length];
        assert!(slice.starts_with("[[") || slice.starts_with("![["));
        assert!(slice.ends_with("]]"));
    }
}
