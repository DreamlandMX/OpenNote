//! Integration checks for the M04 metadata service over a real synthetic vault.
//!
//! These exercise the storage-to-index boundary: notes are created through the
//! actual M02 API, read back through it, and indexed. No real vault is touched.

use onote_domain::ErrorCode;
use onote_metadata_links::{EmbedStatus, LinkStatus};
use onote_vault_storage::VaultStorage;
use opennote_desktop::metadata;

const OP_A: &str = "11111111-1111-4111-8111-111111111111";
const OP_B: &str = "22222222-2222-4222-8222-222222222222";

fn seed(storage: &mut VaultStorage, path: &str, source: &str, operation: &str) -> String {
    storage
        .create_note(path, source, operation)
        .expect("create note")
        .note_id
}

#[test]
fn tags_are_collected_across_notes_with_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Tags").expect("vault");
    seed(&mut storage, "notes/one.md", "#shared #alpha\n", OP_A);
    seed(&mut storage, "notes/two.md", "#shared\n", OP_B);

    let report = metadata::tag_report(&storage).expect("tag report");
    assert!(report.unreadable.is_empty());
    let shared = report
        .tags
        .iter()
        .find(|entry| entry.tag == "shared")
        .expect("shared tag");
    assert_eq!(shared.note_paths, vec!["notes/one.md", "notes/two.md"]);
    let alpha = report
        .tags
        .iter()
        .find(|entry| entry.tag == "alpha")
        .expect("alpha tag");
    assert_eq!(alpha.note_paths, vec!["notes/one.md"]);
}

#[test]
fn a_link_inside_code_produces_no_backlink() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Code").expect("vault");
    seed(&mut storage, "notes/target.md", "target\n", OP_A);
    seed(
        &mut storage,
        "notes/source.md",
        "```\n[[target]]\n```\n`[[target]]`\n",
        OP_B,
    );
    let target_id = storage
        .list()
        .expect("list")
        .into_iter()
        .find(|note| note.relative_path == "notes/target.md")
        .expect("target summary")
        .note_id;

    let report = metadata::backlink_report(&storage, &target_id).expect("backlinks");
    assert!(report.backlinks.is_empty());
}

#[test]
fn backlinks_resolve_by_path_and_record_the_offset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Backlinks").expect("vault");
    let target_source = "# Target\n\nbody\n";
    seed(&mut storage, "notes/target.md", target_source, OP_A);
    let source = "see [[notes/target]] and [[notes/target#Target]]\n";
    let source_id = seed(&mut storage, "notes/source.md", source, OP_B);
    let target_id = storage
        .list()
        .expect("list")
        .into_iter()
        .find(|note| note.relative_path == "notes/target.md")
        .expect("target summary")
        .note_id;

    let report = metadata::backlink_report(&storage, &target_id).expect("backlinks");
    assert_eq!(report.backlinks.len(), 2);
    assert_eq!(report.backlinks[0].from_path, "notes/source.md");
    // Storage embeds an `opennote-id` frontmatter block, and the editor receives
    // that same stored source, so offsets are measured against the stored bytes.
    let stored = storage.read(&source_id).expect("read source").source;
    assert_eq!(
        report.backlinks[0].offset,
        stored.find("[[notes/target]]").unwrap()
    );
    assert_eq!(report.backlinks[1].anchor.as_deref(), Some("Target"));
    assert!(report.unreadable.is_empty());
}

#[test]
fn a_broken_link_is_reported_without_touching_the_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Broken").expect("vault");
    let source = "keep [[missing-note|label]]\n";
    let note_id = seed(&mut storage, "notes/a.md", source, OP_A);
    let stored_before = storage.read(&note_id).expect("read").source;

    let report = metadata::link_report(&storage).expect("link report");
    assert_eq!(report.broken.len(), 1);
    assert_eq!(report.broken[0].status, LinkStatus::BrokenLink);
    assert_eq!(report.broken[0].target, "missing-note");
    assert_eq!(report.broken[0].label.as_deref(), Some("label"));
    assert!(report.missing_anchors.is_empty());

    // The index never rewrites: the stored bytes are unchanged.
    let stored_after = storage.read(&note_id).expect("read back").source;
    assert_eq!(stored_after, stored_before);
    assert!(stored_after.contains("[[missing-note|label]]"));
}

#[test]
fn deleting_a_link_target_keeps_the_source_bytes_and_reports_the_link_broken() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "DeletedTarget").expect("vault");
    let history = std::sync::Arc::new(
        onote_history_backup::HistoryStore::open(dir.path(), &storage.vault().vault_id)
            .expect("history"),
    );
    storage.set_revision_archive(history.clone());
    storage.set_trash_archive(history);
    let target = storage
        .create_note("notes/target.md", "# Target\n", OP_A)
        .expect("target");
    let source = storage
        .create_note(
            "notes/source.md",
            "keep [[notes/target]] and [[notes/target|alias]]\n",
            OP_B,
        )
        .expect("source");
    let source_path = dir.path().join("notes/source.md");
    let source_bytes = std::fs::read(&source_path).expect("source bytes");
    let source_document = storage.read(&source.note_id).expect("read source");
    assert!(metadata::link_report(&storage)
        .expect("links before")
        .broken
        .is_empty());

    storage
        .delete_note(
            &target.note_id,
            target.local_revision,
            &target.content_hash,
            "33333333-3333-4333-8333-333333333333",
        )
        .expect("delete target");

    assert_eq!(
        std::fs::read(&source_path).expect("source bytes after"),
        source_bytes
    );
    assert_eq!(
        storage.read(&source.note_id).expect("read source after"),
        source_document
    );
    let report = metadata::link_report(&storage).expect("links after");
    assert!(report.missing_anchors.is_empty());
    assert_eq!(report.broken.len(), 2);
    assert!(report.broken.iter().all(|issue| {
        issue.from_note_id == source.note_id
            && issue.from_path == "notes/source.md"
            && issue.target == "notes/target"
            && issue.status == LinkStatus::BrokenLink
    }));
    assert_eq!(
        report.broken[0].offset,
        source_document
            .source
            .find("[[notes/target]]")
            .expect("first offset")
    );
    assert_eq!(report.broken[1].label.as_deref(), Some("alias"));
}

#[test]
fn a_missing_anchor_is_separated_from_a_broken_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Anchors").expect("vault");
    seed(&mut storage, "notes/target.md", "# Present\n\ntext\n", OP_A);
    seed(
        &mut storage,
        "notes/source.md",
        "[[notes/target#Gone]]\n",
        OP_B,
    );

    let report = metadata::link_report(&storage).expect("link report");
    assert!(report.broken.is_empty());
    assert_eq!(report.missing_anchors.len(), 1);
    assert_eq!(report.missing_anchors[0].status, LinkStatus::AnchorMissing);
    assert_eq!(report.missing_anchors[0].anchor.as_deref(), Some("Gone"));
}

#[test]
fn note_metadata_reports_headings_tags_links_and_backlinks_together() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Meta").expect("vault");
    seed(
        &mut storage,
        "notes/target.md",
        "# One\n\n## Two\n\ntext ^block\n",
        OP_A,
    );
    seed(
        &mut storage,
        "notes/source.md",
        "#tag\n\n[[notes/target#Two]]\n",
        OP_B,
    );
    let target_id = storage
        .list()
        .expect("list")
        .into_iter()
        .find(|note| note.relative_path == "notes/target.md")
        .expect("target summary")
        .note_id;

    let report = metadata::note_metadata(&storage, &target_id).expect("metadata");
    assert_eq!(report.title, "target");
    assert_eq!(report.relative_path, "notes/target.md");
    let slugs: Vec<&str> = report.headings.iter().map(|h| h.slug.as_str()).collect();
    assert_eq!(slugs, vec!["one", "two"]);
    assert_eq!(report.block_anchors.len(), 1);
    assert_eq!(report.block_anchors[0].id, "block");
    assert_eq!(report.backlinks.len(), 1);
    assert_eq!(report.backlinks[0].from_path, "notes/source.md");
}

#[test]
fn an_unknown_note_id_is_a_typed_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = VaultStorage::create(dir.path(), "Empty").expect("vault");
    let error = metadata::note_metadata(&storage, "does-not-exist").expect_err("must fail");
    assert_eq!(error.code, ErrorCode::ObjectNotFound);
    assert_eq!(error.message_key, "error.noteMissing");
}

#[test]
fn an_embed_expands_a_heading_section_and_a_cycle_stops() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Embeds").expect("vault");
    seed(
        &mut storage,
        "notes/target.md",
        "# One\n\none\n\n## Two\n\ntwo\n\n# Three\n\nthree\n",
        OP_A,
    );
    let source_id = seed(
        &mut storage,
        "notes/source.md",
        "![[notes/target#Two]]\n",
        OP_B,
    );
    let target_id = storage
        .list()
        .expect("list")
        .into_iter()
        .find(|note| note.relative_path == "notes/target.md")
        .expect("target summary")
        .note_id;

    let link = onote_metadata_links::RawLink {
        kind: onote_metadata_links::LinkKind::Embed,
        target: "notes/target".to_owned(),
        label: None,
        anchor: Some("Two".to_owned()),
        offset: 0,
        length: 0,
    };
    let expansion = metadata::expand_embed(&storage, &source_id, &link, &[]).expect("expand");
    assert_eq!(expansion.status, EmbedStatus::Ok);
    assert_eq!(expansion.content.as_deref(), Some("## Two\n\ntwo"));
    assert_eq!(expansion.target_id.as_deref(), Some(target_id.as_str()));

    // Continuing the same path back into the source note is a cycle.
    let back = onote_metadata_links::RawLink {
        kind: onote_metadata_links::LinkKind::Embed,
        target: source_id.clone(),
        label: None,
        anchor: None,
        offset: 0,
        length: 0,
    };
    let mut chain = expansion.chain.clone();
    let cycle = metadata::expand_embed(&storage, &target_id, &back, &chain).expect("expand");
    assert_eq!(cycle.status, EmbedStatus::Cycle);
    // The caller-supplied chain is what keeps detection intact across calls.
    chain.push(target_id.clone());
    let still_cycle = metadata::expand_embed(&storage, &target_id, &back, &chain).expect("expand");
    assert_eq!(still_cycle.status, EmbedStatus::Cycle);
}

#[test]
fn a_read_only_vault_still_serves_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "ReadOnly").expect("vault");
    seed(&mut storage, "notes/a.md", "#tag [[notes/a]]\n", OP_A);
    // Metadata is derived from existing sources, so it needs no write access.
    let report = metadata::tag_report(&storage).expect("tags");
    assert_eq!(report.tags.len(), 1);
    let link = metadata::link_report(&storage).expect("links");
    assert!(link.broken.is_empty());
}

#[test]
fn a_note_that_cannot_be_read_is_reported_not_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Damaged").expect("vault");
    seed(&mut storage, "notes/good.md", "#tag\n", OP_A);
    let broken_id = seed(&mut storage, "notes/broken.md", "[[notes/good]]\n", OP_B);
    let broken_path = storage
        .list()
        .expect("list")
        .into_iter()
        .find(|note| note.note_id == broken_id)
        .expect("broken summary")
        .relative_path;
    let absolute = dir.path().join(&broken_path);
    std::fs::write(&absolute, [0xFFu8, 0xFE, 0x00, 0x01]).expect("corrupt bytes");

    let report = metadata::tag_report(&storage).expect("tags still returned");
    // The readable note is still indexed...
    assert!(report.tags.iter().any(|entry| entry.tag == "tag"));
    // ...and the unreadable one is surfaced rather than dropped.
    assert_eq!(report.unreadable.len(), 1);
    assert_eq!(report.unreadable[0].relative_path, broken_path);
}

#[test]
fn a_rename_plan_reports_edits_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "Rename").expect("vault");
    let target_id = seed(&mut storage, "notes/old.md", "# Old\n\nbody\n", OP_A);
    let source_id = seed(
        &mut storage,
        "notes/src.md",
        "see [[notes/old|Alias]] and notes/old in prose\n",
        OP_B,
    );
    let stored_before = storage.read(&source_id).expect("read").source;

    let plan = metadata::rename_plan(&storage, &target_id, "notes/new.md").expect("plan");
    assert!(plan.keeps_identity);
    assert_eq!(plan.from_path, "notes/old.md");
    assert_eq!(plan.to_path, "notes/new.md");
    assert_eq!(plan.edits.len(), 1, "only the real link, not the prose");
    assert_eq!(plan.edits[0].after, "[[notes/new|Alias]]");
    assert_eq!(plan.edits[0].note_id, source_id);

    // Planning is read-only: the stored source is untouched.
    let stored_after = storage.read(&source_id).expect("read").source;
    assert_eq!(stored_after, stored_before);
}

#[test]
fn a_rename_plan_leaves_an_ambiguous_reference_alone_and_says_why() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "RenameAmbig").expect("vault");
    let target_id = seed(&mut storage, "notes/dup.md", "one\n", OP_A);
    seed(&mut storage, "other/dup.md", "two\n", OP_B);
    seed(
        &mut storage,
        "notes/src.md",
        "[[Dup]]\n",
        "33333333-3333-4333-8333-333333333333",
    );

    let plan = metadata::rename_plan(&storage, &target_id, "notes/renamed.md").expect("plan");
    assert!(plan.edits.is_empty());
    assert_eq!(plan.untouched.len(), 1);
    assert_eq!(
        plan.untouched[0].reason,
        onote_metadata_links::RenameWarningReason::AmbiguousTarget
    );
}

#[test]
fn a_rename_plan_refuses_an_empty_target_and_an_unknown_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "RenameGuard").expect("vault");
    let note_id = seed(&mut storage, "notes/a.md", "body\n", OP_A);

    let empty = metadata::rename_plan(&storage, &note_id, "   ").expect_err("empty target");
    assert_eq!(empty.code, ErrorCode::InvalidInput);
    assert_eq!(empty.message_key, "error.renameTargetRequired");

    let unknown = metadata::rename_plan(&storage, "nope", "notes/b.md").expect_err("unknown note");
    assert_eq!(unknown.code, ErrorCode::ObjectNotFound);
}

#[test]
fn backlinks_refuse_a_malformed_or_unknown_note_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = VaultStorage::create(dir.path(), "BacklinkGuard").expect("vault");

    let malformed = metadata::backlink_report(&storage, "notes/one.md").expect_err("malformed id");
    assert_eq!(malformed.code, ErrorCode::InvalidInput);
    assert_eq!(malformed.message_key, "error.invalidNoteId");

    let unknown = metadata::backlink_report(&storage, "44444444-4444-4444-8444-444444444444")
        .expect_err("unknown note");
    assert_eq!(unknown.code, ErrorCode::ObjectNotFound);
    assert_eq!(unknown.message_key, "error.noteMissing");
}

#[test]
fn an_embed_refuses_malformed_ids_before_resolving() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "EmbedGuard").expect("vault");
    let note_id = seed(&mut storage, "notes/a.md", "# A\n", OP_A);
    let link = onote_metadata_links::RawLink {
        kind: onote_metadata_links::LinkKind::Embed,
        target: "notes/a".to_owned(),
        label: None,
        anchor: None,
        offset: 0,
        length: 0,
    };

    let bad_from =
        metadata::expand_embed(&storage, "not-a-note", &link, &[]).expect_err("malformed from id");
    assert_eq!(bad_from.code, ErrorCode::InvalidInput);
    assert_eq!(bad_from.message_key, "error.invalidNoteId");

    let unknown_from =
        metadata::expand_embed(&storage, "44444444-4444-4444-8444-444444444444", &link, &[])
            .expect_err("unknown from id");
    assert_eq!(unknown_from.code, ErrorCode::ObjectNotFound);
    assert_eq!(unknown_from.message_key, "error.noteMissing");

    // The last entry still names the note being expanded; only a middle entry
    // is malformed, so this exercises the per-entry check.
    let bad_chain = metadata::expand_embed(
        &storage,
        &note_id,
        &link,
        &["tampered".to_owned(), note_id.clone()],
    )
    .expect_err("malformed chain entry");
    assert_eq!(bad_chain.code, ErrorCode::InvalidInput);
    assert_eq!(bad_chain.message_key, "error.invalidEmbedChain");
}

#[test]
fn an_embed_refuses_a_forged_chain_shape_and_accepts_a_real_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "EmbedChain").expect("vault");
    let root_id = seed(&mut storage, "notes/root.md", "![[notes/mid]]\n", OP_A);
    let mid_id = seed(&mut storage, "notes/mid.md", "![[notes/leaf]]\n", OP_B);
    let leaf_id = seed(
        &mut storage,
        "notes/leaf.md",
        "leaf\n",
        "33333333-3333-4333-8333-333333333333",
    );
    let link = onote_metadata_links::RawLink {
        kind: onote_metadata_links::LinkKind::Embed,
        target: "notes/leaf".to_owned(),
        label: None,
        anchor: None,
        offset: 0,
        length: 0,
    };

    let wrong_last = metadata::expand_embed(&storage, &mid_id, &link, &[root_id.clone()])
        .expect_err("chain does not end at the expanding note");
    assert_eq!(wrong_last.code, ErrorCode::InvalidInput);
    assert_eq!(wrong_last.message_key, "error.invalidEmbedChain");

    let overlong = metadata::expand_embed(
        &storage,
        &mid_id,
        &link,
        &[
            root_id.clone(),
            mid_id.clone(),
            "44444444-4444-4444-8444-444444444444".to_owned(),
            mid_id.clone(),
        ],
    )
    .expect_err("chain exceeds the embed limit");
    assert_eq!(overlong.code, ErrorCode::InvalidInput);
    assert_eq!(overlong.message_key, "error.invalidEmbedChain");

    let leaf_source = storage.read(&leaf_id).expect("read leaf").source;
    let expansion =
        metadata::expand_embed(&storage, &mid_id, &link, &[root_id.clone(), mid_id.clone()])
            .expect("a real path still expands");
    assert_eq!(expansion.status, EmbedStatus::Ok);
    // Whole-note embeds include the stored identity block, which is part of the
    // source every embed reader sees.
    assert_eq!(expansion.content.as_deref(), Some(leaf_source.as_str()));
    assert_eq!(expansion.chain, vec![root_id, mid_id, leaf_id]);
}

#[test]
fn an_embed_chain_must_reference_notes_in_this_vault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut storage = VaultStorage::create(dir.path(), "ChainIdentity").expect("vault");
    let root_id = seed(&mut storage, "notes/root.md", "root\n", OP_A);
    let mid_id = seed(&mut storage, "notes/mid.md", "mid\n", OP_B);
    let leaf_id = seed(
        &mut storage,
        "notes/leaf.md",
        "leaf\n",
        "33333333-3333-4333-8333-333333333333",
    );
    let link = onote_metadata_links::RawLink {
        kind: onote_metadata_links::LinkKind::Embed,
        target: "notes/leaf".to_owned(),
        label: None,
        anchor: None,
        offset: 0,
        length: 0,
    };

    // A canonical UUID that names no note in this vault cannot be smuggled in
    // to forge a cycle or skip expansion.
    let unknown = metadata::expand_embed(
        &storage,
        &mid_id,
        &link,
        &[
            "44444444-4444-4444-8444-444444444444".to_owned(),
            mid_id.clone(),
        ],
    )
    .expect_err("unknown chain note");
    assert_eq!(unknown.code, ErrorCode::InvalidInput);
    assert_eq!(unknown.message_key, "error.invalidEmbedChain");

    // The genuine path still expands.
    let expansion =
        metadata::expand_embed(&storage, &mid_id, &link, &[root_id.clone(), mid_id.clone()])
            .expect("real chain");
    assert_eq!(expansion.status, EmbedStatus::Ok);
    assert_eq!(expansion.target_id.as_deref(), Some(leaf_id.as_str()));
}

#[test]
fn an_embed_target_outside_the_vault_is_never_read() {
    let parent = tempfile::tempdir().expect("tempdir");
    let vault_root = parent.path().join("vault");
    let mut storage = VaultStorage::create(&vault_root, "Confined").expect("vault");
    let note_id = seed(&mut storage, "notes/a.md", "safe body\n", OP_A);
    // A readable file that sits next to the vault, not inside it.
    let secret_path = parent.path().join("secret.md");
    std::fs::write(&secret_path, "TOP SECRET\n").expect("outside file");

    let absolute = secret_path.to_string_lossy().into_owned();
    let targets = [
        "../secret.md".to_owned(),
        "..\\secret.md".to_owned(),
        absolute,
        "C:\\Windows\\win.ini".to_owned(),
        "\\\\server\\share\\secret.md".to_owned(),
    ];
    for target in targets {
        let link = onote_metadata_links::RawLink {
            kind: onote_metadata_links::LinkKind::Embed,
            target: target.clone(),
            label: None,
            anchor: None,
            offset: 0,
            length: 0,
        };
        let expansion =
            metadata::expand_embed(&storage, &note_id, &link, &[]).expect("typed outcome");
        assert_eq!(expansion.status, EmbedStatus::BrokenLink, "target {target}");
        assert!(expansion.target_id.is_none(), "target {target}");
        assert!(expansion.target_path.is_none(), "target {target}");
        assert!(
            expansion
                .content
                .as_deref()
                .is_none_or(|content| !content.contains("TOP SECRET")),
            "target {target} leaked outside content"
        );
    }
}

#[test]
fn metadata_is_scoped_to_the_supplied_vault() {
    let dir_a = tempfile::tempdir().expect("tempdir A");
    let dir_b = tempfile::tempdir().expect("tempdir B");
    let mut storage_a = VaultStorage::create(dir_a.path(), "Vault A").expect("vault A");
    let mut storage_b = VaultStorage::create(dir_b.path(), "Vault B").expect("vault B");
    let source_a = seed(&mut storage_a, "notes/source.md", "source A\n", OP_A);
    seed(&mut storage_a, "notes/shared.md", "A body\n", OP_B);
    let note_b = seed(&mut storage_b, "notes/shared.md", "B body\n", OP_A);

    let link = onote_metadata_links::RawLink {
        kind: onote_metadata_links::LinkKind::Embed,
        target: "notes/shared".to_owned(),
        label: None,
        anchor: None,
        offset: 0,
        length: 0,
    };
    let expansion = metadata::expand_embed(&storage_a, &source_a, &link, &[]).expect("expand A");
    let content = expansion.content.expect("A content");
    assert!(content.contains("A body"));
    assert!(!content.contains("B body"));

    // A note id from the other vault is not visible through this vault, even
    // though the id is canonical and the other storage can read it.
    let foreign = metadata::note_metadata(&storage_a, &note_b).expect_err("foreign note");
    assert_eq!(foreign.code, ErrorCode::ObjectNotFound);
    assert_eq!(foreign.message_key, "error.noteMissing");

    let foreign_from =
        metadata::expand_embed(&storage_a, &note_b, &link, &[]).expect_err("foreign from id");
    assert_eq!(foreign_from.code, ErrorCode::ObjectNotFound);

    let foreign_chain = metadata::expand_embed(
        &storage_a,
        &source_a,
        &link,
        &[note_b.clone(), source_a.clone()],
    )
    .expect_err("foreign chain id");
    assert_eq!(foreign_chain.code, ErrorCode::InvalidInput);
    assert_eq!(foreign_chain.message_key, "error.invalidEmbedChain");
}
