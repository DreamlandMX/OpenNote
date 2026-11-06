use std::fs;
use std::path::Path;

use onote_domain::ErrorCode;
use tempfile::TempDir;
use uuid::Uuid;

use onote_vault_storage::VaultStorage;

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

// ----------------------------------------------------------------------------
// helpers (deterministic, per-test temporary dirs)
// ----------------------------------------------------------------------------

fn op_id() -> String {
    Uuid::new_v4().to_string()
}

fn write_bytes(path: &Path, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

fn summary_for(storage: &VaultStorage, relative: &str) -> onote_domain::NoteSummary {
    storage
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == relative)
        .expect("note must be listed")
}

// ----------------------------------------------------------------------------
// quoted/plain/uppercase `opennote-id` values load through the real YAML
// parser and surface canonicalized from the scan
// ----------------------------------------------------------------------------

#[test]
fn scanned_id_quoted_with_comment_canonicalized() {
    let dir = TempDir::new().unwrap();
    let quoted_id = Uuid::new_v4();
    let simple_id = Uuid::new_v4();
    write_bytes(
        &dir.path().join("q.md"),
        format!(
            "---\nopennote-id: \"{}\" # storage identity\n---\nbody\n",
            quoted_id.to_string().to_uppercase()
        )
        .as_bytes(),
    );
    write_bytes(
        &dir.path().join("simple.md"),
        // UUID simple form (no hyphens) is still a valid scalar UUID
        format!(
            "---\nopennote-id: {}\n---\nbody\n",
            simple_id.to_string().replace('-', "")
        )
        .as_bytes(),
    );
    let before_q = read_bytes(&dir.path().join("q.md"));
    let before_simple = read_bytes(&dir.path().join("simple.md"));

    let storage = VaultStorage::open(dir.path()).unwrap();
    let q = summary_for(&storage, "q.md");
    let simple = summary_for(&storage, "simple.md");
    assert!(!q.read_only, "quoted+commented UUID must scan writable");
    assert!(!simple.read_only, "simple UUID form must scan writable");
    assert_eq!(
        q.note_id,
        quoted_id.to_string(),
        "canonicalized hyphenated UUID"
    );
    assert_eq!(simple.note_id, simple_id.to_string());
    // unknown lines / quotes / comments are never rewritten by scanning
    assert_eq!(read_bytes(&dir.path().join("q.md")), before_q);
    assert_eq!(read_bytes(&dir.path().join("simple.md")), before_simple);
}

// ----------------------------------------------------------------------------
// a nested `opennote-id` key is ordinary unknown YAML: never extracted,
// never validated, never rejected
// ----------------------------------------------------------------------------

#[test]
fn nested_opennote_id_is_ordinary_unknown_yaml() {
    let dir = TempDir::new().unwrap();
    let content = "---\ntitle: t\nprops:\n  opennote-id: certainly-not-a-uuid\n---\nbody\n";
    write_bytes(&dir.path().join("n.md"), content.as_bytes());
    let before = read_bytes(&dir.path().join("n.md"));

    let storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "n.md");
    assert!(
        !note.read_only,
        "nested opennote-id must not make the note read-only"
    );
    assert_eq!(read_bytes(&dir.path().join("n.md")), before);
}

// ----------------------------------------------------------------------------
// duplicate keys anywhere, invalid YAML, non-mapping top level and an
// invalid reserved id: scan read-only, disk bytes preserved
// ----------------------------------------------------------------------------

#[test]
fn malformed_or_invalid_frontmatter_scans_read_only() {
    let dir = TempDir::new().unwrap();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "duplicate_keys.md",
            b"---\ntitle: a\ntitle: b\n---\nbody\n".to_vec(),
        ),
        (
            "duplicate_reserved.md",
            format!(
                "---\nopennote-id: {}\nopennote-id: {}\n---\nbody\n",
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .as_bytes()
            .to_vec(),
        ),
        (
            "unterminated_flow.md",
            b"---\ntags: [a, b\n---\nbody\n".to_vec(),
        ),
        ("unclosed_block.md", b"---\ntitle: never closed".to_vec()),
        (
            "scalar_block.md",
            b"---\njust a scalar\n---\nbody\n".to_vec(),
        ),
        ("sequence_block.md", b"---\n- a\n- b\n---\nbody\n".to_vec()),
        (
            "non_scalar_reserved.md",
            b"---\nopennote-id: [1, 2]\n---\nbody\n".to_vec(),
        ),
        (
            "null_reserved.md",
            b"---\nopennote-id:\n---\nbody\n".to_vec(),
        ),
        (
            "invalid_reserved.md",
            b"---\nopennote-id: not-a-uuid\n---\nbody\n".to_vec(),
        ),
    ];
    for (name, content) in &cases {
        write_bytes(&dir.path().join(name), content);
    }

    let storage = VaultStorage::open(dir.path()).unwrap();
    let notes = storage.list().unwrap();
    for (name, content) in &cases {
        let note = notes.iter().find(|n| &n.relative_path == name).unwrap();
        assert!(note.read_only, "{name} must scan read-only");
        assert_eq!(
            read_bytes(&dir.path().join(name)),
            *content,
            "{name} disk bytes must be preserved by scanning"
        );
    }
}

// ----------------------------------------------------------------------------
// empty or comment-only blocks are accepted (no identity, still writable)
// ----------------------------------------------------------------------------

#[test]
fn empty_or_comment_only_block_is_writable() {
    let dir = TempDir::new().unwrap();
    let cases: Vec<(&str, &[u8])> = vec![
        ("empty.md", b"---\n---\nbody\n"),
        ("comments.md", b"---\n# a note about nothing\n---\nbody\n"),
        ("blank_lines.md", b"---\n\n---\nbody\n"),
    ];
    for (name, content) in &cases {
        write_bytes(&dir.path().join(name), content);
    }

    let storage = VaultStorage::open(dir.path()).unwrap();
    for (name, content) in &cases {
        let note = summary_for(&storage, name);
        assert!(!note.read_only, "{name} must stay writable");
        assert_eq!(read_bytes(&dir.path().join(name)), content.to_vec());
    }
}

// ----------------------------------------------------------------------------
// explicit create rejects malformed/invalid/foreign frontmatter
// ----------------------------------------------------------------------------

#[test]
fn create_rejects_malformed_invalid_or_foreign_frontmatter() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let cases: Vec<(&str, String)> = vec![
        (
            "duplicate.md",
            "---\ntitle: a\ntitle: b\n---\nbody\n".into(),
        ),
        (
            "invalid.md",
            "---\nopennote-id: not-a-uuid\n---\nbody\n".into(),
        ),
        ("scalar.md", "---\nscalar only\n---\nbody\n".into()),
        (
            "foreign_quoted.md",
            format!(
                "---\nopennote-id: \"{}\" # someone else's\n---\nbody\n",
                Uuid::new_v4()
            ),
        ),
    ];
    for (name, source) in &cases {
        let err = storage.create_note(name, source, &op_id()).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::InvalidInput,
            "{name}: expected InvalidInput, got {:?} ({:?})",
            err.code,
            err.message_key
        );
        assert!(
            !dir.path().join(name).exists(),
            "{name} must not be created"
        );
    }
}

// ----------------------------------------------------------------------------
// explicit save rejects a malformed/invalid source and keeps disk bytes
// ----------------------------------------------------------------------------

#[test]
fn save_rejects_malformed_or_invalid_source_keeping_disk_bytes() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let a = storage.create_note("a.md", "A body", &op_id()).unwrap();
    let b = storage.create_note("b.md", "B body", &op_id()).unwrap();

    let corrupt_a: &str = "---\ntitle: a\ntitle: b\n---\nbody\n";
    let corrupt_b: &str = "---\nopennote-id: not-a-uuid\n---\nbody\n";
    write_bytes(&dir.path().join("a.md"), corrupt_a.as_bytes());
    write_bytes(&dir.path().join("b.md"), corrupt_b.as_bytes());

    // read refreshes the baseline after the external change; a later save of
    // those very bytes must be rejected by composition (defense in depth for
    // any path that bypasses the scan-time read_only flag)
    let doc_a = storage.read(a.note_id.as_str()).unwrap();
    let doc_b = storage.read(b.note_id.as_str()).unwrap();
    assert_eq!(doc_a.source, corrupt_a);
    assert_eq!(doc_b.source, corrupt_b);

    for doc in [&doc_a, &doc_b] {
        let err = storage
            .save(
                doc.note_id.as_str(),
                &doc.source,
                doc.local_revision,
                &op_id(),
            )
            .unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::InvalidInput,
            "expected InvalidInput, got {:?} ({:?})",
            err.code,
            err.message_key
        );
    }
    assert_eq!(read_bytes(&dir.path().join("a.md")), corrupt_a.as_bytes());
    assert_eq!(read_bytes(&dir.path().join("b.md")), corrupt_b.as_bytes());
}

// ----------------------------------------------------------------------------
// Unicode BOM is skipped by exact byte length before the first-line check
// ----------------------------------------------------------------------------

#[test]
fn bom_before_frontmatter_skipped_by_scan() {
    let dir = TempDir::new().unwrap();
    let id = Uuid::new_v4();
    let text = format!(
        "---\r\nopennote-id: \"{}\" # c\r\nother: keep\r\n---\r\nbody\r\n",
        id
    );
    let mut bytes = BOM.to_vec();
    bytes.extend(text.as_bytes());
    write_bytes(&dir.path().join("bom.md"), &bytes);

    let storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "bom.md");
    assert!(!note.read_only, "BOM + valid id block must scan writable");
    assert_eq!(note.note_id, id.to_string());
    assert_eq!(
        read_bytes(&dir.path().join("bom.md")),
        bytes,
        "scan must not touch BOM'd bytes"
    );
}

// ----------------------------------------------------------------------------
// new notes: UTF-8 without BOM, LF line endings; body EOLs normalized
// ----------------------------------------------------------------------------

#[test]
fn new_note_has_no_bom_and_lf() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let note = storage
        .create_note("n.md", "\u{FEFF}# first\r\nline two\r\n", &op_id())
        .unwrap();

    let expected = format!(
        "---\nopennote-id: {}\n---\n# first\nline two\n",
        note.note_id
    );
    assert_eq!(note.source, expected);
    let disk = read_bytes(&dir.path().join("n.md"));
    assert_eq!(disk, expected.as_bytes());
    assert!(!disk.starts_with(BOM), "new note must not carry a BOM");
    assert!(!disk.contains(&b'\r'), "new note must use LF only");
    assert!(disk.contains(&b'\n'));
}

// ----------------------------------------------------------------------------
// an indented `---` inside a literal block scalar is content, never the
// closing delimiter; the closing marker must sit at column zero
// ----------------------------------------------------------------------------

#[test]
fn literal_block_scalar_with_indented_marker_fully_preserved() {
    let dir = TempDir::new().unwrap();
    let content = "---\ndescription: |\n  para one\n  ---\n  para two\ntitle: demo\n---\nbody\n";
    write_bytes(&dir.path().join("lit.md"), content.as_bytes());

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "lit.md");
    assert!(
        !note.read_only,
        "indented --- inside a literal block must not close the frontmatter"
    );
    assert_eq!(read_bytes(&dir.path().join("lit.md")), content.as_bytes());

    let doc = storage.read(note.note_id.as_str()).unwrap();
    assert_eq!(doc.source, content, "literal block passed through verbatim");
    storage
        .save(
            note.note_id.as_str(),
            &doc.source,
            doc.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("save with indented marker returned {e:?}"));

    let saved_text = String::from_utf8(read_bytes(&dir.path().join("lit.md"))).unwrap();
    assert_eq!(
        saved_text,
        format!(
            "---\nopennote-id: {}\ndescription: |\n  para one\n  ---\n  para two\ntitle: demo\n---\nbody\n",
            note.note_id
        )
    );

    // after save the whole block (indented marker included) still parses
    let reopened = VaultStorage::open(dir.path()).unwrap();
    let note2 = reopened
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "lit.md")
        .unwrap();
    assert_eq!(
        note2.note_id, note.note_id,
        "id intact across the full block"
    );
    assert!(!note2.read_only);
}

// ----------------------------------------------------------------------------
// a top-level flow mapping without an id receives its identity inside the
// flow braces; every original raw byte is preserved and the composed
// artifact re-parses, so a later scan never flips to read-only
// ----------------------------------------------------------------------------

#[test]
fn flow_map_without_id_inserts_identity_keeping_raw_bytes() {
    let dir = TempDir::new().unwrap();
    let mut storage = VaultStorage::create(dir.path(), "T").unwrap();
    let source = "---\n{title: Demo, tags: [a, b]}\n---\nbody\n";
    let note = storage
        .create_note("flow.md", source, &op_id())
        .unwrap_or_else(|e| panic!("flow map create returned {e:?}"));

    let expected = format!(
        "---\n{{opennote-id: {},title: Demo, tags: [a, b]}}\n---\nbody\n",
        note.note_id
    );
    assert_eq!(note.source, expected, "flow bytes must be preserved inline");
    assert_eq!(read_bytes(&dir.path().join("flow.md")), expected.as_bytes());

    let mut reopened = VaultStorage::open(dir.path()).unwrap();
    let n2 = summary_for(&reopened, "flow.md");
    assert!(!n2.read_only, "flow map with inserted id stays writable");
    assert_eq!(
        n2.note_id, note.note_id,
        "id extracted back from the flow map"
    );

    let doc = reopened.read(note.note_id.as_str()).unwrap();
    reopened
        .save(
            note.note_id.as_str(),
            &doc.source,
            doc.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("flow map save returned {e:?}"));
    assert_eq!(
        read_bytes(&dir.path().join("flow.md")),
        expected.as_bytes(),
        "save must not rewrite the flow content"
    );
}

#[test]
fn flow_map_with_valid_id_scans_and_saves_unchanged() {
    let dir = TempDir::new().unwrap();
    let id = Uuid::new_v4();
    let content = format!("---\n{{\"opennote-id\": \"{id}\", title: Demo}}\n---\nbody\n");
    write_bytes(&dir.path().join("flowid.md"), content.as_bytes());

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "flowid.md");
    assert!(!note.read_only, "valid flow-map id must scan writable");
    assert_eq!(note.note_id, id.to_string());

    let doc = storage.read(note.note_id.as_str()).unwrap();
    let saved = storage
        .save(
            note.note_id.as_str(),
            &doc.source,
            doc.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("flow map save returned {e:?}"));
    assert_eq!(saved.local_revision, doc.local_revision + 1);
    assert_eq!(
        read_bytes(&dir.path().join("flowid.md")),
        content.as_bytes(),
        "valid frontmatter must never be rewritten"
    );
}

// ----------------------------------------------------------------------------
// insertion-unsupported shapes (anchored root flow node, or a root mapping
// indented off column zero whose plain prepend would be invalid YAML) scan
// read-only and explicit composition refuses typed; bytes never change
// ----------------------------------------------------------------------------

#[test]
fn unsupported_insertion_scans_read_only_and_save_refuses() {
    let dir = TempDir::new().unwrap();
    write_bytes(
        &dir.path().join("indent.md"),
        b"---\n  title: demo\n---\nbody\n",
    );
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "indent.md");
    assert!(
        note.read_only,
        "root mapping indentation incompatible with prepend must scan read-only"
    );
    assert_eq!(
        read_bytes(&dir.path().join("indent.md")),
        b"---\n  title: demo\n---\nbody\n",
        "scan must not touch the bytes"
    );
    let err = storage
        .save(
            note.note_id.as_str(),
            "fresh body",
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidInput,
        "explicit save must refuse (typed), got {:?}",
        err.message_key
    );
    assert_eq!(
        read_bytes(&dir.path().join("indent.md")),
        b"---\n  title: demo\n---\nbody\n",
        "refused save keeps disk bytes"
    );
}

#[test]
fn anchored_root_flow_map_never_persists_invalid_frontmatter() {
    let dir = TempDir::new().unwrap();
    write_bytes(
        &dir.path().join("anch.md"),
        b"---\n&anchor {title: Demo}\n---\nbody\n",
    );
    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "anch.md");
    assert!(
        note.read_only,
        "anchor-prefixed root must scan read-only (unsupported insertion)"
    );
    let before = read_bytes(&dir.path().join("anch.md"));
    let err = storage
        .save(
            note.note_id.as_str(),
            &String::from_utf8(before.clone()).unwrap(),
            note.local_revision,
            &op_id(),
        )
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidInput,
        "anchor-prefixed root insertion must refuse typed, got {:?}",
        err.message_key
    );
    assert_eq!(
        read_bytes(&dir.path().join("anch.md")),
        before,
        "anchored root bytes unchanged"
    );
}

// ----------------------------------------------------------------------------
// legacy save: identity insertion into an existing block keeps the supplied
// BOM and newline preference verbatim
// ----------------------------------------------------------------------------

#[test]
fn legacy_frontmatter_save_retains_bom_and_crlf() {
    let dir = TempDir::new().unwrap();
    let text = "---\r\ntitle: t\r\n---\r\nbody\r\n";
    let mut bytes = BOM.to_vec();
    bytes.extend(text.as_bytes());
    write_bytes(&dir.path().join("l.md"), &bytes);

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "l.md");
    let doc = storage.read(note.note_id.as_str()).unwrap();
    let saved = storage
        .save(
            note.note_id.as_str(),
            &doc.source,
            doc.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("legacy save returned {e:?}"));
    assert_eq!(saved.local_revision, doc.local_revision + 1);

    let expected = format!(
        "\u{FEFF}---\r\nopennote-id: {}\r\ntitle: t\r\n---\r\nbody\r\n",
        note.note_id
    );
    assert_eq!(read_bytes(&dir.path().join("l.md")), expected.as_bytes());
}

// ----------------------------------------------------------------------------
// insertion that would compose valid YAML is verified before any successful
// return; this holds for the CRLF legacy path too: an identity-less block in
// a BOM'd CRLF file receives id + \r\n and stays openable
// ----------------------------------------------------------------------------

#[test]
fn crlf_saved_identity_reextraction() {
    let dir = TempDir::new().unwrap();
    let text = "---\r\ntitle: t\r\n---\r\nbody\r\n";
    let mut bytes = BOM.to_vec();
    bytes.extend(text.as_bytes());
    write_bytes(&dir.path().join("crlf.md"), &bytes);

    let mut storage = VaultStorage::open(dir.path()).unwrap();
    let note = summary_for(&storage, "crlf.md");
    assert!(!note.read_only, "CRLF identity-less block stays writable");
    let doc = storage.read(note.note_id.as_str()).unwrap();
    storage
        .save(
            note.note_id.as_str(),
            &doc.source,
            doc.local_revision,
            &op_id(),
        )
        .unwrap_or_else(|e| panic!("CRLF legacy save returned {e:?}"));

    let reopened = VaultStorage::open(dir.path()).unwrap();
    let note2 = reopened
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.relative_path == "crlf.md")
        .unwrap();
    assert_eq!(note2.note_id, note.note_id);
    assert!(!note2.read_only);
}
