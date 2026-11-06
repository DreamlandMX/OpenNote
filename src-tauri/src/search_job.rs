use crate::{svc_err, ErrorCode, ServiceError};
use onote_search::SearchPage;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    Queued,
    LoadingIndex,
    Reading,
    Indexing,
    Querying,
    SavingIndex,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchJobView {
    pub job_id: String,
    pub status: String,
    pub phase: Phase,
    pub processed: usize,
    pub total: usize,
    pub unreadable: usize,
    pub cancel_requested: bool,
    pub page: Option<SearchPage>,
    pub error: Option<ServiceError>,
}

pub(crate) struct Job {
    vault_id: String,
    cancel: AtomicBool,
    view: Mutex<SearchJobView>,
}

impl Job {
    pub(crate) fn progress(
        &self,
        phase: Phase,
        processed: usize,
        total: usize,
        unreadable: usize,
    ) -> bool {
        if self.cancel.load(Ordering::SeqCst) {
            return false;
        }
        let Ok(mut view) = self.view.lock() else {
            return false;
        };
        view.phase = phase;
        view.processed = processed;
        view.total = total;
        view.unreadable = unreadable;
        !self.cancel.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub(crate) struct SearchJobHost(Mutex<Vec<Arc<Job>>>);

fn invalid() -> ServiceError {
    svc_err(ErrorCode::InvalidInput, "error.searchJobInvalid")
}
fn poisoned() -> ServiceError {
    svc_err(ErrorCode::RescanRequired, "error.searchIndexUnavailable")
}
fn canonical(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
}

impl SearchJobHost {
    pub(crate) fn start(
        &self,
        vault_id: String,
        total: usize,
        run: impl FnOnce(&Job) -> Result<SearchPage, ServiceError> + Send + 'static,
    ) -> Result<SearchJobView, ServiceError> {
        if !canonical(&vault_id) {
            return Err(invalid());
        }
        let mut jobs = self.0.lock().map_err(|_| poisoned())?;
        if jobs.len() >= 32 {
            let index = jobs
                .iter()
                .position(|job| job.view.lock().is_ok_and(|view| view.status != "running"))
                .ok_or_else(|| svc_err(ErrorCode::FileLocked, "error.searchIndexUnavailable"))?;
            jobs.remove(index);
        }
        let initial = SearchJobView {
            job_id: uuid::Uuid::new_v4().to_string(),
            status: "running".into(),
            phase: Phase::Queued,
            processed: 0,
            total,
            unreadable: 0,
            cancel_requested: false,
            page: None,
            error: None,
        };
        let job = Arc::new(Job {
            vault_id,
            cancel: AtomicBool::new(false),
            view: Mutex::new(initial.clone()),
        });
        jobs.push(job.clone());
        let worker = job.clone();
        if std::thread::Builder::new()
            .name("search-index".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&worker)))
                        .unwrap_or_else(|_| Err(poisoned()));
                if let Ok(mut view) = worker.view.lock() {
                    match result {
                        Ok(page) => {
                            view.status = "completed".into();
                            view.page = Some(page);
                        }
                        Err(error) => {
                            view.status = if error.code == ErrorCode::OperationCancelled {
                                "cancelled"
                            } else {
                                "failed"
                            }
                            .into();
                            view.error = Some(error);
                        }
                    }
                }
            })
            .is_err()
        {
            jobs.retain(|item| !Arc::ptr_eq(item, &job));
            return Err(poisoned());
        }
        Ok(initial)
    }

    pub(crate) fn status(
        &self,
        vault_id: &str,
        job_id: &str,
        cancel: bool,
    ) -> Result<SearchJobView, ServiceError> {
        if !canonical(vault_id) || !canonical(job_id) {
            return Err(invalid());
        }
        let jobs = self.0.lock().map_err(|_| poisoned())?;
        let job = jobs
            .iter()
            .find(|job| {
                job.vault_id == vault_id && job.view.lock().is_ok_and(|view| view.job_id == job_id)
            })
            .ok_or_else(invalid)?;
        let mut view = job.view.lock().map_err(|_| poisoned())?;
        if cancel && view.status == "running" {
            job.cancel.store(true, Ordering::SeqCst);
            view.cancel_requested = true;
        }
        Ok(view.clone())
    }

    pub(crate) fn cancel_all(&self) -> Result<(), ServiceError> {
        let mut jobs = self.0.lock().map_err(|_| poisoned())?;
        for job in jobs.iter() {
            job.cancel.store(true, Ordering::SeqCst);
            if let Ok(mut view) = job.view.lock() {
                if view.status == "running" {
                    view.cancel_requested = true;
                }
            }
        }
        jobs.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{self, SearchHost};
    use onote_search::{SearchIndex, SearchRequest};
    use onote_vault_storage::VaultStorage;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn cancelled() -> ServiceError {
        svc_err(ErrorCode::OperationCancelled, "error.searchCancelled")
    }
    fn page() -> SearchPage {
        SearchIndex::rebuild(&[], 1, 0)
            .query(&SearchRequest::new(""))
            .unwrap()
    }
    fn wait(host: &SearchJobHost, vault: &str, job: &str) -> SearchJobView {
        let started = Instant::now();
        loop {
            let view = host.status(vault, job, false).unwrap();
            if view.status != "running" {
                return view;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn cancellation_does_not_wait_for_a_search_lock_and_preserves_source_and_old_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "Search cancellation").unwrap();
        storage.create_note("a.md", "alpha", &id()).unwrap();
        let vault = storage.vault().vault_id.clone();
        let host = Arc::new(Mutex::new(SearchHost::default()));
        let old = search::search(
            &host.lock().unwrap(),
            &storage,
            &vault,
            &SearchRequest::new("alpha"),
        )
        .unwrap();
        storage.create_note("b.md", "beta", &id()).unwrap();
        let before = std::fs::read(dir.path().join("a.md")).unwrap();
        let root = dir.path().to_path_buf();
        let summaries = storage.list().unwrap();
        let (ready, received) = mpsc::channel();
        let (resume, continued) = mpsc::channel();
        let jobs = Arc::new(SearchJobHost::default());
        let worker_host = host.clone();
        let worker_vault = vault.clone();
        let initial = jobs
            .start(vault.clone(), 2, move |job| {
                let mut paused = false;
                let mut progress = |phase, done, total, unreadable| {
                    if !paused {
                        paused = true;
                        ready.send(()).unwrap();
                        continued.recv().unwrap();
                    }
                    job.progress(phase, done, total, unreadable)
                };
                let request = SearchRequest::new("beta");
                let prepared = worker_host.lock().unwrap().prepare_controlled(
                    &root,
                    &worker_vault,
                    &summaries,
                    &request,
                    true,
                    &mut progress,
                )?;
                let result = worker_host.lock().unwrap().finish_controlled(
                    prepared,
                    &request,
                    &mut progress,
                );
                result
            })
            .unwrap();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            host.try_lock().is_err(),
            "worker really holds the search lock"
        );
        let cancelling_jobs = jobs.clone();
        let cancelling_vault = vault.clone();
        let job_id = initial.job_id.clone();
        let (done, ack) = mpsc::channel();
        std::thread::spawn(move || {
            done.send(cancelling_jobs.status(&cancelling_vault, &job_id, true))
                .unwrap()
        });
        let requested = ack.recv_timeout(Duration::from_secs(2));
        resume.send(()).unwrap();
        let requested = requested.unwrap().unwrap();
        assert!(requested.cancel_requested);
        assert_eq!(requested.status, "running");
        assert!(requested.page.is_none());
        let terminal = wait(&jobs, &vault, &initial.job_id);
        assert_eq!(terminal.status, "cancelled");
        assert!(terminal.page.is_none());
        assert_eq!(host.lock().unwrap().status().unwrap(), old.index_status);
        assert_eq!(std::fs::read(dir.path().join("a.md")).unwrap(), before);
        let fresh = search::search(
            &host.lock().unwrap(),
            &storage,
            &vault,
            &SearchRequest::new("beta"),
        )
        .unwrap();
        assert_eq!(fresh.items.len(), 1);
    }

    #[test]
    fn terminal_status_scope_and_session_clear_never_claim_cancelled_success() {
        let host = SearchJobHost::default();
        let vault = id();
        let initial = host.start(vault.clone(), 0, |_| Ok(page())).unwrap();
        let complete = wait(&host, &vault, &initial.job_id);
        assert_eq!(complete.status, "completed");
        assert_eq!(
            host.status(&vault, &initial.job_id, true).unwrap().status,
            "completed"
        );
        assert!(host.status(&id(), &initial.job_id, true).is_err());
        assert!(host.status(&vault, "bad", true).is_err());
        let failed = host
            .start(vault.clone(), 0, |_| {
                Err(svc_err(ErrorCode::IoDenied, "test"))
            })
            .unwrap();
        assert_eq!(wait(&host, &vault, &failed.job_id).status, "failed");
        host.cancel_all().unwrap();
        assert!(host.status(&vault, &initial.job_id, false).is_err());
    }

    #[test]
    fn registry_is_bounded_without_evicting_live_jobs() {
        let host = SearchJobHost::default();
        let vault = id();
        let mut resume = Vec::new();
        let mut jobs = Vec::new();
        for _ in 0..32 {
            let (tx, rx) = mpsc::channel();
            resume.push(tx);
            jobs.push(
                host.start(vault.clone(), 1, move |job| {
                    rx.recv().unwrap();
                    if !job.progress(Phase::Reading, 0, 1, 0) {
                        Err(cancelled())
                    } else {
                        Ok(page())
                    }
                })
                .unwrap(),
            );
        }
        assert_eq!(
            host.start(vault.clone(), 0, |_| Ok(page()))
                .unwrap_err()
                .code,
            ErrorCode::FileLocked
        );
        for job in &jobs {
            host.status(&vault, &job.job_id, true).unwrap();
        }
        for tx in resume {
            tx.send(()).unwrap();
        }
        for job in &jobs {
            assert_eq!(wait(&host, &vault, &job.job_id).status, "cancelled");
        }
        let newer = host.start(vault.clone(), 0, |_| Ok(page())).unwrap();
        assert_eq!(host.0.lock().unwrap().len(), 32);
        assert_eq!(wait(&host, &vault, &newer.job_id).status, "completed");
    }

    #[test]
    fn switching_sessions_invalidates_old_handles_and_signals_a_live_worker() {
        let host = SearchJobHost::default();
        let vault = id();
        let (resume, received) = mpsc::channel();
        let (observed, result) = mpsc::channel();
        let initial = host
            .start(vault.clone(), 1, move |job| {
                received.recv().unwrap();
                let keep_going = job.progress(Phase::Reading, 0, 1, 0);
                observed.send(keep_going).unwrap();
                if keep_going {
                    Ok(page())
                } else {
                    Err(cancelled())
                }
            })
            .unwrap();
        host.cancel_all().unwrap();
        assert!(host.status(&vault, &initial.job_id, false).is_err());
        resume.send(()).unwrap();
        assert!(!result.recv_timeout(Duration::from_secs(2)).unwrap());
        let replacement = host.start(vault.clone(), 0, |_| Ok(page())).unwrap();
        assert_eq!(wait(&host, &vault, &replacement.job_id).status, "completed");
    }
}
