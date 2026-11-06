//! Tag rename and merge planning (F08-A03).
//!
//! A tag lives in two places — a `#tag` in the body and a `tags:` entry in the
//! frontmatter — and a rename must rewrite both or the note ends up with a tag it
//! claims to have removed. This module plans that rewrite without touching a file:
//!
//! * a body tag is replaced only where `extract_tags` reported it, so a `#tag`
//!   inside a code fence or inline code is never rewritten (F08-R01);
//! * a rename onto an existing tag is a **merge**, and the plan says how many
//!   losing occurrences would collapse so the caller can report it;
//! * one note changing elsewhere must not abort the others: each note is planned
//!   independently and a stale one is reported next to the rest (F08-R04).
//!
//! Nothing here reads or writes a file, and a tag removal never deletes a note
//! (F08-R03).

use crate::tags::{extract_tags, validate_tag, TagIssue, TagOccurrence, TagSource};
use serde::{Deserialize, Serialize};

/// One note's rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRewrite {
    pub note_id: String,
    pub relative_path: String,
    /// The source after the rewrite.
    pub source: String,
    /// Occurrences changed in this note.
    pub changed: usize,
    /// The revision the caller read, so the save can detect a conflict.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TagPlanIssue {
    SourceUnsupported {
        note_id: String,
    },
    /// The new tag is empty or too long, with the reason as a message key.
    InvalidTag {
        reason: String,
    },
    /// The note changed since the plan was made (VERSION_CONFLICT).
    RevisionChanged {
        note_id: String,
        expected: u64,
        actual: u64,
    },
    /// The caller never read this note, so it cannot be saved safely.
    RevisionUnknown {
        note_id: String,
    },
    /// The old and new tag are the same once trimmed.
    NoChange,
}

/// The whole rename plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRenamePlan {
    pub old_tag: String,
    pub new_tag: String,
    /// True when the new tag already existed somewhere, so this is a merge.
    pub is_merge: bool,
    /// Notes whose source changes.
    pub rewrites: Vec<TagRewrite>,
    /// Notes that could not be planned, with the reason.
    pub conflicted: Vec<(String, TagPlanIssue)>,
    /// Total occurrences that change across every note.
    pub total_changes: usize,
}

impl TagRenamePlan {
    /// True when at least one note can be written.
    pub fn has_changes(&self) -> bool {
        !self.rewrites.is_empty()
    }
}

/// A note as the caller read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagNote {
    pub note_id: String,
    pub relative_path: String,
    pub source: String,
    pub revision: u64,
}

/// Rewrites one note's occurrences of `old_tag` to `new_tag`.
///
/// Frontmatter occurrences change the `tags:` value; body occurrences change the
/// `#tag` text in place. Occurrences are applied back to front so an earlier
/// offset stays valid.
fn rewrite_note(source: &str, old_tag: &str, new_tag: &str) -> Result<(String, usize), ()> {
    // `extract_tags` reports what the parser found, including occurrences the
    // caller must not rewrite; each one is re-checked against the masked source so
    // a `#tag` inside a fence or inline code can never be replaced (F08-R01).
    let masked = crate::scan::Scanner::new(source).masked_source();
    let occurrences: Vec<TagOccurrence> = extract_tags(source)
        .into_iter()
        .filter(|occurrence| occurrence.tag == old_tag)
        .filter(|occurrence| {
            occurrence.source == TagSource::Frontmatter
                || masked
                    .get(occurrence.offset..occurrence.offset + 1 + occurrence.tag.len())
                    .is_some_and(|slice| slice.starts_with('#'))
        })
        .collect();
    if occurrences.is_empty() {
        return Ok((source.to_owned(), 0));
    }
    if occurrences.iter().any(|t| t.source == TagSource::Body)
        && !extract_tags(&format!("#{new_tag}"))
            .iter()
            .any(|t| t.tag == new_tag)
    {
        return Err(());
    }
    let mut text = source.to_owned();
    let mut changed = 0usize;
    // Body occurrences carry the offset of the `#`; the tag follows it. They are
    // applied first because the frontmatter rewrite changes the length of the
    // prefix, which would invalidate every body offset if done in the other order.
    let mut body: Vec<&TagOccurrence> = occurrences
        .iter()
        .filter(|occurrence| occurrence.source == TagSource::Body)
        .collect();
    body.sort_by(|left, right| right.offset.cmp(&left.offset));
    for occurrence in &body {
        let start = occurrence.offset + 1;
        let end = start + occurrence.tag.len();
        if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        // Only rewrite when the bytes still spell the tag, so a shifted offset
        // cannot corrupt an unrelated word.
        if text.get(start..end) == Some(old_tag) {
            text.replace_range(start..end, new_tag);
            changed += 1;
        }
    }
    if occurrences
        .iter()
        .any(|occurrence| occurrence.source == TagSource::Frontmatter)
    {
        let (next, frontmatter_changes) = rewrite_frontmatter(&text, old_tag, new_tag)?;
        text = next;
        changed += frontmatter_changes;
    }
    verified_rewrite(text, changed, old_tag, new_tag)
}

/// Only the supported top-level tag property changes; unknown YAML stays byte-identical.
fn rewrite_frontmatter(source: &str, old_tag: &str, new_tag: &str) -> Result<(String, usize), ()> {
    use yaml_rust2::{Yaml, YamlLoader};
    fn values(value: &Yaml, out: &mut Vec<String>) -> Result<(), ()> {
        match value {
            Yaml::String(text) => out.extend(
                text.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            ),
            Yaml::Array(items) => {
                for item in items {
                    values(item, out)?;
                }
            }
            _ => return Err(()),
        }
        Ok(())
    }
    let scanner = crate::scan::Scanner::new(source);
    let crate::scan::Frontmatter::Closed(span) = scanner.frontmatter() else {
        return Ok((source.to_owned(), 0));
    };
    let documents = YamlLoader::load_from_str(&source[span.start..span.end]).map_err(|_| ())?;
    let Some(Yaml::Hash(entries)) = documents.first() else {
        return Err(());
    };
    let mut text = source.to_owned();
    let mut changed = 0;
    for name in ["tags", "tag"] {
        let Some(value) = entries.get(&Yaml::String(name.into())) else {
            continue;
        };
        let mut tags = Vec::new();
        values(value, &mut tags)?;
        let count = tags.iter().filter(|tag| tag.as_str() == old_tag).count();
        if count == 0 {
            continue;
        }
        let mut next = Vec::new();
        for tag in tags {
            let tag = if tag == old_tag {
                new_tag.to_owned()
            } else {
                tag
            };
            if !tag.is_empty() && !next.contains(&tag) {
                next.push(tag)
            }
        }
        let rendered = if matches!(value, Yaml::String(_)) && next.len() == 1 {
            crate::properties::PropertyValue::Text(next[0].clone())
        } else {
            crate::properties::PropertyValue::MultiSelect(next)
        };
        text = crate::property_write::set_property(&text, name, &rendered)
            .map_err(|_| ())?
            .source;
        changed += count;
    }
    Ok((text, changed))
}
fn verified_rewrite(
    source: String,
    changed: usize,
    old_tag: &str,
    new_tag: &str,
) -> Result<(String, usize), ()> {
    let tags = extract_tags(&source);
    if changed > 0
        && (tags.iter().any(|t| t.tag == old_tag)
            || (!new_tag.is_empty() && !tags.iter().any(|t| t.tag == new_tag)))
    {
        return Err(());
    }
    Ok((source, changed))
}

/// Plans a tag rename across the given notes.
///
/// `actual_revisions` is what the vault reports now; a note that changed since the
/// caller read it is reported as conflicted and left out of the rewrites, so the
/// other notes still proceed (F08-R04).
pub fn plan_rename(
    notes: &[TagNote],
    old_tag: &str,
    new_tag: &str,
    actual_revisions: &[(String, u64)],
    existing_tags: &[String],
) -> TagRenamePlan {
    let new_tag = new_tag.trim().to_owned();
    if let Err(issue) = validate_tag(&new_tag) {
        return TagRenamePlan {
            old_tag: old_tag.to_owned(),
            new_tag: new_tag.clone(),
            is_merge: false,
            rewrites: Vec::new(),
            conflicted: vec![(
                String::new(),
                TagPlanIssue::InvalidTag {
                    reason: match issue {
                        TagIssue::Empty => "error.tagEmpty".to_owned(),
                        TagIssue::TooLong => "error.tagTooLong".to_owned(),
                    },
                },
            )],
            total_changes: 0,
        };
    }
    if old_tag.trim() == new_tag {
        return TagRenamePlan {
            old_tag: old_tag.to_owned(),
            new_tag,
            is_merge: false,
            rewrites: Vec::new(),
            conflicted: vec![(String::new(), TagPlanIssue::NoChange)],
            total_changes: 0,
        };
    }
    let is_merge = existing_tags.iter().any(|tag| tag == &new_tag);

    let mut rewrites: Vec<TagRewrite> = Vec::new();
    let mut conflicted: Vec<(String, TagPlanIssue)> = Vec::new();
    let mut total_changes = 0usize;
    for note in notes {
        match actual_revisions.iter().find(|(id, _)| id == &note.note_id) {
            None => {
                conflicted.push((
                    note.note_id.clone(),
                    TagPlanIssue::RevisionUnknown {
                        note_id: note.note_id.clone(),
                    },
                ));
                continue;
            }
            Some((_, actual)) if *actual != note.revision => {
                conflicted.push((
                    note.note_id.clone(),
                    TagPlanIssue::RevisionChanged {
                        note_id: note.note_id.clone(),
                        expected: note.revision,
                        actual: *actual,
                    },
                ));
                continue;
            }
            _ => {}
        }
        let Ok((source, changed)) = rewrite_note(&note.source, old_tag, &new_tag) else {
            conflicted.push((
                note.note_id.clone(),
                TagPlanIssue::SourceUnsupported {
                    note_id: note.note_id.clone(),
                },
            ));
            continue;
        };
        if changed == 0 {
            continue;
        }
        total_changes += changed;
        rewrites.push(TagRewrite {
            note_id: note.note_id.clone(),
            relative_path: note.relative_path.clone(),
            source,
            changed,
            expected_revision: note.revision,
        });
    }
    rewrites.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    TagRenamePlan {
        old_tag: old_tag.to_owned(),
        new_tag,
        is_merge,
        rewrites,
        conflicted,
        total_changes,
    }
}

/// Removes a tag from every note without touching note identity (F08-R03).
///
/// This reuses the rename machinery with an empty replacement, so a removal and a
/// rename cannot drift apart in behaviour.
pub fn plan_removal(
    notes: &[TagNote],
    tag: &str,
    actual_revisions: &[(String, u64)],
) -> TagRenamePlan {
    let mut plan = TagRenamePlan {
        old_tag: tag.to_owned(),
        new_tag: String::new(),
        is_merge: false,
        rewrites: Vec::new(),
        conflicted: Vec::new(),
        total_changes: 0,
    };
    for note in notes {
        match actual_revisions.iter().find(|(id, _)| id == &note.note_id) {
            None => {
                plan.conflicted.push((
                    note.note_id.clone(),
                    TagPlanIssue::RevisionUnknown {
                        note_id: note.note_id.clone(),
                    },
                ));
                continue;
            }
            Some((_, actual)) if *actual != note.revision => {
                plan.conflicted.push((
                    note.note_id.clone(),
                    TagPlanIssue::RevisionChanged {
                        note_id: note.note_id.clone(),
                        expected: note.revision,
                        actual: *actual,
                    },
                ));
                continue;
            }
            _ => (),
        }
        match remove_tag(&note.source, tag) {
            Ok((source, changed)) if changed > 0 => plan.rewrites.push(TagRewrite {
                note_id: note.note_id.clone(),
                relative_path: note.relative_path.clone(),
                source,
                changed,
                expected_revision: note.revision,
            }),
            Ok(_) => (),
            Err(_) => plan.conflicted.push((
                note.note_id.clone(),
                TagPlanIssue::SourceUnsupported {
                    note_id: note.note_id.clone(),
                },
            )),
        }
    }
    plan.rewrites
        .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    plan.total_changes = plan.rewrites.iter().map(|r| r.changed).sum();
    plan
}

/// Removes every occurrence of a tag from one note's source.
fn remove_tag(source: &str, tag: &str) -> Result<(String, usize), ()> {
    let occurrences: Vec<TagOccurrence> = extract_tags(source)
        .into_iter()
        .filter(|occurrence| occurrence.tag == tag)
        .collect();
    if occurrences.is_empty() {
        return Ok((source.to_owned(), 0));
    }
    let mut text = source.to_owned();
    let mut changed = 0usize;
    let mut body: Vec<&TagOccurrence> = occurrences
        .iter()
        .filter(|occurrence| occurrence.source == TagSource::Body)
        .collect();
    body.sort_by(|left, right| right.offset.cmp(&left.offset));
    for occurrence in &body {
        let start = occurrence.offset;
        let end = start + 1 + occurrence.tag.len();
        if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        if text.get(start + 1..end) != Some(tag) {
            continue;
        }
        // Remove one preceding space too, so the sentence does not end with a
        // trailing space where the tag used to be.
        let from = if start > 0 && text.as_bytes().get(start - 1) == Some(&b' ') {
            start - 1
        } else {
            start
        };
        text.replace_range(from..end, "");
        changed += 1;
    }
    if occurrences
        .iter()
        .any(|occurrence| occurrence.source == TagSource::Frontmatter)
    {
        let (next, frontmatter_changes) = rewrite_frontmatter(&text, tag, "")?;
        text = next;
        changed += frontmatter_changes;
    }
    verified_rewrite(text, changed, tag, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_preserves_unrelated_yaml_and_body_lines_and_crlf() {
        let source="\u{feff}---\r\ntags: [old, keep]\r\ncustom:\r\n  tags: old\r\n  text: |\r\n    -\r\n---\r\n-\r\n`#old`\r\n#old\r\n";
        let plan = plan_removal(
            &[note("n1", "a.md", source, 1)],
            "old",
            &[revision("n1", 1)],
        );
        assert!(plan.conflicted.is_empty());
        let next = &plan.rewrites[0].source;
        assert!(next.contains("custom:\r\n  tags: old\r\n  text: |\r\n    -\r\n"));
        assert!(next.contains("---\r\n-\r\n`#old`\r\n"));
        assert!(next.starts_with('\u{feff}'));
        assert!(!extract_tags(next).iter().any(|t| t.tag == "old"));
    }

    #[test]
    fn a_merge_deduplicates_frontmatter_values_without_rewriting_unknown_fields() {
        let source = "---\ntags: [old, new, keep]\ncustom:  'exact' # comment\n---\n#old #new\n";
        let plan = plan_rename(
            &[note("n1", "a.md", source, 1)],
            "old",
            "new",
            &[revision("n1", 1)],
            &["new".into()],
        );
        let next = &plan.rewrites[0].source;
        assert_eq!(
            extract_tags(next)
                .iter()
                .filter(|t| t.source == TagSource::Frontmatter && t.tag == "new")
                .count(),
            1
        );
        assert!(next.contains("custom:  'exact' # comment\n"));
    }

    #[test]
    fn unsupported_frontmatter_and_body_targets_pause_only_the_affected_note() {
        let notes = vec![
            note(
                "bad",
                "bad.md",
                "---\ntags: &shared [old]\ncustom: *shared\n---\n#old\n",
                1,
            ),
            note("good", "good.md", "#old\n", 1),
        ];
        let plan = plan_rename(
            &notes,
            "old",
            "new",
            &[revision("bad", 1), revision("good", 1)],
            &[],
        );
        assert_eq!(plan.rewrites.len(), 1);
        assert_eq!(plan.rewrites[0].note_id, "good");
        assert!(matches!(
            plan.conflicted[0].1,
            TagPlanIssue::SourceUnsupported { .. }
        ));
        let plan = plan_rename(
            &[note("body", "body.md", "#old\n", 1)],
            "old",
            "new tag",
            &[revision("body", 1)],
            &[],
        );
        assert!(plan.rewrites.is_empty());
        assert!(matches!(
            plan.conflicted[0].1,
            TagPlanIssue::SourceUnsupported { .. }
        ));
    }

    fn note(id: &str, path: &str, source: &str, revision: u64) -> TagNote {
        TagNote {
            note_id: id.to_owned(),
            relative_path: path.to_owned(),
            source: source.to_owned(),
            revision,
        }
    }

    fn revision(id: &str, value: u64) -> (String, u64) {
        (id.to_owned(), value)
    }

    #[test]
    fn a_body_tag_is_renamed_in_place() {
        let notes = vec![note("n1", "a.md", "Text #old more\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert_eq!(plan.rewrites.len(), 1);
        assert_eq!(plan.rewrites[0].source, "Text #new more\n");
        assert_eq!(plan.total_changes, 1);
        assert!(!plan.is_merge);
    }

    #[test]
    fn a_tag_inside_a_code_fence_is_not_rewritten() {
        // F08-R01: code text is not a tag.
        let notes = vec![note("n1", "a.md", "```\n#old\n```\n\n#old\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert_eq!(plan.rewrites.len(), 1);
        let source = &plan.rewrites[0].source;
        assert!(
            source.contains("#old\n"),
            "the fenced occurrence must stay: {source}"
        );
        assert!(source.ends_with("#new\n"));
        assert_eq!(plan.total_changes, 1);
    }

    #[test]
    fn a_fence_that_shares_a_line_with_text_is_not_a_fence() {
        // `#old and ``` opens no fence, so the following `#old` IS a tag. This
        // pins the behaviour the debug probe uncovered: the parser is right, and a
        // test input pretending otherwise would be the bug.
        let notes = vec![note("n1", "a.md", "#old and ```\n#old\n```\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert_eq!(plan.total_changes, 2);
    }

    #[test]
    fn a_frontmatter_list_entry_is_renamed() {
        let notes = vec![note(
            "n1",
            "a.md",
            "---\ntags:\n  - old\n  - keep\n---\nbody\n",
            1,
        )];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert_eq!(plan.rewrites.len(), 1);
        let source = &plan.rewrites[0].source;
        assert!(extract_tags(source).iter().any(|t| t.tag == "new"));
        assert!(extract_tags(source).iter().any(|t| t.tag == "keep"));
        assert!(!source.contains("- old"));
    }

    #[test]
    fn an_inline_frontmatter_list_is_renamed() {
        let notes = vec![note("n1", "a.md", "---\ntags: [old, keep]\n---\nbody\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert!(plan.rewrites[0].source.contains("tags: [new, keep]"));
    }

    #[test]
    fn a_scalar_frontmatter_tag_is_renamed() {
        let notes = vec![note("n1", "a.md", "---\ntags: old\n---\nbody\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert!(plan.rewrites[0].source.contains("tags: new"));
    }

    #[test]
    fn a_prefix_of_another_tag_is_not_touched() {
        // Only a whole tag value is replaced.
        let notes = vec![note("n1", "a.md", "#older #old\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert!(plan.rewrites[0].source.contains("#older"));
        assert!(plan.rewrites[0].source.ends_with("#new\n"));
        assert_eq!(plan.total_changes, 1);
    }

    #[test]
    fn renaming_onto_an_existing_tag_is_reported_as_a_merge() {
        let notes = vec![note("n1", "a.md", "#old #existing\n", 1)];
        let plan = plan_rename(
            &notes,
            "old",
            "existing",
            &[revision("n1", 1)],
            &["existing".to_owned()],
        );
        assert!(plan.is_merge);
        // The rewrite still happens; the caller reports the merge.
        assert_eq!(plan.rewrites.len(), 1);
        assert_eq!(plan.rewrites[0].source, "#existing #existing\n");
    }

    #[test]
    fn a_changed_note_is_conflicted_while_the_others_proceed() {
        // F08-R04: one stale note must not abort the rest.
        let notes = vec![
            note("n1", "a.md", "#old\n", 1),
            note("n2", "b.md", "#old\n", 5),
        ];
        let plan = plan_rename(
            &notes,
            "old",
            "new",
            &[revision("n1", 1), revision("n2", 6)],
            &[],
        );
        assert_eq!(plan.rewrites.len(), 1);
        assert_eq!(plan.rewrites[0].note_id, "n1");
        assert_eq!(plan.conflicted.len(), 1);
        assert!(matches!(
            plan.conflicted[0].1,
            TagPlanIssue::RevisionChanged { .. }
        ));
    }

    #[test]
    fn a_note_never_read_is_conflicted() {
        let notes = vec![note("n1", "a.md", "#old\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[], &[]);
        assert!(plan.rewrites.is_empty());
        assert!(matches!(
            plan.conflicted[0].1,
            TagPlanIssue::RevisionUnknown { .. }
        ));
    }

    #[test]
    fn an_invalid_or_unchanged_target_is_refused() {
        let notes = vec![note("n1", "a.md", "#old\n", 1)];
        let invalid = plan_rename(&notes, "old", "   ", &[revision("n1", 1)], &[]);
        assert!(matches!(
            invalid.conflicted[0].1,
            TagPlanIssue::InvalidTag { .. }
        ));
        let same = plan_rename(&notes, "old", "old", &[revision("n1", 1)], &[]);
        assert!(matches!(same.conflicted[0].1, TagPlanIssue::NoChange));
        assert!(same.rewrites.is_empty());
    }

    #[test]
    fn a_note_without_the_tag_produces_no_rewrite() {
        let notes = vec![note("n1", "a.md", "#other\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert!(plan.rewrites.is_empty());
        assert_eq!(plan.total_changes, 0);
        assert!(!plan.has_changes());
    }

    #[test]
    fn removing_a_body_tag_keeps_the_note_and_tidies_the_space() {
        // F08-R03: a tag removal never deletes the note.
        let notes = vec![note("n1", "a.md", "Text #old more\n", 1)];
        let plan = plan_removal(&notes, "old", &[revision("n1", 1)]);
        assert_eq!(plan.rewrites.len(), 1);
        assert_eq!(plan.rewrites[0].source, "Text more\n");
    }

    #[test]
    fn removing_a_frontmatter_tag_drops_the_line() {
        let notes = vec![note(
            "n1",
            "a.md",
            "---\ntags:\n  - old\n  - keep\n---\nbody\n",
            1,
        )];
        let plan = plan_removal(&notes, "old", &[revision("n1", 1)]);
        let source = &plan.rewrites[0].source;
        assert!(extract_tags(source).iter().any(|t| t.tag == "keep"));
        assert!(!source.contains("- old"));
    }

    #[test]
    fn several_occurrences_in_one_note_are_all_counted() {
        let notes = vec![note("n1", "a.md", "#old and #old again\n", 1)];
        let plan = plan_rename(&notes, "old", "new", &[revision("n1", 1)], &[]);
        assert_eq!(plan.total_changes, 2);
        assert_eq!(plan.rewrites[0].source, "#new and #new again\n");
    }

    #[test]
    fn rewrites_are_ordered_by_path_for_a_deterministic_plan() {
        let notes = vec![
            note("n2", "b.md", "#old\n", 1),
            note("n1", "a.md", "#old\n", 1),
        ];
        let plan = plan_rename(
            &notes,
            "old",
            "new",
            &[revision("n1", 1), revision("n2", 1)],
            &[],
        );
        assert_eq!(
            plan.rewrites
                .iter()
                .map(|rewrite| rewrite.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.md", "b.md"]
        );
    }

    #[test]
    fn a_unicode_tag_is_renamed_correctly() {
        let notes = vec![note("n1", "a.md", "#旧标签 文本\n", 1)];
        let plan = plan_rename(&notes, "旧标签", "新标签", &[revision("n1", 1)], &[]);
        assert_eq!(plan.rewrites[0].source, "#新标签 文本\n");
        assert_eq!(plan.total_changes, 1);
    }
}
