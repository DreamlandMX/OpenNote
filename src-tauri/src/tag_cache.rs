//! Derived tag candidates, keyed by authoritative catalog summaries; no note writes.
use crate::{
    metadata::{TagEntry, TagReport, UnreadableNote},
    svc_err,
};
use onote_domain::{ErrorCode, NoteSummary, ServiceError};
use onote_vault_storage::VaultStorage;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

struct Tags {
    path: String,
    hash: String,
    names: Vec<String>,
}
struct Cache {
    root: PathBuf,
    vault: String,
    notes: BTreeMap<String, Tags>,
}
#[derive(Default)]
pub(crate) struct TagHost(Mutex<Option<Cache>>);

impl TagHost {
    pub(crate) fn report(
        &self,
        root: &Path,
        vault: &str,
        summaries: &[NoteSummary],
        unavailable: &HashSet<String>,
    ) -> Result<TagReport, ServiceError> {
        let mut cache = self
            .0
            .lock()
            .map_err(|_| svc_err(ErrorCode::RescanRequired, "error.searchIndexUnavailable"))?;
        let previous = cache
            .as_ref()
            .filter(|old| old.root == root && old.vault == vault);
        let mut notes = BTreeMap::new();
        let mut tags: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        let mut unreadable = Vec::new();
        for note in summaries {
            let names = if unavailable.contains(&note.note_id) {
                Err(svc_err(ErrorCode::VersionConflict, "error.catalogChanged"))
            } else if let Some(old) = previous
                .and_then(|c| c.notes.get(&note.note_id))
                .filter(|old| old.path == note.relative_path && old.hash == note.content_hash)
            {
                Ok(old.names.clone())
            } else {
                VaultStorage::read_note_source_snapshot(root, note).map(|source| {
                    onote_metadata_links::extract_tags(&source)
                        .into_iter()
                        .map(|tag| tag.tag)
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                })
            };
            match names {
                Ok(names) => {
                    for name in &names {
                        tags.entry(name.clone())
                            .or_default()
                            .push((note.relative_path.clone(), note.note_id.clone()));
                    }
                    notes.insert(
                        note.note_id.clone(),
                        Tags {
                            path: note.relative_path.clone(),
                            hash: note.content_hash.clone(),
                            names,
                        },
                    );
                }
                Err(error) => unreadable.push(UnreadableNote {
                    note_id: note.note_id.clone(),
                    relative_path: note.relative_path.clone(),
                    code: error.code,
                    message_key: error.message_key,
                }),
            }
        }
        let tags = tags
            .into_iter()
            .map(|(tag, mut entries)| {
                entries.sort();
                TagEntry {
                    tag,
                    note_paths: entries.iter().map(|(path, _)| path.clone()).collect(),
                    note_ids: entries.into_iter().map(|(_, id)| id).collect(),
                }
            })
            .collect();
        *cache = Some(Cache {
            root: root.to_path_buf(),
            vault: vault.into(),
            notes,
        });
        Ok(TagReport { tags, unreadable })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    #[test]
    fn tag_snapshots_match_metadata_semantics_without_taking_the_writer_gate_or_writing_notes() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Tag snapshots").unwrap();
        storage
            .create_note(
                "b.md",
                "---\ntags: [Mixed, Mixed, 中文]\n---\n#inline\n```\n#ignored\n```",
                &id(),
            )
            .unwrap();
        storage
            .create_note("a.md", "#Mixed #inline #Mixed", &id())
            .unwrap();
        let expected = crate::metadata::tag_report(&storage).unwrap();
        let summaries = storage.list().unwrap();
        let before: Vec<_> = summaries
            .iter()
            .map(|n| std::fs::read(dir.path().join(&n.relative_path)).unwrap())
            .collect();
        let gate = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.path().join(".opennote/writer.lock"))
            .unwrap();
        gate.try_lock().unwrap();
        let host = TagHost::default();
        let report = host
            .report(
                dir.path(),
                &storage.vault().vault_id,
                &summaries,
                &HashSet::new(),
            )
            .unwrap();
        assert_eq!(report, expected);
        assert!(report.tags.iter().any(|tag| tag.tag == "Mixed"));
        assert!(!report.tags.iter().any(|tag| tag.tag == "ignored"));
        for (summary, bytes) in summaries.iter().zip(before) {
            assert_eq!(
                std::fs::read(dir.path().join(&summary.relative_path)).unwrap(),
                bytes
            );
        }
        assert_eq!(storage.list().unwrap(), summaries);
    }

    #[test]
    fn catalog_conflicts_remove_cached_tags_and_returned_bytes_restore_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Tag availability").unwrap();
        let a = storage.create_note("a.md", "#oldtag", &id()).unwrap();
        storage.create_note("b.md", "#healthy", &id()).unwrap();
        let host = TagHost::default();
        let vault = storage.vault().vault_id.clone();
        let bytes = std::fs::read(dir.path().join("a.md")).unwrap();
        assert_eq!(
            host.report(
                dir.path(),
                &vault,
                &storage.list().unwrap(),
                &HashSet::new()
            )
            .unwrap()
            .tags
            .len(),
            2
        );
        let foreign = format!("---\nopennote-id: {}\n---\n#foreign", id());
        std::fs::write(dir.path().join("a.md"), &foreign).unwrap();
        let view = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        assert!(view.unavailable_note_ids.contains(&a.note_id));
        let unavailable = view.unavailable_note_ids.into_iter().collect();
        let report = host
            .report(dir.path(), &vault, &storage.list().unwrap(), &unavailable)
            .unwrap();
        assert_eq!(
            report
                .tags
                .iter()
                .map(|tag| tag.tag.as_str())
                .collect::<Vec<_>>(),
            ["healthy"]
        );
        assert_eq!(report.unreadable.len(), 1);
        assert_eq!(
            std::fs::read(dir.path().join("a.md")).unwrap(),
            foreign.as_bytes()
        );
        std::fs::write(dir.path().join("a.md"), &bytes).unwrap();
        let returned = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        assert!(returned.unavailable_note_ids.is_empty());
        assert_eq!(
            host.report(
                dir.path(),
                &vault,
                &storage.list().unwrap(),
                &HashSet::new()
            )
            .unwrap()
            .tags
            .len(),
            2
        );
    }
}
