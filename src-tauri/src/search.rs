//! M11 host adapter over M05 search.
//!
//! The index is derived state. [`SearchHost`] keeps a rebuildable cache for one
//! vault: the fingerprint of the note summaries decides whether the warm index
//! is still valid, and only changed notes are re-read. M02 stores validated
//! sources in `.opennote/index.sqlite`; a fresh process reconstructs M05's
//! deterministic query index from them without re-reading every Markdown file.
//!
//! Rebuilds are split so note editing never waits for indexing: a short
//! snapshot phase lists summaries while the caller holds the vault state lock,
//! then the lock is released before any note source is read. Source reads use
//! [`VaultStorage::read_note_source_snapshot`] so they also skip the writer gate.
//! New source reads must match the captured identity/hash. Persistent cache
//! rows must match the same summaries and completed availability observations;
//! like warm memory results, they depend on catalog notifications for freshness.
//!
//! S19 runs the phases in managed background jobs; the legacy entry point is
//! retained. Catalog observations use a separate lock from rebuilding.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::search_job::Phase;
use onote_domain::{ErrorCode, NoteSummary, ServiceError};
use onote_search::{IndexState, SearchIndex, SearchPage, SearchRequest, SearchSource};
use onote_vault_storage::VaultStorage;
type Progress<'a> = dyn FnMut(Phase, usize, usize, usize) -> bool + 'a;
fn check(
    progress: &mut Progress<'_>,
    phase: Phase,
    done: usize,
    total: usize,
    unreadable: usize,
) -> Result<(), ServiceError> {
    if progress(phase, done, total, unreadable) {
        Ok(())
    } else {
        Err(map_query_error(onote_search::SearchError::Cancelled))
    }
}

#[cfg(test)]
#[path = "search_bench.rs"]
mod bench;

pub use onote_metadata_links::title_of;

#[derive(Default)]
pub struct SearchHost {
    session: Mutex<SearchSession>,
    catalog: Mutex<Option<CatalogView>>,
}

struct CatalogView {
    root: PathBuf,
    vault_id: String,
    unavailable: HashSet<String>,
}
impl SearchHost {
    pub(crate) fn observe_catalog(
        &self,
        root: &Path,
        vault_id: &str,
        result: &onote_domain::CatalogRefresh,
    ) -> Result<(), ServiceError> {
        if result.deferred {
            return Ok(());
        }
        *self
            .catalog
            .lock()
            .map_err(|_| svc_err(ErrorCode::RescanRequired, "error.searchIndexUnavailable"))? =
            Some(CatalogView {
                root: root.to_path_buf(),
                vault_id: vault_id.into(),
                unavailable: result.unavailable_note_ids.iter().cloned().collect(),
            });
        Ok(())
    }
    pub(crate) fn unavailable(
        &self,
        root: &Path,
        vault: &str,
    ) -> Result<HashSet<String>, ServiceError> {
        Ok(self
            .catalog
            .lock()
            .map_err(|_| svc_err(ErrorCode::RescanRequired, "error.searchIndexUnavailable"))?
            .as_ref()
            .filter(|v| v.root == root && v.vault_id == vault)
            .map(|v| v.unavailable.clone())
            .unwrap_or_default())
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, SearchSession>, ServiceError> {
        self.session
            .lock()
            .map_err(|_| svc_err(ErrorCode::RescanRequired, "error.searchIndexUnavailable"))
    }

    /// Phase one for callers that already captured summaries under the vault
    /// lock. Reads the sources here, after that lock is released, so a queued
    /// save never waits for the cold-index file reads (F12-R03).
    pub(crate) fn prepare_snapshot(
        &self,
        root: &Path,
        vault_id: &str,
        summaries: &[NoteSummary],
        request: &SearchRequest,
        persist_allowed: bool,
    ) -> Result<Prepared, ServiceError> {
        self.prepare_controlled(
            root,
            vault_id,
            summaries,
            request,
            persist_allowed,
            &mut |_, _, _, _| true,
        )
    }

    pub(crate) fn prepare_controlled(
        &self,
        root: &Path,
        vault_id: &str,
        summaries: &[NoteSummary],
        request: &SearchRequest,
        persist_allowed: bool,
        progress: &mut Progress<'_>,
    ) -> Result<Prepared, ServiceError> {
        let unavailable = self.unavailable(root, vault_id)?;
        self.lock()?.prepare(
            root,
            vault_id,
            summaries,
            &unavailable,
            request,
            persist_allowed,
            progress,
        )
    }

    /// Convenience wrapper used by tests and by callers that still hold the
    /// vault lock. Production search captures summaries first and calls
    /// [`SearchHost::prepare_snapshot`] after releasing it.
    #[cfg(test)]
    pub(crate) fn prepare(
        &self,
        storage: &VaultStorage,
        vault_id: &str,
        request: &SearchRequest,
    ) -> Result<Prepared, ServiceError> {
        let summaries = storage.list()?;
        self.prepare_snapshot(
            storage.root_path(),
            vault_id,
            &summaries,
            request,
            storage.vault().writable,
        )
    }

    /// Reports the cached index status without rebuilding anything.
    ///
    /// Read-only on purpose: asking for diagnostics must not trigger an index
    /// rebuild, which would make a status check expensive and would change the
    /// very state it is describing.
    pub fn status(&self) -> Result<onote_search::IndexStatus, ServiceError> {
        Ok(self.lock()?.warm_status())
    }

    /// Phase two: runs after the vault lock is released.
    pub(crate) fn finish(
        &self,
        prepared: Prepared,
        request: &SearchRequest,
    ) -> Result<SearchPage, ServiceError> {
        self.finish_controlled(prepared, request, &mut |_, _, _, _| true)
    }
    pub(crate) fn finish_controlled(
        &self,
        prepared: Prepared,
        request: &SearchRequest,
        progress: &mut Progress<'_>,
    ) -> Result<SearchPage, ServiceError> {
        self.lock()?.finish(prepared, request, progress)
    }
}

#[derive(Default)]
struct SearchSession {
    cache: Option<CachedSearch>,
}

impl SearchSession {
    /// The status of the warm index, or `Unavailable` when nothing is cached.
    ///
    /// A cold cache is reported as unavailable rather than as zero documents:
    /// "not built yet" is a different fact from "the vault is empty" (F44-R04).
    fn warm_status(&self) -> onote_search::IndexStatus {
        match self.cache.as_ref() {
            Some(cached) => cached.index.status().clone(),
            None => onote_search::IndexStatus {
                state: IndexState::Unavailable,
                version: 0,
                indexed_notes: 0,
                unreadable_notes: 0,
                cache_persisted: None,
            },
        }
    }
}

/// Sources read in phase one. The vault may change before phase two; that is
/// safe because the next query compares a fresh fingerprint and rebuilds, so a
/// page may show a one-save-old index but never a silently permanent one.
pub(crate) struct Prepared {
    root: PathBuf,
    vault_id: String,
    fingerprint: u64,
    sources: Vec<SearchSource>,
    cached_sources: BTreeMap<String, CachedSource>,
    unreadable: usize,
    /// A warm cache answered the query during phase one. Kept as a page, not a
    /// flag, so phase two cannot race a cache replacement.
    warm_page: Option<SearchPage>,
    persist_allowed: bool,
}

struct CachedSearch {
    root: PathBuf,
    vault_id: String,
    fingerprint: u64,
    unreadable: usize,
    sources: BTreeMap<String, CachedSource>,
    index: SearchIndex,
}

type CachedSource = onote_vault_storage::SearchCacheSource;

impl SearchSession {
    /// Reads a captured summary set. A matching fingerprint answers from the
    /// warm index here; otherwise the sources are returned for phase two.
    /// `root` is captured under the vault lock, but the reads themselves run
    /// after that lock is released.
    fn prepare(
        &mut self,
        root: &Path,
        vault_id: &str,
        summaries: &[NoteSummary],
        unavailable: &HashSet<String>,
        request: &SearchRequest,
        persist_allowed: bool,
        progress: &mut Progress<'_>,
    ) -> Result<Prepared, ServiceError> {
        check(progress, Phase::Reading, 0, summaries.len(), 0)?;
        let fingerprint = fingerprint(summaries, unavailable);
        if let Some(cache) = &mut self.cache {
            if cache.root == root
                && cache.vault_id == vault_id
                && cache.fingerprint == fingerprint
                && cache.unreadable == 0
            {
                if persist_allowed
                    && !request.query.trim().is_empty()
                    && cache.index.status().cache_persisted == Some(false)
                {
                    match VaultStorage::write_search_cache(
                        root,
                        vault_id,
                        cache.sources.values(),
                        |done, total| progress(Phase::SavingIndex, done, total, 0),
                    ) {
                        Ok(()) => cache.index.set_cache_persisted(true),
                        Err(error) if error.code == ErrorCode::OperationCancelled => {
                            return Err(error)
                        }
                        Err(_) => {}
                    }
                }
                check(progress, Phase::Querying, 0, 1, 0)?;
                let page = cache.index.query(request).map_err(map_query_error)?;
                check(progress, Phase::Querying, 1, 1, 0)?;
                return Ok(Prepared {
                    root: root.to_path_buf(),
                    vault_id: vault_id.to_owned(),
                    fingerprint,
                    sources: Vec::new(),
                    cached_sources: BTreeMap::new(),
                    unreadable: 0,
                    warm_page: Some(page),
                    persist_allowed,
                });
            }
        }

        if request.query.trim().is_empty() {
            // Validate even empty requests without reading files or claiming a built index.
            let mut page = SearchIndex::rebuild(&[], fingerprint, 0)
                .query(request)
                .map_err(map_query_error)?;
            page.index_status = SearchIndex::unavailable(fingerprint, 0).status().clone();
            return Ok(Prepared {
                root: root.to_path_buf(),
                vault_id: vault_id.to_owned(),
                fingerprint,
                sources: Vec::new(),
                cached_sources: BTreeMap::new(),
                unreadable: 0,
                warm_page: Some(page),
                persist_allowed,
            });
        }

        let previous = self
            .cache
            .as_ref()
            .filter(|cache| cache.root == root && cache.vault_id == vault_id);
        let disk_sources = if previous.is_none() {
            match VaultStorage::read_search_cache(root, vault_id, summaries, |done, total| {
                progress(Phase::LoadingIndex, done, total, 0)
            }) {
                Ok(sources) => sources,
                Err(error) if error.code == ErrorCode::OperationCancelled => return Err(error),
                Err(_) => BTreeMap::new(),
            }
        } else {
            BTreeMap::new()
        };
        let mut sources = Vec::with_capacity(summaries.len());
        let mut cached_sources = BTreeMap::new();
        let mut unreadable = 0usize;
        for (done, summary) in summaries.iter().enumerate() {
            check(progress, Phase::Reading, done, summaries.len(), unreadable)?;
            if unavailable.contains(&summary.note_id) {
                unreadable += 1;
                continue;
            }
            let title = title_of(&summary.relative_path);
            let cached = previous
                .and_then(|cache| cache.sources.get(&summary.note_id))
                .or_else(|| disk_sources.get(&summary.note_id))
                .filter(|source| {
                    source.content_hash == summary.content_hash
                        && source.relative_path == summary.relative_path
                });
            let source = match cached {
                Some(cached) => cached.source.clone(),
                None => match VaultStorage::read_note_source_snapshot(root, summary) {
                    Ok(source) => source,
                    Err(_) => {
                        unreadable += 1;
                        continue;
                    }
                },
            };
            cached_sources.insert(
                summary.note_id.clone(),
                CachedSource {
                    note_id: summary.note_id.clone(),
                    content_hash: summary.content_hash.clone(),
                    relative_path: summary.relative_path.clone(),
                    source: source.clone(),
                },
            );
            sources.push(SearchSource::new(
                summary.note_id.clone(),
                summary.relative_path.clone(),
                title,
                source,
            ));
        }
        check(
            progress,
            Phase::Reading,
            summaries.len(),
            summaries.len(),
            unreadable,
        )?;
        Ok(Prepared {
            root: root.to_path_buf(),
            vault_id: vault_id.to_owned(),
            fingerprint,
            sources,
            cached_sources,
            unreadable,
            warm_page: None,
            persist_allowed,
        })
    }

    fn finish(
        &mut self,
        prepared: Prepared,
        request: &SearchRequest,
        progress: &mut Progress<'_>,
    ) -> Result<SearchPage, ServiceError> {
        if let Some(page) = prepared.warm_page {
            // Answered in phase one; the existing cache stays untouched.
            check(
                progress,
                Phase::Querying,
                1,
                1,
                page.index_status.unreadable_notes,
            )?;
            return Ok(page);
        }
        // Another search may have finished an identical build while this call
        // waited for the host lock. Reuse it instead of parsing the same
        // sources twice; the fingerprint is the cache's own validity token.
        if let Some(cache) = &self.cache {
            if cache.root == prepared.root
                && cache.vault_id == prepared.vault_id
                && cache.fingerprint == prepared.fingerprint
                && cache.unreadable == 0
            {
                check(progress, Phase::Querying, 0, 1, cache.unreadable)?;
                let page = cache.index.query(request).map_err(map_query_error);
                check(progress, Phase::Querying, 1, 1, cache.unreadable)?;
                return page;
            }
        }
        let mut index = if prepared.sources.is_empty() && prepared.unreadable > 0 {
            SearchIndex::unavailable(prepared.fingerprint, prepared.unreadable)
        } else {
            SearchIndex::rebuild_controlled(
                &prepared.sources,
                prepared.fingerprint,
                prepared.unreadable,
                |done, total| progress(Phase::Indexing, done, total, prepared.unreadable),
            )
            .map_err(map_query_error)?
        };
        if index.status().state == IndexState::Unavailable {
            return Err(map_query_error(onote_search::SearchError::IndexUnavailable));
        }
        check(progress, Phase::Querying, 0, 1, prepared.unreadable)?;
        let mut page = index.query(request).map_err(map_query_error);
        check(progress, Phase::Querying, 1, 1, prepared.unreadable)?;
        let persisted = if !prepared.persist_allowed {
            false
        } else {
            match VaultStorage::write_search_cache(
                &prepared.root,
                &prepared.vault_id,
                prepared.cached_sources.values(),
                |done, total| progress(Phase::SavingIndex, done, total, prepared.unreadable),
            ) {
                Ok(()) => true,
                Err(error) if error.code == ErrorCode::OperationCancelled => return Err(error),
                Err(_) => false,
            }
        };
        index.set_cache_persisted(persisted);
        if let Ok(page) = &mut page {
            page.index_status.cache_persisted = Some(persisted);
        }
        self.cache = Some(CachedSearch {
            root: prepared.root,
            vault_id: prepared.vault_id,
            fingerprint: prepared.fingerprint,
            unreadable: prepared.unreadable,
            sources: prepared.cached_sources,
            index,
        });
        // Cache first: an invalid query must not throw away a just-built index.
        page
    }
}

fn fingerprint(summaries: &[NoteSummary], unavailable: &HashSet<String>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut feed = |text: &str| {
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0x1f;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for summary in summaries {
        feed(&summary.note_id);
        feed(&summary.relative_path);
        feed(&summary.content_hash);
        feed(if unavailable.contains(&summary.note_id) {
            "unavailable"
        } else {
            "available"
        });
    }
    hash
}

fn map_query_error(error: onote_search::SearchError) -> ServiceError {
    match error {
        onote_search::SearchError::Cancelled => {
            svc_err(ErrorCode::OperationCancelled, "error.searchCancelled")
        }
        onote_search::SearchError::QueryTooLong => {
            svc_err(ErrorCode::InvalidInput, "error.searchQueryTooLong")
        }
        onote_search::SearchError::InvalidLimit => {
            svc_err(ErrorCode::InvalidInput, "error.searchLimitInvalid")
        }
        onote_search::SearchError::InvalidCursor => retryable(svc_err(
            ErrorCode::VersionConflict,
            "error.searchCursorExpired",
        )),
        onote_search::SearchError::IndexUnavailable => retryable(svc_err(
            ErrorCode::RescanRequired,
            "error.searchIndexUnavailable",
        )),
    }
}

fn svc_err(code: ErrorCode, message_key: &str) -> ServiceError {
    ServiceError {
        code,
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
        retryable: false,
        operation_id: None,
        retry_after_seconds: None,
    }
}

fn retryable(mut error: ServiceError) -> ServiceError {
    error.retryable = true;
    error
}

/// Entry point used by the tests and by callers that already hold the vault
/// lock; it performs both phases back to back. The Tauri command captures
/// summaries under the vault lock, then calls [`SearchHost::prepare_snapshot`]
/// after releasing it so the cold reads never block a queued save.
#[cfg(test)]
pub fn search(
    host: &SearchHost,
    storage: &VaultStorage,
    vault_id: &str,
    request: &SearchRequest,
) -> Result<SearchPage, ServiceError> {
    let prepared = host.prepare(storage, vault_id, request)?;
    host.finish(prepared, request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onote_search::SearchFilters;
    use tempfile::tempdir;

    fn note_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[test]
    fn effective_readonly_scope_never_creates_a_cache_and_can_retry_after_it_becomes_writable() {
        let dir = tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Readonly scope").unwrap();
        storage.create_note("a.md", "alpha", &note_id()).unwrap();
        let request = SearchRequest::new("alpha");
        let summaries = storage.list().unwrap();
        let host = SearchHost::default();
        let prepared = host
            .prepare_snapshot(
                storage.root_path(),
                &storage.vault().vault_id,
                &summaries,
                &request,
                false,
            )
            .unwrap();
        let page = host.finish(prepared, &request).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.index_status.cache_persisted, Some(false));
        assert!(!dir.path().join(".opennote/index.sqlite").exists());
        assert!(!dir.path().join(".opennote/index.writer.lock").exists());
        let prepared = host
            .prepare_snapshot(
                storage.root_path(),
                &storage.vault().vault_id,
                &summaries,
                &request,
                true,
            )
            .unwrap();
        assert_eq!(
            host.finish(prepared, &request)
                .unwrap()
                .index_status
                .cache_persisted,
            Some(true)
        );
    }

    #[test]
    fn fresh_host_uses_persisted_captured_sources_but_catalog_unavailability_excludes_them() {
        let dir = tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Persisted").unwrap();
        let note = storage
            .create_note("a.md", "İstanbul alpha 中文", &note_id())
            .unwrap();
        let request = SearchRequest {
            query: "alpha".into(),
            filters: SearchFilters::default(),
            locale: Some("zh-CN".into()),
            limit: Some(1),
            cursor: None,
        };
        let first = search(
            &SearchHost::default(),
            &storage,
            &storage.vault().vault_id,
            &request,
        )
        .unwrap();
        assert_eq!(first.index_status.cache_persisted, Some(true));
        let summaries = storage.list().unwrap();
        std::fs::rename(dir.path().join("a.md"), dir.path().join("held.md")).unwrap();
        let next = SearchHost::default();
        let prepared = next
            .prepare_snapshot(
                storage.root_path(),
                &storage.vault().vault_id,
                &summaries,
                &request,
                true,
            )
            .unwrap();
        assert_eq!(
            prepared.unreadable, 0,
            "captured source loaded from SQLite without reading Markdown"
        );
        assert_eq!(next.finish(prepared, &request).unwrap(), first);
        let catalog = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        next.observe_catalog(storage.root_path(), &storage.vault().vault_id, &catalog)
            .unwrap();
        let page = search(&next, &storage, &storage.vault().vault_id, &request).unwrap();
        assert!(page.items.iter().all(|item| item.relative_path != "a.md"));
        assert!(page
            .items
            .iter()
            .any(|item| item.note_id == note.note_id && item.relative_path == "held.md"));
    }

    #[test]
    fn foreign_cache_is_memory_only_then_explicit_retry_persists_without_changing_notes() {
        let dir = tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Cache failure").unwrap();
        storage.create_note("a.md", "alpha", &note_id()).unwrap();
        let raw = std::fs::read(dir.path().join("a.md")).unwrap();
        let request = SearchRequest {
            query: "alpha".into(),
            filters: SearchFilters::default(),
            locale: Some("en".into()),
            limit: Some(50),
            cursor: None,
        };
        // A foreign SQLite file is not permission to overwrite it.
        let path = dir.path().join(".opennote/index.sqlite");
        std::fs::write(&path, b"SQLite format 3\0").unwrap();
        let host = SearchHost::default();
        // A separate writer's sidecar forces safe in-memory fallback.
        std::fs::write(dir.path().join(".opennote/index.sqlite-wal"), b"busy").unwrap();
        let page = search(&host, &storage, &storage.vault().vault_id, &request).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.index_status.cache_persisted, Some(false));
        assert_eq!(std::fs::read(&path).unwrap(), b"SQLite format 3\0");
        std::fs::remove_file(dir.path().join(".opennote/index.sqlite-wal")).unwrap();
        // Simulate the user resolving the foreign-file obstruction in this disposable vault.
        std::fs::remove_file(&path).unwrap();
        let retried = search(&host, &storage, &storage.vault().vault_id, &request).unwrap();
        assert_eq!(retried.index_status.cache_persisted, Some(true));
        assert_eq!(retried.items, page.items);
        assert_eq!(std::fs::read(dir.path().join("a.md")).unwrap(), raw);
    }

    #[test]
    fn catalog_unavailability_removes_warm_hits_even_for_previously_readonly_notes_and_allows_return(
    ) {
        let dir = tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Availability").unwrap();
        let a = storage.create_note("a.md", "alpha", &note_id()).unwrap();
        storage
            .create_note("b.md", "healthy beta", &note_id())
            .unwrap();
        let bytes = std::fs::read(dir.path().join("a.md")).unwrap();
        let mut permissions = std::fs::metadata(dir.path().join("a.md"))
            .unwrap()
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(dir.path().join("a.md"), permissions).unwrap();
        let view = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        let host = SearchHost::default();
        let vault = storage.vault().vault_id.clone();
        host.observe_catalog(dir.path(), &vault, &view).unwrap();
        assert_eq!(
            search(&host, &storage, &vault, &SearchRequest::new("alpha"))
                .unwrap()
                .items
                .len(),
            1
        );
        let mut permissions = std::fs::metadata(dir.path().join("a.md"))
            .unwrap()
            .permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(dir.path().join("a.md"), permissions).unwrap();
        std::fs::remove_file(dir.path().join("a.md")).unwrap();
        let missing = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        assert!(missing.unavailable_note_ids.contains(&a.note_id));
        host.observe_catalog(dir.path(), &vault, &missing).unwrap();
        let page = search(&host, &storage, &vault, &SearchRequest::new("alpha")).unwrap();
        assert!(
            page.items.is_empty(),
            "catalog-unavailable notes must not survive in a warm cache"
        );
        assert_eq!(page.index_status.unreadable_notes, 1);
        std::fs::write(dir.path().join("a.md"), &bytes).unwrap();
        let returned = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        host.observe_catalog(dir.path(), &vault, &returned).unwrap();
        assert_eq!(
            search(&host, &storage, &vault, &SearchRequest::new("alpha"))
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(std::fs::read(dir.path().join("a.md")).unwrap(), bytes);
    }

    #[test]
    fn cold_search_refuses_foreign_identity_before_the_first_catalog_notification() {
        let dir = tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Foreign source").unwrap();
        storage.create_note("a.md", "alpha", &note_id()).unwrap();
        storage
            .create_note("b.md", "healthy beta", &note_id())
            .unwrap();
        let before = storage.list().unwrap();
        let foreign = format!("---\nopennote-id: {}\n---\nforeign secret", note_id());
        std::fs::write(dir.path().join("a.md"), &foreign).unwrap();
        let page = search(
            &SearchHost::default(),
            &storage,
            &storage.vault().vault_id,
            &SearchRequest::new("foreign"),
        )
        .unwrap();
        assert!(page.items.is_empty());
        assert_eq!(page.index_status.unreadable_notes, 1);
        assert_eq!(storage.list().unwrap(), before);
        assert_eq!(
            std::fs::read(dir.path().join("a.md")).unwrap(),
            foreign.as_bytes()
        );
    }

    #[test]
    fn availability_observation_does_not_wait_for_rebuilding_and_pending_views_do_not_erase_it() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;
        let dir = tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Observer locks").unwrap();
        let note = storage.create_note("a.md", "alpha", &note_id()).unwrap();
        std::fs::remove_file(dir.path().join("a.md")).unwrap();
        let view = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        let vault = storage.vault().vault_id.clone();
        let root = dir.path().to_path_buf();
        let host = Arc::new(SearchHost::default());
        let held_host = host.clone();
        let (held, ready) = mpsc::channel();
        let (resume, continued) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = held_host.lock().unwrap();
            held.send(()).unwrap();
            continued.recv().unwrap();
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let observing = host.clone();
        let observing_root = root.clone();
        let observing_vault = vault.clone();
        let observed_view = view.clone();
        let (done, ack) = mpsc::channel();
        std::thread::spawn(move || {
            done.send(observing.observe_catalog(&observing_root, &observing_vault, &observed_view))
                .unwrap()
        });
        let observed = ack.recv_timeout(Duration::from_secs(2));
        resume.send(()).unwrap();
        holder.join().unwrap();
        observed.unwrap().unwrap();
        assert!(host
            .unavailable(&root, &vault)
            .unwrap()
            .contains(&note.note_id));
        let mut pending = view;
        pending.deferred = true;
        pending.unavailable_note_ids.clear();
        host.observe_catalog(&root, &vault, &pending).unwrap();
        assert!(host
            .unavailable(&root, &vault)
            .unwrap()
            .contains(&note.note_id));
        assert!(host
            .unavailable(&root.join("different-root"), &vault)
            .unwrap()
            .is_empty());
        assert!(host.unavailable(&root, &note_id()).unwrap().is_empty());
    }

    #[test]
    fn preparing_a_replacement_keeps_the_last_complete_index_available() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Retained index").unwrap();
        storage.create_note("a.md", "alpha", &note_id()).unwrap();
        let vault = storage.vault().vault_id.clone();
        let host = SearchHost::default();
        let old = search(&host, &storage, &vault, &SearchRequest::new("alpha")).unwrap();
        storage.create_note("b.md", "beta", &note_id()).unwrap();
        let _unfinished = host
            .prepare(&storage, &vault, &SearchRequest::new("beta"))
            .unwrap();
        assert_eq!(host.status().unwrap(), old.index_status);
    }

    #[test]
    fn cancelling_indexing_or_publication_does_not_replace_the_complete_index() {
        let dir = tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Publish cancellation").unwrap();
        storage.create_note("a.md", "alpha", &note_id()).unwrap();
        let vault = storage.vault().vault_id.clone();
        let host = SearchHost::default();
        let request = SearchRequest::new("alpha");
        let old = search(&host, &storage, &vault, &request).unwrap();
        let old_disk = std::fs::read(dir.path().join(".opennote/index.sqlite")).unwrap();
        storage.create_note("b.md", "beta", &note_id()).unwrap();
        for (phase, stop) in [
            (Phase::Indexing, 1),
            (Phase::Querying, 0),
            (Phase::Querying, 1),
            (Phase::SavingIndex, 0),
            (Phase::SavingIndex, 1),
            (Phase::SavingIndex, 2),
        ] {
            let prepared = host.prepare(&storage, &vault, &request).unwrap();
            let result = host.finish_controlled(prepared, &request, &mut |current, done, _, _| {
                current != phase || done != stop
            });
            assert_eq!(result.unwrap_err().code, ErrorCode::OperationCancelled);
            assert_eq!(host.status().unwrap(), old.index_status);
            assert_eq!(
                std::fs::read(dir.path().join(".opennote/index.sqlite")).unwrap(),
                old_disk
            );
        }
        let fresh = search(&host, &storage, &vault, &SearchRequest::new("beta")).unwrap();
        assert_eq!(fresh.index_status.indexed_notes, 2);
        assert_eq!(fresh.items.len(), 1);
    }

    #[test]
    fn warm_cache_updates_when_a_note_is_added() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Search").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        storage
            .create_note("alpha.md", "alpha first body", &note_id())
            .unwrap();
        let host = SearchHost::default();
        let first = search(&host, &storage, &vault_id, &SearchRequest::new("alpha")).unwrap();
        assert_eq!(first.items.len(), 1);
        let version = first.index_status.version;

        storage
            .create_note("beta.md", "beta second body", &note_id())
            .unwrap();
        let second = search(&host, &storage, &vault_id, &SearchRequest::new("beta")).unwrap();
        assert_eq!(second.items.len(), 1);
        assert_ne!(second.index_status.version, version);
        assert_eq!(second.index_status.indexed_notes, 2);
    }

    #[test]
    fn cache_reflects_a_saved_body_and_keeps_the_old_version_reachable_by_nothing() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Edits").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        let note = storage
            .create_note("edit.md", "before: alpha body", &note_id())
            .unwrap();
        let host = SearchHost::default();
        let first = search(&host, &storage, &vault_id, &SearchRequest::new("alpha")).unwrap();
        assert_eq!(first.items.len(), 1);
        let old_version = first.index_status.version;

        storage
            .save(
                &note.note_id,
                "after: beta body",
                note.local_revision,
                &note_id(),
            )
            .unwrap();
        let stale = search(&host, &storage, &vault_id, &SearchRequest::new("alpha")).unwrap();
        assert!(stale.items.is_empty(), "old text must not survive a save");
        let fresh = search(&host, &storage, &vault_id, &SearchRequest::new("beta")).unwrap();
        assert_eq!(fresh.items.len(), 1);
        assert_ne!(fresh.index_status.version, old_version);
    }

    #[test]
    fn warm_prepare_answers_in_phase_one_without_rereading_sources() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Warm").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        storage
            .create_note("warm.md", "warm alpha body", &note_id())
            .unwrap();
        let host = SearchHost::default();
        let request = SearchRequest::new("alpha");
        let prepared = host.prepare(&storage, &vault_id, &request).unwrap();
        assert!(
            prepared.warm_page.is_none(),
            "cold prepare must read sources"
        );
        assert_eq!(prepared.sources.len(), 1);
        let cold = host.finish(prepared, &request).unwrap();
        assert_eq!(cold.items.len(), 1);

        // Second call: the fingerprint matches, so phase one already answers
        // and phase two has no sources to parse. This is the warm path that
        // keeps the vault lock short (F12-R03).
        let warm = host.prepare(&storage, &vault_id, &request).unwrap();
        assert!(warm.warm_page.is_some());
        assert!(warm.sources.is_empty());
        let page = host.finish(warm, &request).unwrap();
        assert_eq!(page.items[0].note_id, cold.items[0].note_id);
    }

    #[test]
    fn snapshot_prepare_needs_only_the_root_and_captured_summaries() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Snapshot").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        storage
            .create_note("snapshot.md", "snapshot alpha body", &note_id())
            .unwrap();
        let summaries = storage.list().unwrap();
        let root = storage.root_path().to_path_buf();
        // Production releases the vault state lock (and therefore has no
        // storage instance) before reading sources. Dropping it here makes that
        // contract explicit: only the root and summaries are needed.
        drop(storage);

        let host = SearchHost::default();
        let request = SearchRequest::new("alpha");
        let prepared = host
            .prepare_snapshot(&root, &vault_id, &summaries, &request, true)
            .unwrap();
        assert_eq!(prepared.sources.len(), 1);
        let page = host.finish(prepared, &request).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].relative_path, "snapshot.md");
    }

    #[test]
    fn cold_index_reads_do_not_wait_for_the_writer_gate() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Gate").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        storage
            .create_note("gated.md", "gated alpha body", &note_id())
            .unwrap();
        let summaries = storage.list().unwrap();
        let root = storage.root_path().to_path_buf();
        drop(storage);

        // Another instance holds the writer gate. A cold index build must read
        // the snapshot without taking that gate, or a queued save would wait
        // for indexing (F12-R03).
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".opennote").join("writer.lock"))
            .unwrap();
        holder.try_lock().expect("test holds the writer gate");

        let host = SearchHost::default();
        let request = SearchRequest::new("alpha");
        let prepared = host
            .prepare_snapshot(&root, &vault_id, &summaries, &request, true)
            .unwrap();
        assert_eq!(prepared.sources.len(), 1);
        let page = host.finish(prepared, &request).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].relative_path, "gated.md");

        drop(holder);
    }

    #[test]
    fn snapshot_reads_reject_paths_outside_the_vault() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("inside.md"), "inside").unwrap();

        assert_eq!(
            VaultStorage::read_source_snapshot(root, "inside.md").unwrap(),
            "inside"
        );
        let error = VaultStorage::read_source_snapshot(root, "../outside-snapshot.md").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn phase_two_builds_from_captured_sources_and_the_next_query_rebuilds() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Split").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        let note = storage
            .create_note("split.md", "before: split alpha", &note_id())
            .unwrap();
        let host = SearchHost::default();

        // Phase one captures the pre-save text. The vault changes before phase
        // two, which proves phase two indexes the captured snapshot and does
        // not re-enter the vault: a rebuild cannot block a save (F12-R03).
        let request = SearchRequest::new("before");
        let prepared = host.prepare(&storage, &vault_id, &request).unwrap();
        storage
            .save(
                &note.note_id,
                "after: split beta",
                note.local_revision,
                &note_id(),
            )
            .unwrap();
        let page = host.finish(prepared, &request).unwrap();
        assert_eq!(page.items.len(), 1, "phase two indexes the captured text");

        // The next query sees the new fingerprint, rebuilds, and the old text
        // is gone, so a stale page can never become permanent.
        let next = search(&host, &storage, &vault_id, &SearchRequest::new("before")).unwrap();
        assert!(next.items.is_empty());
        let fresh = search(&host, &storage, &vault_id, &SearchRequest::new("beta")).unwrap();
        assert_eq!(fresh.items.len(), 1);
    }

    #[test]
    fn cache_is_scoped_to_one_vault() {
        let first_dir = tempdir().unwrap();
        let second_dir = tempdir().unwrap();
        let mut first = VaultStorage::create(first_dir.path(), "First").unwrap();
        let mut second = VaultStorage::create(second_dir.path(), "Second").unwrap();
        first
            .create_note("first.md", "first unique body", &note_id())
            .unwrap();
        second
            .create_note("second.md", "second unique body", &note_id())
            .unwrap();
        let host = SearchHost::default();
        let first_page = search(
            &host,
            &first,
            &first.vault().vault_id,
            &SearchRequest::new("first"),
        )
        .unwrap();
        assert_eq!(first_page.items.len(), 1);
        let second_page = search(
            &host,
            &second,
            &second.vault().vault_id,
            &SearchRequest::new("second"),
        )
        .unwrap();
        assert_eq!(second_page.items.len(), 1);
        assert_eq!(second_page.items[0].relative_path, "second.md");
    }

    #[test]
    fn query_errors_are_typed_for_the_ui() {
        let directory = tempdir().unwrap();
        let storage = VaultStorage::create(directory.path(), "Errors").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        let host = SearchHost::default();
        let error = search(
            &host,
            &storage,
            &vault_id,
            &SearchRequest::new("x".repeat(501)),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.message_key, "error.searchQueryTooLong");

        let mut request = SearchRequest::new("x");
        request.cursor = Some("v1:1:1:00".to_owned());
        let error = search(&host, &storage, &vault_id, &request).unwrap_err();
        assert_eq!(error.code, ErrorCode::VersionConflict);
        assert!(error.retryable);

        let mut request = SearchRequest::new("x");
        request.filters = SearchFilters::default();
        request.limit = Some(0);
        let error = search(&host, &storage, &vault_id, &request).unwrap_err();
        assert_eq!(error.message_key, "error.searchLimitInvalid");
    }

    #[test]
    fn cold_empty_queries_do_not_read_sources_or_publish_a_fake_ready_index() {
        let directory = tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Empty search").unwrap();
        storage
            .create_note("unread.md", "body", &note_id())
            .unwrap();
        let summaries = storage.list().unwrap();
        let host = SearchHost::default();
        let request = SearchRequest::new("  \t\n");
        let prepared = host
            .prepare_snapshot(
                directory.path(),
                &storage.vault().vault_id,
                &summaries,
                &request,
                true,
            )
            .unwrap();
        assert!(
            prepared.sources.is_empty(),
            "empty query must not read the note bodies"
        );
        assert!(prepared.cached_sources.is_empty());
        assert!(prepared.warm_page.is_some());
        let page = host.finish(prepared, &request).unwrap();
        assert!(page.items.is_empty());
        assert!(page.next_cursor.is_none());
        assert_eq!(page.index_status.state, IndexState::Unavailable);
        assert_eq!(host.status().unwrap().state, IndexState::Unavailable);

        for request in [
            SearchRequest::new(" ".repeat(501)),
            SearchRequest {
                limit: Some(0),
                ..SearchRequest::new("")
            },
            SearchRequest {
                cursor: Some("damaged".into()),
                ..SearchRequest::new("")
            },
        ] {
            assert!(host
                .prepare_snapshot(
                    directory.path(),
                    &storage.vault().vault_id,
                    &summaries,
                    &request,
                    true
                )
                .is_err());
        }
        let page = search(
            &host,
            &storage,
            &storage.vault().vault_id,
            &SearchRequest::new("body"),
        )
        .unwrap();
        assert_eq!(
            page.items.len(),
            1,
            "a later nonempty search still builds normally"
        );
        let empty = search(
            &host,
            &storage,
            &storage.vault().vault_id,
            &SearchRequest::new(""),
        )
        .unwrap();
        assert_eq!(empty.index_status.state, IndexState::Ready);
        assert_eq!(empty.index_status.indexed_notes, 1);
    }
}
