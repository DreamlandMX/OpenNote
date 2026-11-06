//! F10 link target resolution against a note catalog.

use crate::headings::anchors_of;
use crate::links::RawLink;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The minimum M02 read projection M04 needs: identity, readable path, title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRef {
    pub note_id: String,
    pub relative_path: String,
    pub title: String,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    notes: Vec<NoteRef>,
    by_id: BTreeMap<String, usize>,
    by_path: BTreeMap<String, Vec<usize>>,
    by_title: BTreeMap<String, Vec<usize>>,
}

impl Catalog {
    /// Builds the lookup maps once. Resolution is the hot path during a vault
    /// rebuild, so it must never scan every note per link.
    pub fn new(mut notes: Vec<NoteRef>) -> Self {
        notes.sort_by(|left, right| {
            normalize_path(&left.relative_path)
                .cmp(&normalize_path(&right.relative_path))
                .then_with(|| left.note_id.cmp(&right.note_id))
        });
        let mut by_id = BTreeMap::new();
        let mut by_path: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_title: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, note) in notes.iter().enumerate() {
            by_id.insert(note.note_id.clone(), index);
            by_path
                .entry(normalize_path(&note.relative_path))
                .or_default()
                .push(index);
            by_title
                .entry(normalize_title(&note.title))
                .or_default()
                .push(index);
        }
        Self {
            notes,
            by_id,
            by_path,
            by_title,
        }
    }

    pub fn notes(&self) -> &[NoteRef] {
        &self.notes
    }

    pub fn by_id(&self, note_id: &str) -> Option<&NoteRef> {
        self.by_id.get(note_id).map(|index| &self.notes[*index])
    }

    /// Exact path matches (extension and separator spelling normalized).
    pub fn by_path(&self, path: &str) -> Vec<&NoteRef> {
        self.by_path
            .get(&normalize_path(path))
            .map(|indexes| indexes.iter().map(|index| &self.notes[*index]).collect())
            .unwrap_or_default()
    }

    /// Title matches, used only when the target carries no usable path.
    pub fn by_title(&self, title: &str) -> Vec<&NoteRef> {
        self.by_title
            .get(&normalize_title(title))
            .map(|indexes| indexes.iter().map(|index| &self.notes[*index]).collect())
            .unwrap_or_default()
    }
}

pub(crate) fn normalize_title(title: &str) -> String {
    title.trim().to_lowercase()
}

pub(crate) fn normalize_path(path: &str) -> String {
    let mut value = path.trim().replace('\\', "/");
    while let Some(rest) = value.strip_prefix("./") {
        value = rest.to_owned();
    }
    if value.to_lowercase().ends_with(".md") {
        value.truncate(value.len() - 3);
    }
    value.trim_end_matches('/').to_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    Resolved,
    /// Several notes share the name and no path disambiguates them
    /// (F10-A02); the caller must show the candidates.
    AmbiguousTarget,
    /// No note matches; the link text stays in the source (F10-R02).
    BrokenLink,
    /// The note exists but the heading/block anchor does not (F11-A03).
    AnchorMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLink {
    pub target_id: Option<String>,
    pub target_path: Option<String>,
    pub anchor: Option<String>,
    pub status: LinkStatus,
    /// Readable paths of the competing notes, sorted; empty unless ambiguous.
    pub candidates: Vec<String>,
}

impl ResolvedLink {
    fn unresolved(status: LinkStatus, anchor: Option<String>, candidates: Vec<String>) -> Self {
        Self {
            target_id: None,
            target_path: None,
            anchor,
            status,
            candidates,
        }
    }
}

/// Resolves one link against `catalog` and the current sources of every note.
///
/// `sources` maps note id to its current source text and is used only to check
/// the anchor; an unknown source leaves the anchor unchecked.
pub fn resolve_link(
    catalog: &Catalog,
    sources: &BTreeMap<String, String>,
    source_note_id: &str,
    link: &RawLink,
) -> ResolvedLink {
    let mut resolved = resolve_target(catalog, source_note_id, &link.target);
    if resolved.status != LinkStatus::Resolved {
        return resolved;
    }
    let Some(anchor) = link.anchor.as_deref() else {
        return resolved;
    };
    let Some(target_id) = resolved.target_id.as_deref() else {
        return resolved;
    };
    if let Some(source) = sources.get(target_id) {
        if !anchor_matches(source, anchor) {
            resolved.status = LinkStatus::AnchorMissing;
        }
    }
    resolved
}

/// Exact (case-insensitive for headings) anchor membership; there is no fuzzy
/// fallback, so a deleted anchor can never silently point at a similar
/// paragraph (F11-A03).
pub fn anchor_matches(source: &str, anchor: &str) -> bool {
    anchor_in(&anchors_of(source), anchor)
}

/// Same rule as [`anchor_matches`] but against a precomputed vocabulary, which
/// is what a vault-wide rebuild uses so each source is parsed once.
pub fn anchor_in(anchors: &[String], anchor: &str) -> bool {
    let wanted = anchor.trim().to_lowercase();
    anchors
        .iter()
        .any(|candidate| candidate == anchor.trim() || candidate.to_lowercase() == wanted)
}

/// Resolves a link using a precomputed anchor vocabulary (note id to anchors).
pub fn resolve_link_with_anchors(
    catalog: &Catalog,
    anchors: &BTreeMap<String, Vec<String>>,
    source_note_id: &str,
    link: &RawLink,
) -> ResolvedLink {
    let mut resolved = resolve_target(catalog, source_note_id, &link.target);
    if resolved.status != LinkStatus::Resolved {
        return resolved;
    }
    let Some(anchor) = link.anchor.as_deref() else {
        return resolved;
    };
    let Some(target_id) = resolved.target_id.as_deref() else {
        return resolved;
    };
    if let Some(list) = anchors.get(target_id) {
        if !anchor_in(list, anchor) {
            resolved.status = LinkStatus::AnchorMissing;
        }
    }
    resolved
}

fn resolve_target(catalog: &Catalog, source_note_id: &str, raw_target: &str) -> ResolvedLink {
    let target = raw_target.trim();
    if target.is_empty() {
        // `[[#anchor]]` refers to the note that contains it.
        let note = catalog.by_id(source_note_id);
        return ResolvedLink {
            target_id: Some(source_note_id.to_owned()),
            target_path: note.map(|note| note.relative_path.clone()),
            anchor: None,
            status: LinkStatus::Resolved,
            candidates: Vec::new(),
        };
    }
    if uuid::Uuid::parse_str(target).is_ok_and(|parsed| parsed.to_string() == target) {
        return match catalog.by_id(target) {
            Some(note) => ResolvedLink {
                target_id: Some(note.note_id.clone()),
                target_path: Some(note.relative_path.clone()),
                anchor: None,
                status: LinkStatus::Resolved,
                candidates: Vec::new(),
            },
            None => ResolvedLink::unresolved(LinkStatus::BrokenLink, None, Vec::new()),
        };
    }
    let by_path = catalog.by_path(target);
    if by_path.len() == 1 {
        let note = by_path[0];
        return ResolvedLink {
            target_id: Some(note.note_id.clone()),
            target_path: Some(note.relative_path.clone()),
            anchor: None,
            status: LinkStatus::Resolved,
            candidates: Vec::new(),
        };
    }
    if by_path.len() > 1 {
        return ambiguous(by_path);
    }
    let by_title = catalog.by_title(target);
    match by_title.len() {
        0 => ResolvedLink::unresolved(LinkStatus::BrokenLink, None, Vec::new()),
        1 => {
            let note = by_title[0];
            ResolvedLink {
                target_id: Some(note.note_id.clone()),
                target_path: Some(note.relative_path.clone()),
                anchor: None,
                status: LinkStatus::Resolved,
                candidates: Vec::new(),
            }
        }
        _ => ambiguous(by_title),
    }
}

fn ambiguous(notes: Vec<&NoteRef>) -> ResolvedLink {
    let candidates: Vec<String> = notes
        .iter()
        .map(|note| note.relative_path.clone())
        .collect();
    ResolvedLink::unresolved(LinkStatus::AmbiguousTarget, None, candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::{extract_links, LinkKind};

    fn catalog() -> Catalog {
        Catalog::new(vec![
            NoteRef {
                note_id: "11111111-1111-4111-8111-111111111111".into(),
                relative_path: "notes/a.md".into(),
                title: "A".into(),
            },
            NoteRef {
                note_id: "22222222-2222-4222-8222-222222222222".into(),
                relative_path: "other/a.md".into(),
                title: "A".into(),
            },
            NoteRef {
                note_id: "33333333-3333-4333-8333-333333333333".into(),
                relative_path: "notes/target.md".into(),
                title: "Target".into(),
            },
        ])
    }

    fn link(source: &str) -> RawLink {
        extract_links(source).into_iter().next().unwrap()
    }

    #[test]
    fn a_path_disambiguates_same_titled_notes() {
        let resolved = resolve_target(&catalog(), "nobody", "notes/a");
        assert_eq!(resolved.status, LinkStatus::Resolved);
        assert_eq!(resolved.target_path.as_deref(), Some("notes/a.md"));
    }

    #[test]
    fn a_bare_title_with_two_matches_reports_candidates() {
        let resolved = resolve_target(&catalog(), "nobody", "A");
        assert_eq!(resolved.status, LinkStatus::AmbiguousTarget);
        assert!(resolved.target_id.is_none());
        assert_eq!(resolved.candidates, vec!["notes/a.md", "other/a.md"]);
    }

    #[test]
    fn identity_links_survive_a_rename() {
        let resolved = resolve_target(&catalog(), "nobody", "33333333-3333-4333-8333-333333333333");
        assert_eq!(resolved.status, LinkStatus::Resolved);
        assert_eq!(
            resolved.target_id.as_deref(),
            Some("33333333-3333-4333-8333-333333333333")
        );
    }

    #[test]
    fn a_deleted_anchor_is_reported_without_a_fuzzy_fallback() {
        let mut sources = BTreeMap::new();
        sources.insert(
            "33333333-3333-4333-8333-333333333333".to_owned(),
            "# Target\n\ntext\n".to_owned(),
        );
        let resolved = resolve_link(&catalog(), &sources, "nobody", &link("[[target#Missing]]"));
        assert_eq!(resolved.status, LinkStatus::AnchorMissing);
        assert!(resolved.target_id.is_some());
        let ok = resolve_link(&catalog(), &sources, "nobody", &link("[[target#Target]]"));
        assert_eq!(ok.status, LinkStatus::Resolved);
    }

    #[test]
    fn a_same_note_anchor_resolves_to_the_source_note() {
        let resolved = resolve_target(&catalog(), "notes/a.md", "");
        assert_eq!(resolved.target_id.as_deref(), Some("notes/a.md"));
        assert_eq!(link("[[#section]]").kind, LinkKind::Link);
    }
}
