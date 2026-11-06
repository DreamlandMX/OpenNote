use onote_domain::ErrorCode;
use onote_history_backup::{HistoryStore, SnapshotReferenceIssueReason};
use onote_vault_storage::{RevisionArchive, VaultStorage};
use std::sync::Arc;

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn restoring_an_old_revision_creates_a_new_revision_and_preserves_the_previous_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = Arc::new(HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap());
    vault.set_revision_archive(history.clone());
    let first = vault.create_note("test.md", "first", &op()).unwrap();
    vault
        .save(&first.note_id, "second", first.local_revision, &op())
        .unwrap();
    let second = vault.read(&first.note_id).unwrap();
    let revisions = history.list(&first.note_id).unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(
        history
            .read(&first.note_id, &revisions[0].revision_id)
            .unwrap()
            .source,
        first.source
    );
    let restore_op = op();
    let restored = history
        .restore(
            &mut vault,
            &first.note_id,
            &revisions[0].revision_id,
            second.local_revision,
            &restore_op,
        )
        .unwrap();
    assert_eq!(restored.local_revision, second.local_revision + 1);
    assert!(history
        .list(&first.note_id)
        .unwrap()
        .iter()
        .any(|r| r.content_hash == second.content_hash));
    assert_eq!(vault.read(&first.note_id).unwrap().source, first.source);
    assert_eq!(
        history
            .restore(
                &mut vault,
                &first.note_id,
                &revisions[0].revision_id,
                second.local_revision,
                &restore_op
            )
            .unwrap(),
        restored
    );
    drop(vault);
    let reopened = HistoryStore::open(dir.path(), &first.vault_id).unwrap();
    assert_eq!(
        reopened
            .read(&first.note_id, &revisions[0].revision_id)
            .unwrap()
            .source,
        first.source
    );
}

#[test]
fn damaged_history_is_refused_and_cannot_empty_the_current_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap();
    let note = vault.create_note("test.md", "content", &op()).unwrap();
    let revision = history.capture(&note).unwrap();
    let file = dir
        .path()
        .join(".opennote/history")
        .join(&note.note_id)
        .join(format!("{}.json", revision.revision_id));
    std::fs::write(&file, b"{}").unwrap();
    assert_eq!(
        history
            .read(&note.note_id, &revision.revision_id)
            .unwrap_err()
            .code,
        ErrorCode::HistoryMissing
    );
    assert!(history
        .restore(
            &mut vault,
            &note.note_id,
            &revision.revision_id,
            note.local_revision,
            &op()
        )
        .is_err());
    assert_eq!(vault.read(&note.note_id).unwrap().source, note.source);
    assert!(history.preserve(&note).is_err());
    assert_eq!(std::fs::read(file).unwrap(), b"{}");
}

#[test]
fn stale_restore_and_untrusted_identifiers_do_not_change_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap();
    let note = vault.create_note("test.md", "one", &op()).unwrap();
    let old = history.capture(&note).unwrap();
    vault
        .save(&note.note_id, "two", note.local_revision, &op())
        .unwrap();
    let current = vault.read(&note.note_id).unwrap();
    assert_eq!(
        history
            .restore(
                &mut vault,
                &note.note_id,
                &old.revision_id,
                note.local_revision,
                &op()
            )
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );
    assert!(history.read("../escape", "../anything").is_err());
    assert_eq!(vault.read(&note.note_id).unwrap(), current);
}

#[test]
fn ordinary_saves_coalesce_for_five_minutes_but_forced_capture_keeps_current_version() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = Arc::new(HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap());
    vault.set_revision_archive(history.clone());
    let note = vault.create_note("test.md", "one", &op()).unwrap();
    let second = vault
        .save(&note.note_id, "two", note.local_revision, &op())
        .unwrap();
    vault
        .save(&note.note_id, "three", second.local_revision, &op())
        .unwrap();
    assert_eq!(history.list(&note.note_id).unwrap().len(), 1);
    let current = vault.read(&note.note_id).unwrap();
    history.capture(&current).unwrap();
    assert_eq!(history.list(&note.note_id).unwrap().len(), 2);
}

#[test]
fn history_directory_links_cannot_write_outside_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let note = vault.create_note("test.md", "unchanged", &op()).unwrap();
    let link = dir.path().join(".opennote").join("history");
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .unwrap()
        .status
        .success());
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    let history = HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap();
    assert!(history.capture(&note).is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    assert_eq!(vault.read(&note.note_id).unwrap().source, note.source);
}

#[test]
fn a_damaged_version_is_reported_without_hiding_other_readable_versions() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap();
    let note = vault.create_note("test.md", "one", &op()).unwrap();
    let first = history.capture(&note).unwrap();
    vault
        .save(&note.note_id, "two", note.local_revision, &op())
        .unwrap();
    let second = history
        .capture(&vault.read(&note.note_id).unwrap())
        .unwrap();
    let file = dir
        .path()
        .join(".opennote/history")
        .join(&note.note_id)
        .join(format!("{}.json", first.revision_id));
    std::fs::write(file, "{}").unwrap();
    let listing = history.list_available(&note.note_id).unwrap();
    assert_eq!(listing.items, vec![second]);
    assert_eq!(listing.unavailable_revision_ids, vec![first.revision_id]);
}

#[test]
fn restore_operation_binds_revision_identity_even_when_two_versions_have_identical_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap();
    let first = vault.create_note("test.md", "one", &op()).unwrap();
    let old = history.capture(&first).unwrap();
    let second = vault
        .save(&first.note_id, "two", first.local_revision, &op())
        .unwrap();
    vault
        .save(&first.note_id, &first.source, second.local_revision, &op())
        .unwrap();
    let current = vault.read(&first.note_id).unwrap();
    let same_text = history.capture(&current).unwrap();
    let restore_op = op();
    history
        .restore(
            &mut vault,
            &first.note_id,
            &old.revision_id,
            current.local_revision,
            &restore_op,
        )
        .unwrap();
    assert_eq!(
        history
            .restore(
                &mut vault,
                &first.note_id,
                &same_text.revision_id,
                current.local_revision,
                &restore_op
            )
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
    assert_eq!(
        vault
            .save(
                &first.note_id,
                &first.source,
                current.local_revision,
                &restore_op
            )
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyMismatch
    );
}

#[test]
fn interrupted_restore_replays_once_after_reopen_and_preserves_both_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = Arc::new(HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap());
    vault.set_revision_archive(history.clone());
    let first = vault.create_note("中文.md", "第一版", &op()).unwrap();
    let old = history.capture(&first).unwrap();
    vault
        .save(&first.note_id, "当前版", first.local_revision, &op())
        .unwrap();
    let current = vault.read(&first.note_id).unwrap();
    let operation = op();
    vault.push_failpoint(onote_vault_storage::Failpoint::AfterReplace);
    history
        .restore(
            &mut vault,
            &first.note_id,
            &old.revision_id,
            current.local_revision,
            &operation,
        )
        .unwrap_err();
    drop(vault);
    let mut vault = VaultStorage::open(dir.path()).unwrap();
    vault.set_revision_archive(history.clone());
    assert_eq!(
        vault.recover().unwrap().applied,
        vec![first.note_id.clone()]
    );
    let result = history
        .restore(
            &mut vault,
            &first.note_id,
            &old.revision_id,
            current.local_revision,
            &operation,
        )
        .unwrap();
    assert_eq!(result.local_revision, current.local_revision + 1);
    assert_eq!(vault.read(&first.note_id).unwrap().source, first.source);
    assert!(history
        .list(&first.note_id)
        .unwrap()
        .iter()
        .any(|r| r.local_revision == current.local_revision
            && history.read(&first.note_id, &r.revision_id).unwrap().source == current.source));
}

#[test]
fn archive_retains_exact_bom_crlf_and_unknown_yaml_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let vault = VaultStorage::create(dir.path(), "history").unwrap();
    let vault_id = vault.vault().vault_id;
    drop(vault);
    let source = "\u{feff}---\r\ncustom: '保留' # 原样\r\n---\r\n# 第一版\r\n";
    std::fs::write(dir.path().join("原文.md"), source).unwrap();
    let mut vault = VaultStorage::open(dir.path()).unwrap();
    let note_id = vault.list().unwrap()[0].note_id.clone();
    let original = vault.read(&note_id).unwrap();
    let history = Arc::new(HistoryStore::open(dir.path(), &vault_id).unwrap());
    vault.set_revision_archive(history.clone());
    vault
        .save(&note_id, "# 第二版\n", original.local_revision, &op())
        .unwrap();
    let versions = history.list(&note_id).unwrap();
    assert_eq!(
        history
            .read(&note_id, &versions[0].revision_id)
            .unwrap()
            .source
            .as_bytes(),
        source.as_bytes()
    );
}

#[test]
fn history_reads_report_missing_and_unsafe_references_without_hiding_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    std::fs::create_dir_all(dir.path().join("notes/assets")).unwrap();
    std::fs::write(dir.path().join("notes/assets/present.png"), b"image").unwrap();
    let source = concat!(
        "![present](assets/present.png)\n",
        "![missing](assets/missing.png)\n",
        "[unsafe](../../escape.png)\n",
        "[remote](https://example.com/a.png)\n",
        "`![code](ignored.png)`\n",
    );
    let note = vault
        .create_note("notes/version.md", source, &op())
        .unwrap();
    let history = HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap();
    let revision = history.capture(&note).unwrap();
    let content = history
        .read_with_references(&note.note_id, &revision.revision_id)
        .unwrap();
    assert_eq!(content.document, note);
    assert_eq!(
        content
            .reference_report
            .issues
            .iter()
            .map(|issue| (issue.reference.as_str(), issue.reason))
            .collect::<Vec<_>>(),
        vec![
            ("../../escape.png", SnapshotReferenceIssueReason::Unsafe),
            ("assets/missing.png", SnapshotReferenceIssueReason::Missing),
        ]
    );
    assert!(!content.reference_report.complete);

    std::fs::remove_file(dir.path().join("notes/assets/present.png")).unwrap();
    let after_removal = history
        .read_with_references(&note.note_id, &revision.revision_id)
        .unwrap();
    assert_eq!(after_removal.document.source, note.source);
    assert!(after_removal
        .reference_report
        .issues
        .iter()
        .any(|issue| issue.reference == "assets/present.png"
            && issue.reason == SnapshotReferenceIssueReason::Missing));
}

#[test]
fn restoring_r3_over_r8_creates_r9_and_keeps_r8_readable() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = VaultStorage::create(dir.path(), "history").unwrap();
    let history = Arc::new(HistoryStore::open(dir.path(), &vault.vault().vault_id).unwrap());
    vault.set_revision_archive(history.clone());
    let note = vault.create_note("note.md", "revision 0", &op()).unwrap();
    let mut old = None;
    for revision in 1..=8 {
        vault
            .save(
                &note.note_id,
                &format!("revision {revision}"),
                revision - 1,
                &op(),
            )
            .unwrap();
        if revision == 3 {
            old = Some(
                history
                    .capture(&vault.read(&note.note_id).unwrap())
                    .unwrap(),
            );
        }
    }
    let old = old.unwrap();
    let restored = history
        .restore(&mut vault, &note.note_id, &old.revision_id, 8, &op())
        .unwrap();
    assert_eq!(restored.local_revision, 9);
    assert!(vault
        .read(&note.note_id)
        .unwrap()
        .source
        .ends_with("revision 3"));
    let previous = history
        .list(&note.note_id)
        .unwrap()
        .into_iter()
        .find(|r| r.local_revision == 8)
        .unwrap();
    assert!(history
        .read(&note.note_id, &previous.revision_id)
        .unwrap()
        .source
        .ends_with("revision 8"));
}
