//! F10-R03 / F03-A01: rename planning.
//!
//! M04 produces a read-only plan; M02 validates and executes it. Two rules are
//! load-bearing:
//!
//! * a plan is never a global string replacement — every edit is a byte range
//!   around a link M04 actually parsed, so prose that happens to contain the
//!   old title is left alone;
//! * only links that resolved *to this note* are rewritten, so a same-named
//!   note elsewhere is never silently retargeted (F10-A02).

use crate::index::VaultIndex;
use crate::links::LinkKind;
use crate::resolve::LinkStatus;
use serde::{Deserialize, Serialize};

pub use onote_domain::{
    RenameBaseline, RenameEdit, RenamePlan, RenameWarning, RenameWarningReason,
};

/// The readable title of a note: its file stem, without the `.md` extension.
pub fn title_of(relative_path: &str) -> String {
    let name = relative_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(relative_path);
    name.strip_suffix(".md")
        .or_else(|| name.strip_suffix(".MD"))
        .unwrap_or(name)
        .to_owned()
}

/// The link spelling a plan prefers when writing a new target: the readable
/// path without the `.md` extension, which stays valid after a rename.
pub fn link_target_for(relative_path: &str) -> String {
    relative_path
        .strip_suffix(".md")
        .or_else(|| relative_path.strip_suffix(".MD"))
        .unwrap_or(relative_path)
        .replace('\\', "/")
}

/// Builds the plan for renaming `note_id` from its current path to `to_path`.
///
/// The plan is pure data: it records byte ranges and the exact replacement
/// text. It never writes, so a cancelled rename leaves every note untouched
/// (F10-R06).
pub fn plan_rename(index: &VaultIndex, note_id: &str, to_path: &str) -> Option<RenamePlan> {
    let target = index.note(note_id)?;
    let from_path = target.relative_path.clone();
    let path_target = link_target_for(to_path);
    // Wiki delimiters and UUID-shaped filenames need identity links to round-trip.
    let new_target = if path_target.contains(['#', '[', ']', '|'])
        || uuid::Uuid::parse_str(&path_target).is_ok()
    {
        note_id.to_owned()
    } else {
        path_target
    };
    let mut edits = Vec::new();
    let mut untouched = Vec::new();
    for note in index.notes() {
        let Some(source) = index.source(&note.note_id) else {
            continue;
        };
        for parsed in &note.links {
            let status = parsed.resolved.status;
            let points_at_target = parsed.resolved.target_id.as_deref() == Some(note_id);
            if !points_at_target {
                if matches!(status, LinkStatus::AmbiguousTarget | LinkStatus::BrokenLink)
                    && resembles(parsed, &from_path, &target.title)
                {
                    untouched.push(RenameWarning {
                        note_id: note.note_id.clone(),
                        relative_path: note.relative_path.clone(),
                        offset: parsed.link.offset,
                        target: parsed.link.target.clone(),
                        reason: if status == LinkStatus::AmbiguousTarget {
                            RenameWarningReason::AmbiguousTarget
                        } else {
                            RenameWarningReason::BrokenLink
                        },
                    });
                }
                continue;
            }
            let before = source
                .get(parsed.link.offset..parsed.link.offset + parsed.link.length)
                .unwrap_or_default()
                .to_owned();
            // An identity link (the target is the note id) already survives a
            // rename, so rewriting it would only churn the file.
            if parsed.link.target.trim() == note_id {
                continue;
            }
            let after = rebuild_link(&parsed.link.kind, &new_target, parsed, &before);
            if after == before {
                continue;
            }
            edits.push(RenameEdit {
                note_id: note.note_id.clone(),
                relative_path: note.relative_path.clone(),
                offset: parsed.link.offset,
                length: parsed.link.length,
                before,
                after,
            });
        }
    }
    edits.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.offset.cmp(&right.offset))
    });
    untouched.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.offset.cmp(&right.offset))
    });
    Some(RenamePlan {
        plan_id: uuid::Uuid::new_v4().to_string(),
        vault_id: String::new(),
        expected_hashes: Vec::new(),
        note_id: note_id.to_owned(),
        from_path,
        to_path: to_path.to_owned(),
        keeps_identity: true,
        edits,
        untouched,
    })
}

fn resembles(parsed: &crate::index::ParsedLink, from_path: &str, title: &str) -> bool {
    let target = parsed.link.target.trim();
    if target.is_empty() {
        return false;
    }
    let normalized = crate::resolve::normalize_path(target);
    let old = crate::resolve::normalize_path(from_path);
    normalized == old || target.eq_ignore_ascii_case(title)
}

/// Rebuilds one link keeping its kind, label and anchor. The `before` text is
/// used only to preserve whatever spelling of the label the user chose.
fn rebuild_link(
    kind: &LinkKind,
    new_target: &str,
    parsed: &crate::index::ParsedLink,
    before: &str,
) -> String {
    let mut rebuilt = String::new();
    if *kind == LinkKind::Embed {
        rebuilt.push('!');
    }
    rebuilt.push_str("[[");
    rebuilt.push_str(new_target);
    if let Some(anchor) = &parsed.link.anchor {
        rebuilt.push('#');
        rebuilt.push_str(anchor);
    }
    if let Some(label) = &parsed.link.label {
        rebuilt.push('|');
        rebuilt.push_str(label);
    }
    rebuilt.push_str("]]");
    // If the reconstruction cannot be the exact same shape as the source text,
    // keep the original rather than emit a link the parser would not read back.
    if rebuilt.len() == before.len() || before.starts_with("[[") || before.starts_with("![[") {
        rebuilt
    } else {
        before.to_owned()
    }
}

/// One note a rename has to rewrite and save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitStep {
    pub note_id: String,
    pub relative_path: String,
    /// The source after the link rewrite.
    pub source: String,
    /// The revision the caller read, so the save can detect a conflict.
    pub expected_revision: u64,
    /// Links rewritten in this note.
    pub edits: usize,
}

/// Why a set of commit steps cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommitIssue {
    /// A note changed since the plan was made (VERSION_CONFLICT).
    RevisionChanged {
        note_id: String,
        expected: u64,
        actual: u64,
    },
    /// The note the plan moves is not the note that exists now.
    TargetMissing { note_id: String },
    /// The destination is already occupied (PATH_COLLISION).
    DestinationOccupied { path: String },
    /// The plan would move a note into its own subtree.
    NestedDestination { path: String, destination: String },
}

/// The steps to persist a rename, plus whatever blocks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameCommit {
    /// Every note whose links change, in a deterministic order.
    pub steps: Vec<CommitStep>,
    /// The move itself: the file to relocate.
    pub move_from: String,
    pub move_to: String,
    /// True when the note id survives the move (F03-R02).
    pub keeps_identity: bool,
    /// Non-empty when the commit must not run.
    pub issues: Vec<CommitIssue>,
    /// Issues the plan already reported and deliberately left alone.
    pub untouched: Vec<RenameWarning>,
}

impl RenameCommit {
    /// True when every step may be written.
    pub fn is_applicable(&self) -> bool {
        self.issues.is_empty()
    }

    /// How many links across all notes change.
    pub fn total_edits(&self) -> usize {
        self.steps.iter().map(|step| step.edits).sum()
    }
}

/// Builds the commit steps for a plan.
///
/// `revisions` is what the caller read when it made the plan; `actual` is what the
/// vault reports now. A difference means the plan is stale and must be rebuilt
/// rather than applied, because the link offsets would land in the wrong place.
/// `existing_paths` is used only for the destination collision check.
pub fn plan_commit(
    index: &VaultIndex,
    plan: &RenamePlan,
    revisions: &[(String, u64)],
    actual: &[(String, u64)],
    existing_paths: &[String],
    _force: bool,
) -> RenameCommit {
    let mut issues: Vec<CommitIssue> = Vec::new();

    let rewritten = apply_plan(index, plan);
    let mut steps: Vec<CommitStep> = Vec::new();
    for (note_id, relative_path, source) in rewritten {
        let expected = revisions
            .iter()
            .find(|(id, _)| id == &note_id)
            .map(|(_, revision)| *revision);
        let current = actual
            .iter()
            .find(|(id, _)| id == &note_id)
            .map(|(_, revision)| *revision);
        match (expected, current) {
            (Some(expected), Some(current)) if expected != current => {
                issues.push(CommitIssue::RevisionChanged {
                    note_id: note_id.clone(),
                    expected,
                    actual: current,
                });
                continue;
            }
            (None, _) => {
                // A note the caller never read cannot be saved safely.
                issues.push(CommitIssue::RevisionChanged {
                    note_id: note_id.clone(),
                    expected: 0,
                    actual: current.unwrap_or(0),
                });
                continue;
            }
            _ => {}
        }
        let edits = plan
            .edits
            .iter()
            .filter(|edit| edit.note_id == note_id)
            .count();
        steps.push(CommitStep {
            note_id,
            relative_path,
            source,
            expected_revision: expected.unwrap_or(0),
            edits,
        });
    }
    steps.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    // The move itself: the target must still exist and the destination must be free
    // even when an older caller supplies the legacy force hint.
    if index.note(&plan.note_id).is_none() {
        issues.push(CommitIssue::TargetMissing {
            note_id: plan.note_id.clone(),
        });
    }
    let folded_destination = plan.to_path.replace('\\', "/").to_lowercase();
    if existing_paths.iter().any(|path| {
        path.replace('\\', "/").to_lowercase() == folded_destination
            && path.replace('\\', "/") != plan.from_path
    }) {
        issues.push(CommitIssue::DestinationOccupied {
            path: plan.to_path.clone(),
        });
    }
    let from_folded = plan.from_path.replace('\\', "/").to_lowercase();
    if folded_destination.len() > from_folded.len()
        && folded_destination.starts_with(&from_folded)
        && folded_destination.as_bytes().get(from_folded.len()) == Some(&b'/')
    {
        issues.push(CommitIssue::NestedDestination {
            path: plan.from_path.clone(),
            destination: plan.to_path.clone(),
        });
    }

    RenameCommit {
        steps,
        move_from: plan.from_path.clone(),
        move_to: plan.to_path.clone(),
        keeps_identity: plan.keeps_identity,
        issues,
        untouched: plan.untouched.clone(),
    }
}

/// Applies a plan to note sources, returning the rewritten sources. Callers own
/// persistence; this keeps the planning module free of I/O.
pub fn apply_plan(index: &VaultIndex, plan: &RenamePlan) -> Vec<(String, String, String)> {
    let mut by_note: std::collections::BTreeMap<&str, Vec<&RenameEdit>> = Default::default();
    for edit in &plan.edits {
        by_note.entry(edit.note_id.as_str()).or_default().push(edit);
    }
    let mut results = Vec::new();
    for (note_id, mut edits) in by_note {
        let Some(source) = index.source(note_id) else {
            continue;
        };
        let Some(metadata) = index.note(note_id) else {
            continue;
        };
        // Apply from the end so earlier offsets stay valid.
        edits.sort_by(|left, right| right.offset.cmp(&left.offset));
        let mut rewritten = source.to_owned();
        for edit in edits {
            if edit.offset + edit.length > rewritten.len() {
                continue;
            }
            rewritten.replace_range(edit.offset..edit.offset + edit.length, &edit.after);
        }
        results.push((
            note_id.to_owned(),
            metadata.relative_path.clone(),
            rewritten,
        ));
    }
    results
}

#[cfg(test)]
mod commit_tests {
    use super::*;
    use crate::index::NoteSource;

    const A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

    fn index() -> VaultIndex {
        VaultIndex::rebuild(&[
            NoteSource::new(
                A.to_owned(),
                "a.md".to_owned(),
                "a".to_owned(),
                "[[b]]\n".to_owned(),
            ),
            NoteSource::new(
                B.to_owned(),
                "b.md".to_owned(),
                "b".to_owned(),
                "body\n".to_owned(),
            ),
        ])
    }

    fn revision(id: &str, value: u64) -> (String, u64) {
        (id.to_owned(), value)
    }

    #[test]
    fn a_clean_plan_produces_one_step_per_affected_note() {
        let index = index();
        let plan = plan_rename(&index, B, "moved/b.md").expect("plan");
        let commit = plan_commit(
            &index,
            &plan,
            &[revision(A, 1), revision(B, 1)],
            &[revision(A, 1), revision(B, 1)],
            &["a.md".to_owned(), "b.md".to_owned()],
            false,
        );
        assert!(commit.is_applicable(), "issues: {:?}", commit.issues);
        // Only the note that links to b needs rewriting.
        assert_eq!(commit.steps.len(), 1);
        assert_eq!(commit.steps[0].note_id, A);
        assert_eq!(commit.steps[0].edits, 1);
        assert_eq!(commit.move_from, "b.md");
        assert_eq!(commit.move_to, "moved/b.md");
        assert!(commit.keeps_identity);
        assert_eq!(commit.total_edits(), 1);
    }

    #[test]
    fn a_changed_revision_blocks_the_commit() {
        // VERSION_CONFLICT: the offsets would land in the wrong place.
        let index = index();
        let plan = plan_rename(&index, B, "moved/b.md").expect("plan");
        let commit = plan_commit(
            &index,
            &plan,
            &[revision(A, 1)],
            &[revision(A, 2)],
            &[],
            false,
        );
        assert!(!commit.is_applicable());
        assert!(matches!(
            commit.issues.first(),
            Some(CommitIssue::RevisionChanged { note_id, .. }) if note_id == A
        ));
        // The stale note is not included, so nothing is written against a bad base.
        assert!(commit.steps.is_empty());
    }

    #[test]
    fn a_note_never_read_cannot_be_committed() {
        let index = index();
        let plan = plan_rename(&index, B, "moved/b.md").expect("plan");
        let commit = plan_commit(&index, &plan, &[], &[revision(A, 1)], &[], false);
        assert!(!commit.is_applicable());
    }

    #[test]
    fn an_occupied_destination_blocks_the_move() {
        // PATH_COLLISION: never overwrite.
        let index = index();
        let plan = plan_rename(&index, B, "a.md").expect("plan");
        let commit = plan_commit(
            &index,
            &plan,
            &[revision(A, 1), revision(B, 1)],
            &[revision(A, 1), revision(B, 1)],
            &["a.md".to_owned(), "b.md".to_owned()],
            false,
        );
        assert!(commit
            .issues
            .iter()
            .any(|issue| matches!(issue, CommitIssue::DestinationOccupied { .. })));
    }

    #[test]
    fn the_collision_check_ignores_case() {
        // Windows treats these as one file.
        let index = index();
        let plan = plan_rename(&index, B, "a.md").expect("plan");
        let commit = plan_commit(
            &index,
            &plan,
            &[revision(A, 1), revision(B, 1)],
            &[revision(A, 1), revision(B, 1)],
            &["A.md".to_owned(), "b.md".to_owned()],
            false,
        );
        assert!(commit
            .issues
            .iter()
            .any(|issue| matches!(issue, CommitIssue::DestinationOccupied { .. })));
    }

    #[test]
    fn a_destination_inside_the_moved_note_is_refused() {
        let index = index();
        // `plan_rename` returns None for some paths; build the plan directly so the
        // nesting check itself is what gets tested.
        let plan = RenamePlan {
            plan_id: uuid::Uuid::new_v4().to_string(),
            vault_id: String::new(),
            expected_hashes: Vec::new(),
            note_id: B.to_owned(),
            from_path: "b.md".to_owned(),
            to_path: "b.md/sub/b.md".to_owned(),
            keeps_identity: true,
            edits: Vec::new(),
            untouched: Vec::new(),
        };
        let commit = plan_commit(&index, &plan, &[], &[], &[], false);
        assert!(commit
            .issues
            .iter()
            .any(|issue| matches!(issue, CommitIssue::NestedDestination { .. })));
    }

    #[test]
    fn force_never_allows_an_occupied_destination() {
        let index = index();
        let plan = plan_rename(&index, B, "a.md").expect("plan");
        let forced = plan_commit(
            &index,
            &plan,
            &[revision(A, 1), revision(B, 1)],
            &[revision(A, 1), revision(B, 1)],
            &["a.md".to_owned(), "b.md".to_owned()],
            true,
        );
        assert!(forced
            .issues
            .iter()
            .any(|issue| matches!(issue, CommitIssue::DestinationOccupied { .. })));
    }

    #[test]
    fn a_missing_target_is_reported() {
        let index = index();
        let plan = RenamePlan {
            plan_id: uuid::Uuid::new_v4().to_string(),
            vault_id: String::new(),
            expected_hashes: Vec::new(),
            note_id: "gone".to_owned(),
            from_path: "gone.md".to_owned(),
            to_path: "new.md".to_owned(),
            keeps_identity: true,
            edits: Vec::new(),
            untouched: Vec::new(),
        };
        let commit = plan_commit(&index, &plan, &[], &[], &[], false);
        assert!(commit
            .issues
            .iter()
            .any(|issue| matches!(issue, CommitIssue::TargetMissing { .. })));
    }

    #[test]
    fn the_plan_carries_the_untouched_warnings_through() {
        // A rename must not silently repair a broken link, and the commit keeps
        // that fact visible.
        let index = VaultIndex::rebuild(&[
            NoteSource::new(
                A.to_owned(),
                "a.md".to_owned(),
                "a".to_owned(),
                "[[b]] [[gone]]\n".to_owned(),
            ),
            NoteSource::new(
                B.to_owned(),
                "b.md".to_owned(),
                "b".to_owned(),
                "body\n".to_owned(),
            ),
        ]);
        let plan = plan_rename(&index, B, "moved/b.md").expect("plan");
        let commit = plan_commit(
            &index,
            &plan,
            &[revision(A, 1), revision(B, 1)],
            &[revision(A, 1), revision(B, 1)],
            &[],
            false,
        );
        assert_eq!(commit.untouched.len(), plan.untouched.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::NoteSource;

    const A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

    #[test]
    fn special_filenames_keep_references_resolvable_through_identity() {
        for destination in [
            "folder/New#name.md",
            "folder/New]name.md",
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.md",
        ] {
            let sources = vec![
                NoteSource::new(A, "Source.md", "Source", "[[Old|alias]] ![[Old]]"),
                NoteSource::new(B, "Old.md", "Old", "body"),
            ];
            let index = VaultIndex::rebuild(&sources);
            let plan = plan_rename(&index, B, destination).unwrap();
            assert_eq!(plan.edits.len(), 2);
            assert!(plan.edits.iter().all(|e| e.after.contains(B)));
            let rewritten = apply_plan(&index, &plan);
            let next = VaultIndex::rebuild(&[
                NoteSource::new(A, "Source.md", "Source", &rewritten[0].2),
                NoteSource::new(B, destination, "New", "body"),
            ]);
            assert!(next
                .note(A)
                .unwrap()
                .links
                .iter()
                .all(|l| l.resolved.target_id.as_deref() == Some(B)));
        }
    }
    const C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

    fn index(sources: Vec<NoteSource>) -> VaultIndex {
        VaultIndex::rebuild(&sources)
    }

    #[test]
    fn a_plan_rewrites_links_that_resolve_to_the_note_and_keeps_the_label() {
        let sources = vec![
            NoteSource::new(A, "notes/old.md", "old", "body\n"),
            NoteSource::new(
                B,
                "notes/src.md",
                "src",
                "see [[notes/old|Alias]] and [[notes/old#Head]]\n",
            ),
        ];
        let plan = plan_rename(&index(sources), A, "notes/new.md").expect("plan");
        assert!(plan.keeps_identity);
        assert_eq!(plan.from_path, "notes/old.md");
        assert_eq!(plan.to_path, "notes/new.md");
        assert_eq!(plan.edits.len(), 2);
        assert_eq!(plan.edits[0].after, "[[notes/new|Alias]]");
        assert_eq!(plan.edits[1].after, "[[notes/new#Head]]");
        assert!(plan.untouched.is_empty());
    }

    #[test]
    fn prose_containing_the_old_title_is_never_rewritten() {
        let sources = vec![
            NoteSource::new(A, "notes/old.md", "old", "body\n"),
            NoteSource::new(
                B,
                "notes/src.md",
                "src",
                "notes/old is mentioned in prose\n",
            ),
        ];
        let plan = plan_rename(&index(sources), A, "notes/new.md").expect("plan");
        assert!(plan.edits.is_empty(), "no link, no edit");
    }

    #[test]
    fn an_ambiguous_reference_is_reported_and_left_alone() {
        let sources = vec![
            NoteSource::new(A, "notes/dup.md", "Dup", "one\n"),
            NoteSource::new(B, "other/dup.md", "Dup", "two\n"),
            NoteSource::new(C, "notes/src.md", "src", "[[Dup]]\n"),
        ];
        let plan = plan_rename(&index(sources), A, "notes/renamed.md").expect("plan");
        assert!(plan.edits.is_empty());
        assert_eq!(plan.untouched.len(), 1);
        assert_eq!(
            plan.untouched[0].reason,
            RenameWarningReason::AmbiguousTarget
        );
    }

    #[test]
    fn an_identity_link_is_left_untouched_because_it_already_survives() {
        let sources = vec![
            NoteSource::new(A, "notes/old.md", "old", "body\n"),
            NoteSource::new(
                B,
                "notes/src.md",
                "src",
                "[[aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa]]\n",
            ),
        ];
        let plan = plan_rename(&index(sources), A, "notes/new.md").expect("plan");
        assert!(plan.edits.is_empty());
        assert!(plan.untouched.is_empty());
    }

    #[test]
    fn applying_a_plan_rewrites_only_the_recorded_ranges() {
        let sources = vec![
            NoteSource::new(A, "notes/old.md", "old", "body\n"),
            NoteSource::new(
                B,
                "notes/src.md",
                "src",
                "keep [[notes/old]] and [[other]]\n",
            ),
        ];
        let index = index(sources);
        let plan = plan_rename(&index, A, "notes/new.md").expect("plan");
        let applied = apply_plan(&index, &plan);
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].2, "keep [[notes/new]] and [[other]]\n");
    }

    #[test]
    fn an_embed_keeps_its_bang_and_its_anchor() {
        let sources = vec![
            NoteSource::new(A, "notes/old.md", "old", "# Head\n\nbody\n"),
            NoteSource::new(B, "notes/src.md", "src", "![[notes/old#Head]]\n"),
        ];
        let index = index(sources);
        let plan = plan_rename(&index, A, "notes/new.md").expect("plan");
        assert_eq!(plan.edits[0].after, "![[notes/new#Head]]");
        let applied = apply_plan(&index, &plan);
        assert_eq!(applied[0].2, "![[notes/new#Head]]\n");
    }

    #[test]
    fn a_link_target_uses_the_readable_path_without_the_extension() {
        assert_eq!(link_target_for("notes/new.md"), "notes/new");
        assert_eq!(link_target_for("notes\\new.MD"), "notes/new");
        assert_eq!(link_target_for("plain"), "plain");
    }
}
