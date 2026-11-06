//! Rebuildable derived index over a set of notes: headings, anchors, tags,
//! links, backlinks and embed expansion (F08/F10/F11).
//!
//! Nothing here is persisted: `VaultIndex::rebuild` recomputes everything from
//! the current sources, which is what keeps the index a derived artifact
//! (F08-R02) and what makes an edited embed show new content (F11-A01).

use crate::headings::{extract_block_anchors, extract_headings, BlockAnchor, Heading};
use crate::links::{extract_links, LinkKind, RawLink};
use crate::resolve::{
    resolve_link, resolve_link_with_anchors, Catalog, LinkStatus, NoteRef, ResolvedLink,
};
use crate::scan::lines;
use crate::tags::{extract_tags, TagOccurrence};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// F11 field default: nested embeds fold at depth 3.
pub const EMBED_DEPTH_LIMIT: usize = 3;

/// One note as the index consumes it: identity plus its current source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSource {
    pub note_id: String,
    pub relative_path: String,
    pub title: String,
    pub source: String,
}

impl NoteSource {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedLink {
    pub link: RawLink,
    pub resolved: ResolvedLink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteMetadata {
    pub note_id: String,
    pub relative_path: String,
    pub title: String,
    pub headings: Vec<Heading>,
    pub block_anchors: Vec<BlockAnchor>,
    pub tags: Vec<TagOccurrence>,
    pub links: Vec<ParsedLink>,
    /// Heading slugs, heading text and block ids: everything a link may target.
    pub anchors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backlink {
    pub from_note_id: String,
    pub from_path: String,
    pub kind: LinkKind,
    pub label: Option<String>,
    pub anchor: Option<String>,
    pub offset: usize,
}

/// A link that does not reach exactly one note, or whose anchor is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkIssue {
    pub from_note_id: String,
    pub from_path: String,
    pub target: String,
    /// Visible label from `[[target|label]]`, kept so the issue list can show
    /// what the author actually wrote even when the target is broken.
    pub label: Option<String>,
    pub anchor: Option<String>,
    pub status: LinkStatus,
    pub candidates: Vec<String>,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmbedStatus {
    Ok,
    BrokenLink,
    AmbiguousTarget,
    AnchorMissing,
    /// The embed refers back to a note already on this expansion path.
    Cycle,
    /// Nesting reached [`EMBED_DEPTH_LIMIT`]; the caller renders a link instead.
    DepthExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedExpansion {
    pub status: EmbedStatus,
    pub target_id: Option<String>,
    pub target_path: Option<String>,
    pub anchor: Option<String>,
    pub content: Option<String>,
    /// Note ids expanded on this path, root first. A placeholder can render it.
    pub chain: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct VaultIndex {
    catalog: Catalog,
    texts: BTreeMap<String, String>,
    notes: Vec<NoteMetadata>,
    backlinks: BTreeMap<String, Vec<Backlink>>,
    issues: Vec<LinkIssue>,
    tag_index: BTreeMap<String, Vec<String>>,
}

impl VaultIndex {
    /// Recomputes every derived structure from the given sources.
    ///
    /// Two passes, one parse per source: the first pass collects anchors so the
    /// second can resolve links against a vocabulary instead of re-scanning the
    /// target note for every reference.
    pub fn rebuild(sources: &[NoteSource]) -> Self {
        let catalog = Catalog::new(
            sources
                .iter()
                .map(|note| NoteRef {
                    note_id: note.note_id.clone(),
                    relative_path: note.relative_path.clone(),
                    title: note.title.clone(),
                })
                .collect(),
        );
        let texts: BTreeMap<String, String> = sources
            .iter()
            .map(|note| (note.note_id.clone(), note.source.clone()))
            .collect();
        let mut anchors_by_note: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut parsed: Vec<(
            Vec<Heading>,
            Vec<BlockAnchor>,
            Vec<TagOccurrence>,
            Vec<RawLink>,
        )> = Vec::with_capacity(sources.len());
        for note in sources {
            let headings = extract_headings(&note.source);
            let block_anchors = extract_block_anchors(&note.source);
            let tags = extract_tags(&note.source);
            let links = extract_links(&note.source);
            let mut anchors: Vec<String> = headings
                .iter()
                .flat_map(|heading| [heading.slug.clone(), heading.text.clone()])
                .collect();
            anchors.extend(block_anchors.iter().map(|anchor| anchor.id.clone()));
            anchors_by_note.insert(note.note_id.clone(), anchors);
            parsed.push((headings, block_anchors, tags, links));
        }
        let mut notes = Vec::with_capacity(sources.len());
        let mut backlinks: BTreeMap<String, Vec<Backlink>> = BTreeMap::new();
        let mut issues = Vec::new();
        let mut tag_index: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (note, (headings, block_anchors, tags, raw_links)) in sources.iter().zip(parsed) {
            let anchors = anchors_by_note
                .get(&note.note_id)
                .cloned()
                .unwrap_or_default();
            let links: Vec<ParsedLink> = raw_links
                .into_iter()
                .map(|link| ParsedLink {
                    resolved: resolve_link_with_anchors(
                        &catalog,
                        &anchors_by_note,
                        &note.note_id,
                        &link,
                    ),
                    link,
                })
                .collect();
            for parsed in &links {
                let status = parsed.resolved.status;
                if matches!(status, LinkStatus::Resolved | LinkStatus::AnchorMissing) {
                    if let Some(target_id) = parsed.resolved.target_id.clone() {
                        backlinks.entry(target_id).or_default().push(Backlink {
                            from_note_id: note.note_id.clone(),
                            from_path: note.relative_path.clone(),
                            kind: parsed.link.kind,
                            label: parsed.link.label.clone(),
                            anchor: parsed.link.anchor.clone(),
                            offset: parsed.link.offset,
                        });
                    }
                }
                if status != LinkStatus::Resolved {
                    issues.push(LinkIssue {
                        from_note_id: note.note_id.clone(),
                        from_path: note.relative_path.clone(),
                        target: parsed.link.target.clone(),
                        label: parsed.link.label.clone(),
                        anchor: parsed.link.anchor.clone(),
                        status,
                        candidates: parsed.resolved.candidates.clone(),
                        offset: parsed.link.offset,
                    });
                }
            }
            for tag in &tags {
                let entry = tag_index.entry(tag.tag.clone()).or_default();
                if !entry.contains(&note.note_id) {
                    entry.push(note.note_id.clone());
                }
            }
            notes.push(NoteMetadata {
                note_id: note.note_id.clone(),
                relative_path: note.relative_path.clone(),
                title: note.title.clone(),
                headings,
                block_anchors,
                tags,
                links,
                anchors,
            });
        }
        for list in backlinks.values_mut() {
            list.sort_by(|left, right| {
                left.from_path
                    .cmp(&right.from_path)
                    .then_with(|| left.offset.cmp(&right.offset))
            });
        }
        for list in tag_index.values_mut() {
            list.sort();
        }
        issues.sort_by(|left, right| {
            left.from_path
                .cmp(&right.from_path)
                .then_with(|| left.offset.cmp(&right.offset))
        });
        Self {
            catalog,
            texts,
            notes,
            backlinks,
            issues,
            tag_index,
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn notes(&self) -> &[NoteMetadata] {
        &self.notes
    }

    pub fn note(&self, note_id: &str) -> Option<&NoteMetadata> {
        self.notes.iter().find(|note| note.note_id == note_id)
    }

    /// The current source of a note, as indexed.
    pub fn source(&self, note_id: &str) -> Option<&str> {
        self.texts.get(note_id).map(String::as_str)
    }

    /// Notes referencing `note_id`, in path then offset order.
    pub fn backlinks(&self, note_id: &str) -> &[Backlink] {
        self.backlinks.get(note_id).map_or(&[], Vec::as_slice)
    }

    /// Tag to note ids, both sorted.
    pub fn tags(&self) -> &BTreeMap<String, Vec<String>> {
        &self.tag_index
    }

    /// Every link problem, including missing anchors.
    pub fn issues(&self) -> &[LinkIssue] {
        &self.issues
    }

    /// Broken and ambiguous targets only.
    pub fn broken_links(&self) -> Vec<&LinkIssue> {
        self.issues
            .iter()
            .filter(|issue| {
                matches!(
                    issue.status,
                    LinkStatus::BrokenLink | LinkStatus::AmbiguousTarget
                )
            })
            .collect()
    }

    /// Links whose note exists but whose heading/block anchor does not.
    pub fn missing_anchors(&self) -> Vec<&LinkIssue> {
        self.issues
            .iter()
            .filter(|issue| issue.status == LinkStatus::AnchorMissing)
            .collect()
    }

    /// Expands one embed from `from_note_id`, tracking the path for cycle and
    /// depth detection.
    pub fn expand_embed(&self, from_note_id: &str, link: &RawLink) -> EmbedExpansion {
        let mut chain = vec![from_note_id.to_owned()];
        self.expand_embed_in(&mut chain, from_note_id, link)
    }

    /// Same as [`Self::expand_embed`] but continues an existing path, so a
    /// caller that renders nested embeds can keep cycle detection intact.
    pub fn expand_embed_in(
        &self,
        chain: &mut Vec<String>,
        from_note_id: &str,
        link: &RawLink,
    ) -> EmbedExpansion {
        let resolved = resolve_link(&self.catalog, &self.texts, from_note_id, link);
        let base = EmbedExpansion {
            status: EmbedStatus::Ok,
            target_id: resolved.target_id.clone(),
            target_path: resolved.target_path.clone(),
            anchor: resolved.anchor.clone(),
            content: None,
            chain: chain.clone(),
        };
        let status = match resolved.status {
            LinkStatus::Resolved => EmbedStatus::Ok,
            LinkStatus::BrokenLink => EmbedStatus::BrokenLink,
            LinkStatus::AmbiguousTarget => EmbedStatus::AmbiguousTarget,
            LinkStatus::AnchorMissing => EmbedStatus::AnchorMissing,
        };
        if status != EmbedStatus::Ok {
            return EmbedExpansion { status, ..base };
        }
        let Some(target_id) = resolved.target_id.clone() else {
            return EmbedExpansion {
                status: EmbedStatus::BrokenLink,
                ..base
            };
        };
        if chain.iter().any(|visited| *visited == target_id) {
            return EmbedExpansion {
                status: EmbedStatus::Cycle,
                ..base
            };
        }
        // `chain` holds every note already expanded on this path, root included,
        // so its length is the nesting level the new note would reach.
        if chain.len() >= EMBED_DEPTH_LIMIT {
            return EmbedExpansion {
                status: EmbedStatus::DepthExceeded,
                ..base
            };
        }
        let Some(source) = self.texts.get(&target_id) else {
            return EmbedExpansion {
                status: EmbedStatus::BrokenLink,
                ..base
            };
        };
        let content = match link.anchor.as_deref() {
            None => Some(source.clone()),
            Some(anchor) => content_for_anchor(source, anchor),
        };
        let Some(content) = content else {
            return EmbedExpansion {
                status: EmbedStatus::AnchorMissing,
                ..base
            };
        };
        chain.push(target_id);
        EmbedExpansion {
            status: EmbedStatus::Ok,
            content: Some(content),
            chain: chain.clone(),
            ..base
        }
    }
}

/// The exact content an anchor selects: a heading section, or the paragraph
/// carrying an explicit block id. `None` means the anchor does not exist, which
/// must surface as a placeholder rather than a neighbouring paragraph
/// (F11-A03).
pub fn content_for_anchor(source: &str, anchor: &str) -> Option<String> {
    let wanted = anchor.trim().to_lowercase();
    let headings = extract_headings(source);
    if let Some(index) = headings
        .iter()
        .position(|heading| heading.slug == wanted || heading.text.trim().to_lowercase() == wanted)
    {
        let heading = &headings[index];
        let end = headings
            .iter()
            .skip(index + 1)
            .find(|next| next.level <= heading.level)
            .map_or(source.len(), |next| next.offset);
        return Some(source[heading.offset..end].trim_end().to_owned());
    }
    let blocks = extract_block_anchors(source);
    let block = blocks
        .iter()
        .find(|block| block.id == anchor.trim() || block.id.to_lowercase() == wanted)?;
    let all = lines(source);
    let mut start = block.line;
    while start > 0 && !all[start - 1].text.trim().is_empty() {
        start -= 1;
    }
    let text = &source[all[start].start..all[block.line].end];
    Some(strip_anchor_marker(text))
}

fn strip_anchor_marker(text: &str) -> String {
    let trimmed = text.trim_end();
    match trimmed.rfind('^') {
        Some(caret) => trimmed[..caret].trim_end().to_owned(),
        None => trimmed.to_owned(),
    }
}
