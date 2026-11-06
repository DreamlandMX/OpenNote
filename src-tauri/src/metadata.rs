//! Vault-scoped metadata service: the M11 host adapter over M04.
//!
//! This module owns the boundary between storage and the metadata core. It
//! reads every note source through M02, hands the sources to
//! [`onote_metadata_links::VaultIndex`], and returns derived data to the
//! frontend. Nothing is cached: every call rebuilds from the current sources,
//! so an index can never disagree with the notes on disk.

use std::collections::{BTreeMap, VecDeque};

use onote_domain::{ErrorCode, ServiceError};
use onote_metadata_links::{
    Backlink, EmbedExpansion, LinkIssue, NoteSource, ParsedLink, TagOccurrence, VaultIndex,
};
use onote_vault_storage::VaultStorage;
use serde::{Deserialize, Serialize};

/// The readable title of a note: its file stem, without the `.md` extension.
pub use onote_metadata_links::title_of;

/// Reads every note and rebuilds the index. A note that fails to read is
/// reported instead of being silently skipped, because a missing note would
/// otherwise look like a broken link.
pub fn rebuild_index(
    storage: &VaultStorage,
) -> Result<(VaultIndex, Vec<UnreadableNote>), ServiceError> {
    let summaries = storage.list()?;
    let mut sources = Vec::with_capacity(summaries.len());
    let mut unreadable = Vec::new();
    for summary in summaries {
        match storage.read(&summary.note_id) {
            Ok(document) => sources.push(NoteSource::new(
                summary.note_id,
                summary.relative_path.clone(),
                title_of(&summary.relative_path),
                document.source,
            )),
            Err(error) => unreadable.push(UnreadableNote {
                note_id: summary.note_id,
                relative_path: summary.relative_path,
                code: error.code,
                message_key: error.message_key,
            }),
        }
    }
    Ok((VaultIndex::rebuild(&sources), unreadable))
}

/// A note the index could not read. Reported to the UI so the user can repair
/// it rather than seeing phantom broken links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreadableNote {
    pub note_id: String,
    pub relative_path: String,
    pub code: ErrorCode,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagEntry {
    pub tag: String,
    pub note_ids: Vec<String>,
    pub note_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagReport {
    pub tags: Vec<TagEntry>,
    pub unreadable: Vec<UnreadableNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BacklinkReport {
    pub note_id: String,
    pub backlinks: Vec<Backlink>,
    pub unreadable: Vec<UnreadableNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkReport {
    /// Broken and ambiguous targets.
    pub broken: Vec<LinkIssue>,
    /// Links whose note exists but whose anchor does not.
    pub missing_anchors: Vec<LinkIssue>,
    pub unreadable: Vec<UnreadableNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteMetadataReport {
    pub note_id: String,
    pub relative_path: String,
    pub title: String,
    pub headings: Vec<onote_metadata_links::Heading>,
    pub block_anchors: Vec<onote_metadata_links::BlockAnchor>,
    pub tags: Vec<TagOccurrence>,
    pub links: Vec<ParsedLink>,
    pub backlinks: Vec<Backlink>,
    pub unreadable: Vec<UnreadableNote>,
}

fn canonical_note_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}

fn note_error(code: ErrorCode, message_key: &str, note_id: &str) -> ServiceError {
    ServiceError {
        code,
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
        retryable: false,
        operation_id: Some(note_id.to_owned()),
        retry_after_seconds: None,
    }
}

/// Rejects ids storage could never have issued and ids that no longer name a
/// note, so a mistyped or stale caller gets a typed answer instead of an empty
/// result that looks like a note with no backlinks.
fn validate_note(index: &VaultIndex, note_id: &str) -> Result<(), ServiceError> {
    if !canonical_note_id(note_id) {
        return Err(note_error(
            ErrorCode::InvalidInput,
            "error.invalidNoteId",
            note_id,
        ));
    }
    if index.note(note_id).is_none() {
        return Err(note_error(
            ErrorCode::ObjectNotFound,
            "error.noteMissing",
            note_id,
        ));
    }
    Ok(())
}

/// The chain crosses the command boundary from the client, so its shape is
/// checked before it can influence cycle or depth decisions: canonical ids of
/// notes in this vault only, never longer than the embed limit, and the note
/// being expanded must be the chain's last entry.
fn validate_embed_chain(
    index: &VaultIndex,
    chain: &[String],
    from_note_id: &str,
) -> Result<(), ServiceError> {
    let invalid = || {
        note_error(
            ErrorCode::InvalidInput,
            "error.invalidEmbedChain",
            from_note_id,
        )
    };
    if chain.len() > onote_metadata_links::EMBED_DEPTH_LIMIT {
        return Err(invalid());
    }
    if chain.last().is_some_and(|last| last != from_note_id) {
        return Err(invalid());
    }
    if chain
        .iter()
        .any(|id| !canonical_note_id(id) || index.note(id).is_none())
    {
        return Err(invalid());
    }
    Ok(())
}

/// All tags in the vault with the notes that carry them, sorted by tag.
pub fn tag_report(storage: &VaultStorage) -> Result<TagReport, ServiceError> {
    let (index, unreadable) = rebuild_index(storage)?;
    let mut paths: BTreeMap<&str, &str> = BTreeMap::new();
    for note in index.notes() {
        paths.insert(note.note_id.as_str(), note.relative_path.as_str());
    }
    let tags = index
        .tags()
        .iter()
        .map(|(tag, note_ids)| {
            // Sort by readable path and keep ids aligned with it, so the panel
            // lists notes in a stable, human-meaningful order instead of
            // whatever order the ids happen to hash into.
            let mut entries: Vec<(String, String)> = note_ids
                .iter()
                .map(|id| {
                    (
                        paths
                            .get(id.as_str())
                            .copied()
                            .unwrap_or_default()
                            .to_owned(),
                        id.clone(),
                    )
                })
                .collect();
            entries.sort();
            TagEntry {
                tag: tag.clone(),
                note_paths: entries.iter().map(|(path, _)| path.clone()).collect(),
                note_ids: entries.into_iter().map(|(_, id)| id).collect(),
            }
        })
        .collect();
    Ok(TagReport { tags, unreadable })
}

/// Backlinks pointing at one note.
pub fn backlink_report(
    storage: &VaultStorage,
    note_id: &str,
) -> Result<BacklinkReport, ServiceError> {
    let (index, unreadable) = rebuild_index(storage)?;
    validate_note(&index, note_id)?;
    Ok(BacklinkReport {
        note_id: note_id.to_owned(),
        backlinks: index.backlinks(note_id).to_vec(),
        unreadable,
    })
}

/// F15: one node of the relationship graph.
///
/// Identity is the note id, so renaming a note's title or moving its file does
/// not create a second node (F15-R01). A link whose target does not exist still
/// becomes a node, marked `missing`, so the graph shows the broken edge instead
/// of hiding it (F15-R03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub relative_path: String,
    /// `note` for a real note, `missing` for an unresolved link target.
    pub kind: String,
    /// True for the note the graph was centred on.
    pub anchor: bool,
    /// Distance in links from the anchor; 0 for the anchor itself.
    pub depth: usize,
}

/// F15: one edge. `embed` edges are drawn differently from plain links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// `link` or `embed`.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphReport {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// True when the node limit cut the result, so the UI can say so (F15-R04).
    pub truncated: bool,
    /// Notes the index could not read; the graph is incomplete while non-empty.
    pub unreadable: Vec<UnreadableNote>,
}

/// Local-graph depth limits from F15 §4: one to three hops.
pub const MAX_GRAPH_DEPTH: usize = 3;
/// Default node budget from F15 §4.
pub const DEFAULT_GRAPH_LIMIT: usize = 500;

/// Builds the neighbourhood graph around `focus` (F15 local graph).
///
/// `focus` of `None` means the whole vault (F15 global graph). The walk is
/// breadth-first so the nodes closest to the anchor are kept when the limit is
/// hit, and `truncated` reports that the result is partial rather than silently
/// dropping nodes.
pub fn graph_report(
    storage: &VaultStorage,
    focus: Option<&str>,
    depth: usize,
    limit: usize,
) -> Result<GraphReport, ServiceError> {
    let (index, unreadable) = rebuild_index(storage)?;
    let depth = depth.clamp(1, MAX_GRAPH_DEPTH);
    let limit = if limit == 0 {
        DEFAULT_GRAPH_LIMIT
    } else {
        limit
    };

    // Outgoing edges first, so a walk can follow links forward and, via the
    // backlink index, backwards. Everything is owned here: a link whose target
    // does not resolve still becomes a node keyed by its raw target, so a broken
    // reference is visible in the graph (F15-R03).
    let target_of = |parsed: &onote_metadata_links::ParsedLink| -> String {
        parsed.resolved.target_id.clone().unwrap_or_else(|| {
            format!(
                "missing:{}",
                parsed
                    .resolved
                    .target_path
                    .clone()
                    .unwrap_or_else(|| parsed.link.target.clone())
            )
        })
    };
    let mut outgoing: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut incoming: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for note in index.notes() {
        for parsed in &note.links {
            let kind = match parsed.link.kind {
                onote_metadata_links::LinkKind::Embed => "embed",
                _ => "link",
            };
            let target = target_of(parsed);
            outgoing
                .entry(note.note_id.clone())
                .or_default()
                .push((target.clone(), kind.to_owned()));
            incoming
                .entry(target)
                .or_default()
                .push((note.note_id.clone(), kind.to_owned()));
        }
    }

    let mut included: BTreeMap<String, usize> = BTreeMap::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut truncated = false;

    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    match focus {
        Some(note_id) => {
            if index.note(note_id).is_some() {
                included.insert(note_id.to_owned(), 0);
                queue.push_back((note_id.to_owned(), 0));
            }
        }
        None => {
            // The global graph starts from every note at depth 0, so the whole
            // vault is visible without a walk.
            for note in index.notes() {
                if included.len() >= limit {
                    truncated = true;
                    break;
                }
                included.insert(note.note_id.clone(), 0);
            }
            for note in index.notes() {
                for parsed in &note.links {
                    let target = target_of(parsed);
                    if !included.contains_key(&target) {
                        continue;
                    }
                    edges.push(GraphEdge {
                        from: note.note_id.clone(),
                        to: target,
                        kind: match parsed.link.kind {
                            onote_metadata_links::LinkKind::Embed => "embed".to_owned(),
                            _ => "link".to_owned(),
                        },
                    });
                }
            }
        }
    }

    while let Some((current, distance)) = queue.pop_front() {
        if distance >= depth {
            continue;
        }
        let neighbours: Vec<(String, String)> = outgoing
            .get(&current)
            .into_iter()
            .flatten()
            .chain(incoming.get(&current).into_iter().flatten())
            .cloned()
            .collect();
        for (other, kind) in neighbours {
            let (from, to) = if outgoing
                .get(&current)
                .is_some_and(|list| list.iter().any(|(target, _)| *target == other))
            {
                (current.clone(), other.clone())
            } else {
                (other.clone(), current.clone())
            };
            if !included.contains_key(&other) {
                if included.len() >= limit {
                    truncated = true;
                    continue;
                }
                included.insert(other.clone(), distance + 1);
                queue.push_back((other.clone(), distance + 1));
            }
            edges.push(GraphEdge { from, to, kind });
        }
    }

    // Deduplicate edges: two notes that link and embed each other would
    // otherwise produce a doubled line.
    edges.sort_by(|left, right| {
        (left.from.as_str(), left.to.as_str(), left.kind.as_str()).cmp(&(
            right.from.as_str(),
            right.to.as_str(),
            right.kind.as_str(),
        ))
    });
    edges.dedup();

    let mut nodes: Vec<GraphNode> = included
        .iter()
        .map(|(note_id, distance)| {
            let metadata = index.note(note_id);
            GraphNode {
                id: note_id.clone(),
                title: metadata.map(|note| note.title.clone()).unwrap_or_else(|| {
                    // A missing target shows the path the author typed.
                    note_id
                        .strip_prefix("missing:")
                        .unwrap_or(note_id)
                        .to_owned()
                }),
                relative_path: metadata
                    .map(|note| note.relative_path.clone())
                    .unwrap_or_default(),
                kind: if metadata.is_some() {
                    "note"
                } else {
                    "missing"
                }
                .to_owned(),
                anchor: focus == Some(note_id.as_str()),
                depth: *distance,
            }
        })
        .collect();
    // Keep only edges whose both ends exist as nodes; the missing-node pass
    // above may itself have been truncated.
    let present: std::collections::BTreeSet<&str> =
        nodes.iter().map(|node| node.id.as_str()).collect();
    edges.retain(|edge| present.contains(edge.from.as_str()) && present.contains(edge.to.as_str()));
    nodes.sort_by(|left, right| {
        (left.depth, left.title.as_str(), left.id.as_str()).cmp(&(
            right.depth,
            right.title.as_str(),
            right.id.as_str(),
        ))
    });

    Ok(GraphReport {
        nodes,
        edges,
        truncated,
        unreadable,
    })
}

/// Every link problem in the vault, partitioned so the UI can label them.
pub fn link_report(storage: &VaultStorage) -> Result<LinkReport, ServiceError> {
    let (index, unreadable) = rebuild_index(storage)?;
    Ok(LinkReport {
        broken: index.broken_links().into_iter().cloned().collect(),
        missing_anchors: index.missing_anchors().into_iter().cloned().collect(),
        unreadable,
    })
}

/// Derived metadata for one note, including its backlinks.
pub fn note_metadata(
    storage: &VaultStorage,
    note_id: &str,
) -> Result<NoteMetadataReport, ServiceError> {
    let (index, unreadable) = rebuild_index(storage)?;
    let note = index.note(note_id).ok_or_else(|| {
        let mut error = ServiceError {
            code: ErrorCode::ObjectNotFound,
            message_key: "error.noteMissing".to_owned(),
            params: BTreeMap::new(),
            retryable: false,
            operation_id: None,
            retry_after_seconds: None,
        };
        error.operation_id = Some(note_id.to_owned());
        error
    })?;
    Ok(NoteMetadataReport {
        note_id: note.note_id.clone(),
        relative_path: note.relative_path.clone(),
        title: note.title.clone(),
        headings: note.headings.clone(),
        block_anchors: note.block_anchors.clone(),
        tags: note.tags.clone(),
        links: note.links.clone(),
        backlinks: index.backlinks(note_id).to_vec(),
        unreadable,
    })
}

/// Expands one embed target. `chain` carries the note ids already expanded on
/// the caller's path so cycles and the depth limit stay intact across calls.
pub fn expand_embed(
    storage: &VaultStorage,
    from_note_id: &str,
    link: &onote_metadata_links::RawLink,
    chain: &[String],
) -> Result<EmbedExpansion, ServiceError> {
    let (index, _) = rebuild_index(storage)?;
    validate_note(&index, from_note_id)?;
    validate_embed_chain(&index, chain, from_note_id)?;
    let mut chain = if chain.is_empty() {
        vec![from_note_id.to_owned()]
    } else {
        chain.to_vec()
    };
    Ok(index.expand_embed_in(&mut chain, from_note_id, link))
}

/// Plans a rename without applying anything (F10-R03). The caller previews the
/// edits and only then asks M02 to execute, so a cancelled rename leaves every
/// note byte-identical.
pub fn rename_plan(
    storage: &VaultStorage,
    note_id: &str,
    to_path: &str,
) -> Result<onote_metadata_links::RenamePlan, ServiceError> {
    let target = to_path.trim();
    if target.is_empty() {
        return Err(ServiceError {
            code: ErrorCode::InvalidInput,
            message_key: "error.renameTargetRequired".to_owned(),
            params: BTreeMap::new(),
            retryable: false,
            operation_id: None,
            retry_after_seconds: None,
        });
    }
    let target = onote_vault_storage::validate_new_note_path(target)?;
    let (index, unreadable) = rebuild_index(storage)?;
    if !unreadable.is_empty() {
        return Err(ServiceError {
            code: ErrorCode::RescanRequired,
            message_key: "error.renameBaselineChanged".into(),
            params: BTreeMap::new(),
            retryable: false,
            operation_id: None,
            retry_after_seconds: None,
        });
    }
    index.note(note_id).ok_or_else(|| ServiceError {
        code: ErrorCode::ObjectNotFound,
        message_key: "error.noteMissing".to_owned(),
        params: BTreeMap::new(),
        retryable: false,
        operation_id: Some(note_id.to_owned()),
        retry_after_seconds: None,
    })?;
    let mut plan =
        onote_metadata_links::plan_rename(&index, note_id, &target).ok_or_else(|| {
            ServiceError {
                code: ErrorCode::ObjectNotFound,
                message_key: "error.noteMissing".to_owned(),
                params: BTreeMap::new(),
                retryable: false,
                operation_id: Some(note_id.to_owned()),
                retry_after_seconds: None,
            }
        })?;
    plan.vault_id = storage.vault().vault_id;
    let ids = std::iter::once(note_id.to_owned())
        .chain(plan.edits.iter().map(|e| e.note_id.clone()))
        .collect::<std::collections::BTreeSet<_>>();
    for id in ids {
        let document = storage.read(&id)?;
        if document.read_only || index.source(&id) != Some(document.source.as_str()) {
            return Err(ServiceError {
                code: ErrorCode::VersionConflict,
                message_key: "error.renameBaselineChanged".into(),
                params: BTreeMap::new(),
                retryable: false,
                operation_id: None,
                retry_after_seconds: None,
            });
        }
        plan.expected_hashes.push(onote_domain::RenameBaseline {
            note_id: id,
            relative_path: document.relative_path,
            expected_revision: document.local_revision,
            content_hash: document.content_hash,
        });
    }
    Ok(plan)
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagUpdatePreview {
    pub plan: onote_metadata_links::TagRenamePlan,
    pub write_plan: Option<onote_domain::TagWritePlan>,
    pub conflict_paths: BTreeMap<String, String>,
}

pub fn tag_update_preview(
    storage: &VaultStorage,
    old_tag: &str,
    new_tag: Option<&str>,
) -> Result<TagUpdatePreview, ServiceError> {
    use onote_metadata_links::{TagNote, TagPlanIssue};
    let summaries = storage.list()?;
    let paths = summaries
        .iter()
        .map(|note| (note.note_id.clone(), note.relative_path.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut documents = BTreeMap::new();
    let mut notes = Vec::new();
    let mut unavailable = Vec::new();
    let mut existing = Vec::new();
    for summary in summaries {
        match storage.read(&summary.note_id) {
            Ok(document) => {
                let tags = onote_metadata_links::extract_tags(&document.source);
                existing.extend(tags.iter().map(|t| t.tag.clone()));
                if !tags.iter().any(|t| t.tag == old_tag) {
                    continue;
                }
                if document.read_only || !storage.vault().writable {
                    unavailable.push((
                        document.note_id.clone(),
                        TagPlanIssue::SourceUnsupported {
                            note_id: document.note_id,
                        },
                    ));
                    continue;
                }
                notes.push(TagNote {
                    note_id: document.note_id.clone(),
                    relative_path: document.relative_path.clone(),
                    source: document.source.clone(),
                    revision: document.local_revision,
                });
                documents.insert(document.note_id.clone(), document);
            }
            Err(_) => unavailable.push((
                summary.note_id.clone(),
                TagPlanIssue::SourceUnsupported {
                    note_id: summary.note_id,
                },
            )),
        }
    }
    let revisions = notes
        .iter()
        .map(|n| (n.note_id.clone(), n.revision))
        .collect::<Vec<_>>();
    let mut plan = match new_tag {
        Some(new_tag) => {
            onote_metadata_links::plan_tag_rename(&notes, old_tag, new_tag, &revisions, &existing)
        }
        None => onote_metadata_links::plan_removal(&notes, old_tag, &revisions),
    };
    plan.conflicted.extend(unavailable);
    let write_plan = if plan.rewrites.is_empty() {
        None
    } else {
        Some(onote_domain::TagWritePlan {
            plan_id: uuid::Uuid::new_v4().to_string(),
            vault_id: storage.vault().vault_id,
            old_tag: old_tag.to_owned(),
            new_tag: new_tag.map(|_| plan.new_tag.clone()),
            is_merge: plan.is_merge,
            items: plan
                .rewrites
                .iter()
                .map(|r| onote_domain::TagWriteItem {
                    note_id: r.note_id.clone(),
                    relative_path: r.relative_path.clone(),
                    expected_revision: r.expected_revision,
                    content_hash: documents[&r.note_id].content_hash.clone(),
                    source: r.source.clone(),
                })
                .collect(),
        })
    };
    let conflict_paths = plan
        .conflicted
        .iter()
        .filter_map(|(id, _)| paths.get(id).map(|path| (id.clone(), path.clone())))
        .collect();
    Ok(TagUpdatePreview {
        plan,
        write_plan,
        conflict_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_rename_executes_and_rebuilds_references_without_touching_prose_or_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "rename host").unwrap();
        let operation = || uuid::Uuid::new_v4().to_string();
        let target = storage
            .create_note("Old.md", "# Heading\r\n", &operation())
            .unwrap();
        let linked=storage.create_note("Source.md","\u{feff}---\r\ncustom: unchanged\r\n---\r\nProse Old. [[Old#Heading|alias]] ![[Old]] `[[Old]]`\r\n",&operation()).unwrap();
        let plan = rename_plan(&storage, &target.note_id, "Folder/New.md").unwrap();
        assert_eq!(plan.vault_id, storage.vault().vault_id);
        assert_eq!(plan.expected_hashes.len(), 2);
        assert!(plan
            .expected_hashes
            .iter()
            .any(|b| b.note_id == target.note_id));
        assert_eq!(plan.edits.len(), 2);
        let result = storage
            .rename(&plan, target.local_revision, &operation())
            .unwrap();
        assert!(result.completed);
        let expected = linked
            .source
            .replace("[[Old#Heading|alias]]", "[[Folder/New#Heading|alias]]")
            .replace("![[Old]]", "![[Folder/New]]");
        assert_eq!(storage.read(&linked.note_id).unwrap().source, expected);
        let (index, unreadable) = rebuild_index(&storage).unwrap();
        assert!(unreadable.is_empty());
        let links = &index.note(&linked.note_id).unwrap().links;
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|link| link.resolved.target_id.as_deref()
            == Some(&target.note_id)
            && link.resolved.status == onote_metadata_links::LinkStatus::Resolved));
    }

    #[test]
    fn tag_preview_persists_merge_then_removal_without_losing_notes_or_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let original = format!("\u{feff}---\r\nopennote-id: {id}\r\ntags: [old, new]\r\ncustom:\r\n  tags: [old]\r\n---\r\n#old #new `#old`\r\n-\r\nsearchable body\r\n");
        std::fs::write(dir.path().join("Tags.md"), &original).unwrap();
        let mut storage = VaultStorage::create(dir.path(), "tags host").unwrap();
        let preview = tag_update_preview(&storage, "old", Some("new")).unwrap();
        assert!(preview.plan.is_merge);
        assert!(preview.plan.conflicted.is_empty());
        let plan = preview.write_plan.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Tags.md")).unwrap(),
            original
        );
        assert!(
            storage
                .apply_tag_update(&plan, &uuid::Uuid::new_v4().to_string())
                .unwrap()
                .completed
        );
        let merged = storage.read(&id).unwrap();
        assert!(merged.source.starts_with('\u{feff}'));
        assert!(merged.source.contains("custom:\r\n  tags: [old]\r\n"));
        assert!(merged.source.contains("`#old`\r\n-\r\nsearchable body\r\n"));
        let report = tag_report(&storage).unwrap();
        assert_eq!(report.tags.len(), 1);
        assert_eq!(report.tags[0].tag, "new");
        let removal = tag_update_preview(&storage, "new", None).unwrap();
        assert!(
            storage
                .apply_tag_update(
                    &removal.write_plan.unwrap(),
                    &uuid::Uuid::new_v4().to_string()
                )
                .unwrap()
                .completed
        );
        assert!(tag_report(&storage).unwrap().tags.is_empty());
        let (index, unreadable) = rebuild_index(&storage).unwrap();
        assert!(unreadable.is_empty());
        assert!(index.note(&id).is_some());
        assert!(storage
            .read(&id)
            .unwrap()
            .source
            .contains("searchable body"));
        assert_eq!(storage.list().unwrap().len(), 1);
        let host = crate::search::SearchHost::default();
        let page = crate::search::search(
            &host,
            &storage,
            &storage.vault().vault_id,
            &onote_search::SearchRequest::new("searchable body"),
        )
        .unwrap();
        assert_eq!(page.items.len(), 1);
    }

    #[test]
    fn tag_preview_reports_unsupported_sources_beside_writable_notes_and_serializes_issues() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "tags host").unwrap();
        let good = storage
            .create_note("Good.md", "#old\n", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        storage
            .create_note(
                "Alias.md",
                "---\ntags: &shared [old]\ncopy: *shared\n---\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        let preview = tag_update_preview(&storage, "old", Some("new")).unwrap();
        assert_eq!(
            preview.write_plan.as_ref().unwrap().items[0].note_id,
            good.note_id
        );
        assert_eq!(preview.plan.conflicted.len(), 1);
        let issue = serde_json::to_value(&preview.plan.conflicted[0].1).unwrap();
        assert_eq!(issue["kind"], "sourceUnsupported");
        assert_eq!(
            serde_json::to_value(&preview).unwrap()["conflictPaths"]
                [preview.plan.conflicted[0].0.as_str()],
            "Alias.md"
        );
        assert!(issue["noteId"].is_string());
    }

    #[test]
    fn readonly_tag_preview_does_not_offer_a_write_plan() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "readonly tags").unwrap();
        storage
            .create_note("Tags.md", "#old\n", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        drop(storage);
        let path = dir.path().join(".opennote/vault.json");
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        metadata["writable"] = false.into();
        std::fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let storage = VaultStorage::open(dir.path()).unwrap();
        let preview = tag_update_preview(&storage, "old", None).unwrap();
        assert!(preview.write_plan.is_none());
        assert_eq!(preview.plan.conflicted.len(), 1);
    }

    #[test]
    fn title_is_the_file_stem_without_extension() {
        assert_eq!(title_of("notes/My Note.md"), "My Note");
        assert_eq!(title_of("My Note.MD"), "My Note");
        assert_eq!(title_of("folder/sub/plain"), "plain");
        assert_eq!(title_of("a\\b\\c.md"), "c");
        assert_eq!(title_of(""), "");
    }
}
