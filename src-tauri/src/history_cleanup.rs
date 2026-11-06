use super::*;
use onote_history_backup::HistoryCleanupProgress;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryCleanupJob {
    pub job_id: String,
    pub operation_id: String,
    pub status: String,
    pub progress: HistoryCleanupProgress,
    pub cancel_requested: bool,
    pub result: Option<HistoryCleanupResult>,
    pub error: Option<ServiceError>,
}

struct Job {
    vault_id: String,
    cancel: AtomicBool,
    view: Mutex<HistoryCleanupJob>,
}

#[derive(Default)]
pub(crate) struct HistoryCleanupHost(Mutex<Option<Arc<Job>>>);

impl HistoryCleanupHost {
    pub(crate) fn start(
        &self,
        source: PathBuf,
        history: Arc<HistoryStore>,
        vault_id: String,
        operation_id: String,
        plan: Option<HistoryRetentionPlan>,
    ) -> Result<HistoryCleanupJob, ServiceError> {
        if !uuid::Uuid::parse_str(&operation_id).is_ok_and(|id| id.to_string() == operation_id) {
            return Err(svc_err(ErrorCode::InvalidInput, "error.invalidOperationId"));
        }
        let mut current = self.0.lock().map_err(|_| poison_err())?;
        if let Some(job) = current.as_ref() {
            if job.view.lock().map_err(|_| poison_err())?.status == "running" {
                return Err(svc_err(ErrorCode::FileLocked, "error.historyCleanupBusy"));
            }
        }
        let initial = HistoryCleanupJob {
            job_id: uuid::Uuid::new_v4().to_string(),
            operation_id: operation_id.clone(),
            status: "running".into(),
            progress: HistoryCleanupProgress {
                removed: 0,
                total: plan.as_ref().map_or(0, |plan| plan.candidates.len() as u64),
            },
            cancel_requested: false,
            result: None,
            error: None,
        };
        let job = Arc::new(Job {
            vault_id,
            cancel: AtomicBool::new(false),
            view: Mutex::new(initial.clone()),
        });
        *current = Some(job.clone());
        let spawned = std::thread::Builder::new()
            .name("history-cleanup".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let storage = VaultStorage::open(source)?;
                    if let Some(plan) = plan {
                        history.apply_cleanup(
                            &storage,
                            &plan,
                            &operation_id,
                            &job.cancel,
                            |progress| {
                                if let Ok(mut view) = job.view.lock() {
                                    view.progress = progress;
                                }
                            },
                        )
                    } else {
                        history.cancel_cleanup(&storage, &operation_id)
                    }
                }))
                .unwrap_or_else(|_| Err(task_err()));
                if let Ok(mut view) = job.view.lock() {
                    match outcome {
                        Ok(result) => {
                            view.status = result.status.clone();
                            view.progress.removed = result.removed.len() as u64;
                            view.progress.total =
                                (result.removed.len() + result.remaining.len()) as u64;
                            view.result = Some(result);
                        }
                        Err(error) => {
                            view.status = "failed".into();
                            view.error = Some(error);
                        }
                    }
                }
            });
        if spawned.is_err() {
            *current = None;
            return Err(task_err());
        }
        Ok(initial)
    }

    pub(crate) fn status(
        &self,
        vault_id: &str,
        job_id: &str,
        cancel: bool,
    ) -> Result<HistoryCleanupJob, ServiceError> {
        let current = self.0.lock().map_err(|_| poison_err())?;
        let job = current.as_ref().ok_or_else(no_vault_err)?;
        let mut view = job.view.lock().map_err(|_| poison_err())?;
        if job.vault_id != vault_id || view.job_id != job_id {
            return Err(svc_err(
                ErrorCode::InvalidInput,
                "error.vaultSessionChanged",
            ));
        }
        // Cancellation never takes AppState or the writer gate, so it can interrupt a running deletion.
        if cancel && view.status == "running" {
            job.cancel.store(true, Ordering::SeqCst);
            view.cancel_requested = true;
        }
        Ok(view.clone())
    }
}

pub(crate) fn start(
    app: &AppHandle,
    vault_id: &str,
    operation_id: String,
    plan: Option<HistoryRetentionPlan>,
) -> Result<HistoryCleanupJob, ServiceError> {
    let (source, history) = with_state(app, |state| {
        let storage = scoped_vault_ref(state, vault_id)?;
        if !storage.vault().writable {
            return Err(svc_err(ErrorCode::InvalidInput, "error.vaultReadOnly"));
        }
        Ok((
            storage.root_path().to_path_buf(),
            state.history.clone().ok_or_else(no_vault_err)?,
        ))
    })?;
    app.state::<HistoryCleanupHost>()
        .start(source, history, vault_id.into(), operation_id, plan)
}

#[tauri::command]
pub(crate) fn history_cleanup_status(
    app: AppHandle,
    vault_id: String,
    job_id: String,
) -> Result<HistoryCleanupJob, ServiceError> {
    app.state::<HistoryCleanupHost>()
        .status(&vault_id, &job_id, false)
}

#[tauri::command]
pub(crate) fn cancel_history_cleanup(
    app: AppHandle,
    vault_id: String,
    job_id: String,
) -> Result<HistoryCleanupJob, ServiceError> {
    app.state::<HistoryCleanupHost>()
        .status(&vault_id, &job_id, true)
}

#[tauri::command]
pub(crate) fn stop_history_cleanup(
    app: AppHandle,
    vault_id: String,
    operation_id: String,
) -> Result<HistoryCleanupJob, ServiceError> {
    start(&app, &vault_id, operation_id, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(host: &HistoryCleanupHost, vault: &str, job: &str) -> HistoryCleanupJob {
        let start = std::time::Instant::now();
        loop {
            let view = host.status(vault, job, false).unwrap();
            if view.status != "running" {
                return view;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn scoped_job_handles_only_request_cancellation_until_the_worker_finishes() {
        let host = HistoryCleanupHost::default();
        let job = Arc::new(Job {
            vault_id: "vault".into(),
            cancel: AtomicBool::new(false),
            view: Mutex::new(HistoryCleanupJob {
                job_id: "opaque-handle".into(),
                operation_id: "operation".into(),
                status: "running".into(),
                progress: HistoryCleanupProgress::default(),
                cancel_requested: false,
                result: None,
                error: None,
            }),
        });
        *host.0.lock().unwrap() = Some(job.clone());
        assert!(host.status("another-vault", "opaque-handle", true).is_err());
        assert!(host.status("vault", "wrong-handle", true).is_err());
        assert!(!job.cancel.load(Ordering::SeqCst));
        let requested = host.status("vault", "opaque-handle", true).unwrap();
        assert!(requested.cancel_requested);
        assert_eq!(requested.status, "running");
        assert!(requested.result.is_none());
        assert!(job.cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn worker_stops_a_durable_pending_request_after_settings_change() {
        let dir = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(dir.path(), "synthetic").unwrap();
        let vault_id = storage.vault().vault_id;
        let history = Arc::new(HistoryStore::open(dir.path(), &vault_id).unwrap());
        let plan = history.preview_cleanup(&storage).unwrap().current;
        let operation = uuid::Uuid::new_v4().to_string();
        let journal = dir
            .path()
            .join(".opennote/history")
            .join(format!(".cleanup-{operation}"));
        std::fs::create_dir_all(&journal).unwrap();
        std::fs::write(
            journal.join("request.json"),
            serde_json::to_vec(&serde_json::json!({
                "formatVersion": 1, "operationId": operation, "vaultId": vault_id, "plan": plan,
            }))
            .unwrap(),
        )
        .unwrap();
        history.set_retention(&storage, 45, 0).unwrap();
        let host = HistoryCleanupHost::default();
        let initial = host
            .start(
                dir.path().into(),
                history.clone(),
                vault_id.clone(),
                operation.clone(),
                None,
            )
            .unwrap();
        let terminal = wait(&host, &vault_id, &initial.job_id);
        assert_eq!(terminal.status, "cancelled");
        assert_eq!(terminal.result.unwrap().operation_id, operation);
        assert!(history.pending_cleanups().unwrap().is_empty());
        assert!(journal.join("request.json").exists());
        assert!(journal.join("result.json").exists());
    }
}
