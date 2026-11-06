use super::*;
use onote_domain::{Snapshot, SnapshotProgress};
use onote_history_backup::{
    BackupStore, DailyBackupResult, SnapshotRestoreCancellation, SnapshotRestorePlan,
    SnapshotRestoreResult, SnapshotRetentionPlan,
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
#[path = "backup_cleanup.rs"]
mod cleanup;
#[path = "backup_overwrite.rs"]
mod overwrite;
#[path = "backup_upgrade.rs"]
mod upgrade;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupSelection {
    selection_id: String,
    destination: String,
    available_bytes: Option<u64>,
    checked_at: Option<String>,
    retained_snapshots: u32,
    preferences_revision: u64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestorePreview {
    plan_token: String,
    snapshot: Snapshot,
    target: String,
    note_count: u64,
    reference_report: onote_history_backup::SnapshotReferenceReport,
    #[serde(skip)]
    plan: Option<SnapshotRestorePlan>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub(crate) enum BackupResult {
    Snapshots(Vec<Snapshot>),
    Snapshot(Snapshot),
    Preview(RestorePreview),
    Restored(SnapshotRestoreResult),
    Daily(DailyBackupResult),
    Retention(cleanup::RetentionPreview),
    Cleanup(onote_history_backup::SnapshotCleanupResult),
    UpgradePreview(upgrade::UpgradePreviews),
    Upgraded(upgrade::Upgraded),
    UpgradeAbandoned(onote_history_backup::SnapshotUpgradeAbandonment),
    OverwritePreview(overwrite::OverwritePreview),
    Overwritten(overwrite::Overwritten),
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupJobStatus {
    job_id: String,
    kind: String,
    status: String,
    progress: SnapshotProgress,
    cancel_requested: bool,
    result: Option<BackupResult>,
    error: Option<ServiceError>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupContextView {
    selection: Option<BackupSelection>,
    job: Option<BackupJobStatus>,
    saved_destination: Option<String>,
    configuration_error: Option<ServiceError>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    id: String,
    source: PathBuf,
    destination: PathBuf,
    vault_id: String,
    available_bytes: Option<u64>,
    checked_at: Option<String>,
    retained_snapshots: u32,
    preferences_revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum Work {
    List,
    Create,
    Daily,
    Retention {
        retained_snapshots: u32,
    },
    Cleanup(cleanup::CleanupWork),
    UpgradePreview {
        snapshot_id: Option<String>,
    },
    Upgrade(upgrade::UpgradeWork),
    UpgradeAbandonment(upgrade::UpgradeAbandonmentWork),
    OverwritePreview {
        snapshot_id: String,
    },
    Overwrite(overwrite::OverwriteWork),
    Preview {
        snapshot_id: String,
        target: PathBuf,
    },
    Restore {
        plan: SnapshotRestorePlan,
    },
}
impl Work {
    fn kind(&self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Create => "create",
            Self::Daily => "daily",
            Self::Retention { .. } => "retention",
            Self::Cleanup(work) => work.kind(),
            Self::UpgradePreview { .. } => "upgradePreview",
            Self::Upgrade(_) => "upgrade",
            Self::UpgradeAbandonment(_) => "upgradeAbandon",
            Self::OverwritePreview { .. } => "overwritePreview",
            Self::Overwrite(_) => "overwrite",
            Self::Preview { .. } => "preview",
            Self::Restore { .. } => "restore",
        }
    }
}
#[derive(Debug)]
struct Job {
    selection: Selection,
    work: Work,
    cancel: AtomicBool,
    status: Mutex<BackupJobStatus>,
}
impl Job {
    fn view(&self) -> Result<BackupJobStatus, ServiceError> {
        let mut view = self.status.lock().map_err(|_| poison_err())?.clone();
        view.cancel_requested = self.cancel.load(Ordering::Relaxed);
        Ok(view)
    }
    fn running(&self) -> Result<bool, ServiceError> {
        Ok(self.status.lock().map_err(|_| poison_err())?.status == "running")
    }
    fn progress(&self, progress: SnapshotProgress) {
        if let Ok(mut view) = self.status.lock() {
            view.progress = progress;
        }
    }
    fn finish(&self, result: Result<BackupResult, ServiceError>) {
        let result = match result {
            Ok(BackupResult::Upgraded(value)) if value.vault.is_none() => {
                Err(upgrade::reopen_required(&value))
            }
            Ok(BackupResult::Overwritten(value)) if value.vault.is_none() => {
                Err(overwrite::reopen_required(&value))
            }
            result => result,
        };
        if let Ok(mut view) = self.status.lock() {
            match result {
                Ok(value) => {
                    view.status = if matches!(&value, BackupResult::Cleanup(result) if result.status == "cancelled") { "cancelled" } else { "succeeded" }.into();
                    view.result = Some(value);
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
    }
}
#[derive(Debug, Default)]
pub(crate) struct BackupHost {
    context: Mutex<Context>,
}
#[derive(Debug, Default)]
struct Context {
    session: Option<(String, PathBuf)>,
    selection: Option<Selection>,
    job: Option<Arc<Job>>,
    plan: Option<PinnedPlan>,
    cleanup: Vec<cleanup::PinnedCleanup>,
    upgrades: Vec<upgrade::PinnedUpgrade>,
    overwrite: Option<overwrite::PinnedOverwrite>,
    settings_root: Option<PathBuf>,
    saved_destination: Option<String>,
    configuration_error: Option<ServiceError>,
}
#[derive(Debug)]
struct PinnedPlan {
    token: String,
    plan: SnapshotRestorePlan,
    created_at: std::time::Instant,
}
fn busy() -> ServiceError {
    let mut error = svc_err(ErrorCode::FileLocked, "error.backupBusy");
    error.retryable = true;
    error
}
fn stale() -> ServiceError {
    svc_err(ErrorCode::InvalidInput, "error.backupScopeChanged")
}
fn canonical_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
impl Context {
    fn idle(&self) -> Result<(), ServiceError> {
        if self
            .job
            .as_ref()
            .map(|job| job.running())
            .transpose()?
            .unwrap_or(false)
        {
            return Err(busy());
        }
        Ok(())
    }
    fn scope(&self, vault_id: &str) -> Result<(), ServiceError> {
        if self.session.as_ref().is_none_or(|(id, _)| id != vault_id) {
            Err(stale())
        } else {
            Ok(())
        }
    }
    fn bind(&mut self, storage: &VaultStorage) -> Result<(), ServiceError> {
        self.idle()?;
        self.session = Some((
            storage.vault().vault_id,
            std::fs::canonicalize(storage.root_path()).map_err(|_| stale())?,
        ));
        self.selection = None;
        self.plan = None;
        self.cleanup.clear();
        self.upgrades.clear();
        self.overwrite = None;
        self.job = None;
        self.settings_root = None;
        self.saved_destination = None;
        self.configuration_error = None;
        Ok(())
    }
    fn load_preferences(&mut self, storage: &VaultStorage) -> Result<(), ServiceError> {
        let root = self.settings_root.as_ref().ok_or_else(stale)?;
        let (vault_id, source) = self.session.as_ref().ok_or_else(stale)?;
        if let Some(value) = crate::backup_preferences::load(root, source, vault_id)? {
            self.saved_destination = Some(value.destination.display().to_string());
            self.activate_selection(storage, value.destination, false)?;
            if let Some(selection) = self.selection.as_mut() {
                selection.retained_snapshots = value.retained_snapshots;
                selection.preferences_revision = value.revision;
            }
        }
        Ok(())
    }
    fn bind_preferences(&mut self, storage: &VaultStorage, root: PathBuf) {
        self.settings_root = Some(root);
        self.configuration_error = self.load_preferences(storage).err();
    }
    fn refresh_preferences(&mut self, storage: &VaultStorage) -> Result<(), ServiceError> {
        if self
            .job
            .as_ref()
            .map(|job| job.running())
            .transpose()?
            .unwrap_or(false)
        {
            return Ok(());
        }
        let Some(root) = &self.settings_root else {
            return Ok(());
        };
        let (vault_id, source) = self.session.as_ref().ok_or_else(stale)?;
        let value = crate::backup_preferences::load(root, source, vault_id)?;
        if let Some(value) = value {
            if self.selection.as_ref().is_none_or(|selection| {
                selection.preferences_revision != value.revision
                    || selection.destination != value.destination
                    || selection.retained_snapshots != value.retained_snapshots
            }) {
                self.load_preferences(storage)?;
            }
        } else if self
            .selection
            .as_ref()
            .is_some_and(|selection| selection.preferences_revision > 0)
        {
            return Err(stale());
        }
        Ok(())
    }
    fn select(
        &mut self,
        storage: &VaultStorage,
        destination: PathBuf,
    ) -> Result<BackupSelection, ServiceError> {
        self.activate_selection(storage, destination, true)
    }
    fn activate_selection(
        &mut self,
        storage: &VaultStorage,
        destination: PathBuf,
        persist: bool,
    ) -> Result<BackupSelection, ServiceError> {
        self.idle()?;
        self.scope(&storage.vault().vault_id)?;
        let source = std::fs::canonicalize(storage.root_path()).map_err(|_| stale())?;
        if self
            .session
            .as_ref()
            .is_none_or(|(_, path)| *path != source)
        {
            return Err(stale());
        }
        let (destination, available_bytes, checked_at) = if persist {
            let backup = BackupStore::open(&source, &storage.vault().vault_id, &destination)?;
            let (available_bytes, checked_at) = backup.destination_space()?;
            (
                std::fs::canonicalize(destination).map_err(|_| stale())?,
                Some(available_bytes),
                Some(checked_at),
            )
        } else {
            // Remembered media is probed by a tracked task, never while opening the note vault.
            (destination, None, None)
        };
        let mut retained_snapshots = 7;
        let mut preferences_revision = 0;
        if persist {
            if let Some(root) = &self.settings_root {
                let preferences = crate::backup_preferences::choose(
                    root,
                    &source,
                    &storage.vault().vault_id,
                    &destination,
                )?;
                retained_snapshots = preferences.retained_snapshots;
                preferences_revision = preferences.revision;
                self.saved_destination = Some(destination.display().to_string());
            } else if let Some(failure) = &self.configuration_error {
                return Err(failure.clone());
            }
        }
        let selection = Selection {
            id: uuid::Uuid::new_v4().to_string(),
            source,
            destination,
            vault_id: storage.vault().vault_id,
            available_bytes,
            checked_at,
            retained_snapshots,
            preferences_revision,
        };
        let view = BackupSelection {
            selection_id: selection.id.clone(),
            destination: selection.destination.display().to_string(),
            available_bytes: selection.available_bytes,
            checked_at: selection.checked_at.clone(),
            retained_snapshots: selection.retained_snapshots,
            preferences_revision: selection.preferences_revision,
        };
        self.selection = Some(selection);
        self.plan = None;
        self.cleanup.clear();
        self.upgrades.clear();
        self.overwrite = None;
        self.job = None;
        self.configuration_error = None;
        Ok(view)
    }
    fn selected(
        &self,
        storage: &VaultStorage,
        selection_id: &str,
    ) -> Result<Selection, ServiceError> {
        let selection = self.selection.as_ref().ok_or_else(stale)?;
        if selection.id != selection_id
            || selection.vault_id != storage.vault().vault_id
            || std::fs::canonicalize(storage.root_path()).map_err(|_| stale())? != selection.source
        {
            return Err(stale());
        }
        Ok(selection.clone())
    }
    fn start(
        &mut self,
        selection: Selection,
        work: Work,
        operation_id: &str,
    ) -> Result<(Arc<Job>, bool), ServiceError> {
        if !canonical_id(operation_id) {
            return Err(svc_err(ErrorCode::InvalidInput, "error.invalidOperationId"));
        }
        if let Some(job) = &self.job {
            if job.view()?.job_id == operation_id {
                if job.selection == selection && job.work == work {
                    return Ok((job.clone(), false));
                }
                return Err(svc_err(
                    ErrorCode::IdempotencyMismatch,
                    "error.idempotencyMismatch",
                ));
            }
        }
        self.idle()?;
        let job = Arc::new(Job {
            selection,
            work: work.clone(),
            cancel: AtomicBool::new(false),
            status: Mutex::new(BackupJobStatus {
                job_id: operation_id.into(),
                kind: work.kind().into(),
                status: "running".into(),
                progress: SnapshotProgress {
                    phase: "queued".into(),
                    completed_objects: 0,
                    total_objects: 0,
                },
                cancel_requested: false,
                result: None,
                error: None,
            }),
        });
        self.job = Some(job.clone());
        Ok((job, true))
    }
    fn job(&self, vault_id: &str, job_id: &str) -> Result<Arc<Job>, ServiceError> {
        self.scope(vault_id)?;
        self.job
            .as_ref()
            .filter(|job| job.view().is_ok_and(|view| view.job_id == job_id))
            .cloned()
            .ok_or_else(|| svc_err(ErrorCode::ObjectNotFound, "error.backupJobMissing"))
    }
    fn restore_work(&self, token: Option<&str>) -> Result<Work, ServiceError> {
        let pinned = self
            .plan
            .as_ref()
            .filter(|pinned| {
                Some(pinned.token.as_str()) == token
                    && pinned.created_at.elapsed() < std::time::Duration::from_secs(600)
            })
            .ok_or_else(|| svc_err(ErrorCode::VersionConflict, "error.restorePreviewExpired"))?;
        Ok(Work::Restore {
            plan: pinned.plan.clone(),
        })
    }
}

pub(crate) fn bind_session(app: &AppHandle, storage: &VaultStorage) -> Result<(), ServiceError> {
    let host = app.state::<BackupHost>();
    let mut context = host.context.lock().map_err(|_| poison_err())?;
    context.bind(storage)?;
    match app.path().app_local_data_dir() {
        Ok(root) => {
            context.bind_preferences(storage, root.join("backup-preferences"));
        }
        Err(_) => {
            context.configuration_error = Some(svc_err(
                ErrorCode::IoDenied,
                "error.backupPreferencesUnavailable",
            ))
        }
    }
    Ok(())
}
pub(crate) fn require_idle(app: &AppHandle) -> Result<(), ServiceError> {
    app.state::<BackupHost>()
        .context
        .lock()
        .map_err(|_| poison_err())?
        .idle()
}
fn execute(job: &Job, storage: &VaultStorage) -> Result<BackupResult, ServiceError> {
    if storage.vault().vault_id != job.selection.vault_id
        || std::fs::canonicalize(storage.root_path()).map_err(|_| stale())? != job.selection.source
    {
        return Err(stale());
    }
    let backup = BackupStore::open(
        &job.selection.source,
        &job.selection.vault_id,
        &job.selection.destination,
    )?;
    let operation = job.view()?.job_id;
    let progress = |progress| job.progress(progress);
    match &job.work {
        Work::List => Ok(BackupResult::Snapshots(
            backup.list_with_cancel(&job.cancel, progress)?,
        )),
        Work::Retention { retained_snapshots } => Ok(BackupResult::Retention(cleanup::preview(
            &backup,
            *retained_snapshots,
            &job.cancel,
            progress,
        )?)),
        Work::Cleanup(work) => Ok(BackupResult::Cleanup(work.execute(job, &backup)?)),
        Work::UpgradePreview { snapshot_id } => Ok(BackupResult::UpgradePreview(upgrade::preview(
            &backup,
            storage,
            snapshot_id.as_deref(),
            &job.cancel,
        )?)),
        Work::Upgrade(work) => Ok(BackupResult::Upgraded(upgrade::Upgraded {
            migration: work.execute(job, &backup, storage)?,
            vault: None,
        })),
        Work::UpgradeAbandonment(work) => Ok(BackupResult::UpgradeAbandoned(
            work.execute(&job, &backup, storage)?,
        )),
        Work::OverwritePreview { snapshot_id } => {
            Ok(BackupResult::OverwritePreview(overwrite::preview(
                &backup,
                storage,
                snapshot_id,
                &operation,
                &job.cancel,
                progress,
            )?))
        }
        Work::Overwrite(_) => Err(task_err()),
        Work::Create => Ok(BackupResult::Snapshot(backup.create(
            storage,
            &operation,
            &job.cancel,
            progress,
        )?)),
        Work::Daily => {
            if !storage.vault().writable {
                return Err(svc_err(ErrorCode::IoDenied, "error.dailyBackupReadOnly"));
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| task_err())?
                .as_secs();
            Ok(BackupResult::Daily(backup.run_daily(
                storage,
                &operation,
                now,
                &job.cancel,
                progress,
            )?))
        }
        Work::Preview {
            snapshot_id,
            target,
        } => {
            let plan = backup.plan_restore(snapshot_id, target, &job.cancel)?;
            Ok(BackupResult::Preview(RestorePreview {
                plan_token: String::new(),
                snapshot: plan.snapshot.clone(),
                target: plan.target.display().to_string(),
                note_count: plan.note_count,
                reference_report: plan.reference_report.clone(),
                plan: Some(plan),
            }))
        }
        Work::Restore { plan } => {
            match backup.apply_restore(plan, &operation, &job.cancel, progress) {
                Ok(result) => Ok(BackupResult::Restored(result)),
                Err(error) if error.code == ErrorCode::OperationCancelled => {
                    if job
                        .selection
                        .destination
                        .join(format!(".restore-journal-{operation}"))
                        .exists()
                    {
                        match backup.cancel_restore(&operation)? {
                            SnapshotRestoreCancellation::Cancelled => (),
                            SnapshotRestoreCancellation::AlreadyApplied => {
                                return Ok(BackupResult::Restored(backup.apply_restore(
                                    plan,
                                    &operation,
                                    &job.cancel,
                                    progress,
                                )?))
                            }
                        }
                    }
                    Err(error)
                }
                Err(error) => Err(error),
            }
        }
    }
}
fn launch(app: AppHandle, job: Arc<Job>) {
    tauri::async_runtime::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_state(&app, |state| {
                if let Work::Overwrite(work) = &job.work {
                    return overwrite::execute(&app, state, &job, work);
                }
                let storage = scoped_vault_ref(state, &job.selection.vault_id)?;
                let mut result = execute(&job, storage)?;
                if let BackupResult::Preview(preview) = &mut result {
                    let plan = preview.plan.take().ok_or_else(stale)?;
                    let token = uuid::Uuid::new_v4().to_string();
                    app.state::<BackupHost>()
                        .context
                        .lock()
                        .map_err(|_| poison_err())?
                        .plan = Some(PinnedPlan {
                        token: token.clone(),
                        plan,
                        created_at: std::time::Instant::now(),
                    });
                    preview.plan_token = token;
                }
                if let BackupResult::Retention(preview) = &mut result {
                    app.state::<BackupHost>()
                        .context
                        .lock()
                        .map_err(|_| poison_err())?
                        .pin_cleanup(&job.selection, preview)?;
                }
                if let BackupResult::UpgradePreview(preview) = &mut result {
                    app.state::<BackupHost>()
                        .context
                        .lock()
                        .map_err(|_| poison_err())?
                        .pin_upgrades(&job.selection, preview)?;
                }
                if let BackupResult::OverwritePreview(preview) = &mut result {
                    app.state::<BackupHost>()
                        .context
                        .lock()
                        .map_err(|_| poison_err())?
                        .pin_overwrite(&job.selection, preview)?;
                }
                if matches!(&result, BackupResult::UpgradeAbandoned(_)) {
                    app.state::<BackupHost>()
                        .context
                        .lock()
                        .map_err(|_| poison_err())?
                        .upgrades
                        .clear();
                }
                if matches!(&result, BackupResult::Upgraded(_)) {
                    upgrade::refresh_state(state, &job, &mut result)?;
                    let host = app.state::<BackupHost>();
                    let mut context = host.context.lock().map_err(|_| poison_err())?;
                    context.plan = None;
                    context.cleanup.clear();
                    context.upgrades.clear();
                    context.overwrite = None;
                }
                Ok(result)
            })
        }))
        .unwrap_or_else(|_| Err(task_err()));
        job.finish(result);
    });
}
fn register(
    app: &AppHandle,
    vault_id: &str,
    selection_id: &str,
    operation_id: &str,
    make_work: impl FnOnce(&Context) -> Result<Work, ServiceError>,
) -> Result<BackupJobStatus, ServiceError> {
    let (job, start) = with_state(app, |state| {
        let storage = scoped_vault_ref(state, vault_id)?;
        let host = app.state::<BackupHost>();
        let mut context = host.context.lock().map_err(|_| poison_err())?;
        context.refresh_preferences(storage)?;
        let selection = context.selected(storage, selection_id)?;
        let work = make_work(&context)?;
        context.start(selection, work, operation_id)
    })?;
    let view = job.view()?;
    if start {
        launch(app.clone(), job);
    }
    Ok(view)
}

#[tauri::command]
pub(crate) fn backup_context(
    app: AppHandle,
    vault_id: String,
) -> Result<BackupContextView, ServiceError> {
    let host = app.state::<BackupHost>();
    let context = host.context.lock().map_err(|_| poison_err())?;
    context.scope(&vault_id)?;
    Ok(BackupContextView {
        selection: context.selection.as_ref().map(|s| BackupSelection {
            selection_id: s.id.clone(),
            destination: s.destination.display().to_string(),
            available_bytes: s.available_bytes,
            checked_at: s.checked_at.clone(),
            retained_snapshots: s.retained_snapshots,
            preferences_revision: s.preferences_revision,
        }),
        job: context.job.as_ref().map(|job| job.view()).transpose()?,
        saved_destination: context.saved_destination.clone(),
        configuration_error: context.configuration_error.clone(),
    })
}
#[tauri::command]
pub(crate) async fn set_backup_retention(
    app: AppHandle,
    vault_id: String,
    selection_id: String,
    expected_revision: u64,
    retained_snapshots: u32,
) -> Result<BackupSelection, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            let host = app.state::<BackupHost>();
            let mut context = host.context.lock().map_err(|_| poison_err())?;
            context.idle()?;
            let selection = context.selected(storage, &selection_id)?;
            let root = context.settings_root.as_ref().ok_or_else(stale)?;
            let preferences = crate::backup_preferences::set_retention(
                root,
                &selection.source,
                &vault_id,
                expected_revision,
                retained_snapshots,
                &selection.destination,
            )?;
            if preferences.destination != selection.destination {
                return Err(stale());
            }
            let selection = context.selection.as_mut().ok_or_else(stale)?;
            selection.retained_snapshots = preferences.retained_snapshots;
            selection.preferences_revision = preferences.revision;
            let view = BackupSelection {
                selection_id: selection.id.clone(),
                destination: selection.destination.display().to_string(),
                available_bytes: selection.available_bytes,
                checked_at: selection.checked_at.clone(),
                retained_snapshots: selection.retained_snapshots,
                preferences_revision: selection.preferences_revision,
            };
            context.job = None;
            context.cleanup.clear();
            context.upgrades.clear();
            context.overwrite = None;
            Ok(view)
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
pub(crate) async fn choose_backup_destination(
    app: AppHandle,
    vault_id: String,
    dialog_title: Option<String>,
) -> Result<Option<BackupSelection>, ServiceError> {
    {
        let host = app.state::<BackupHost>();
        let context = host.context.lock().map_err(|_| poison_err())?;
        context.scope(&vault_id)?;
        context.idle()?;
    }
    let title = localized_or_fallback(dialog_title, "Choose backup folder");
    let picker = app.clone();
    let picked =
        tauri::async_runtime::spawn_blocking(move || pick_vault_folder_blocking(&picker, &title))
            .await
            .map_err(map_join_err)?;
    let Some(path) = picked else {
        return Ok(None);
    };
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            app.state::<BackupHost>()
                .context
                .lock()
                .map_err(|_| poison_err())?
                .select(storage, path)
                .map(Some)
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
pub(crate) async fn start_backup_task(
    app: AppHandle,
    vault_id: String,
    selection_id: String,
    operation_id: String,
    kind: String,
    plan_token: Option<String>,
) -> Result<BackupJobStatus, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        register(
            &app,
            &vault_id,
            &selection_id,
            &operation_id,
            |context| match kind.as_str() {
                "list" => Ok(Work::List),
                "create" => Ok(Work::Create),
                "daily" => Ok(Work::Daily),
                "retention" => Ok(Work::Retention {
                    retained_snapshots: context
                        .selection
                        .as_ref()
                        .ok_or_else(stale)?
                        .retained_snapshots,
                }),
                "cleanup" => context.cleanup_work(plan_token.as_deref(), false),
                "cancelCleanup" => context.cleanup_work(plan_token.as_deref(), true),
                "upgrade" => context.upgrade_work(plan_token.as_deref()),
                "upgradeAbandon" => context.upgrade_abandonment_work(plan_token.as_deref()),
                "overwritePreview" => {
                    let snapshot_id =
                        plan_token.filter(|id| canonical_id(id)).ok_or_else(|| {
                            svc_err(ErrorCode::InvalidInput, "error.invalidSnapshotId")
                        })?;
                    Ok(Work::OverwritePreview { snapshot_id })
                }
                "overwrite" => context.overwrite_work(plan_token.as_deref()),
                "restore" => context.restore_work(plan_token.as_deref()),
                _ => Err(svc_err(ErrorCode::InvalidInput, "error.backupKind")),
            },
        )
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
pub(crate) async fn preview_backup_restore(
    app: AppHandle,
    vault_id: String,
    selection_id: String,
    operation_id: String,
    snapshot_id: String,
    target_name: String,
    dialog_title: Option<String>,
) -> Result<Option<BackupJobStatus>, ServiceError> {
    {
        let host = app.state::<BackupHost>();
        let context = host.context.lock().map_err(|_| poison_err())?;
        context.scope(&vault_id)?;
        context.idle()?;
    }
    if target_name.is_empty()
        || target_name.contains(['/', '\\', ':'])
        || target_name == "."
        || target_name == ".."
    {
        return Err(svc_err(ErrorCode::InvalidInput, "error.restoreTarget"));
    }
    let title = localized_or_fallback(dialog_title, "Choose restore parent folder");
    let picker = app.clone();
    let picked =
        tauri::async_runtime::spawn_blocking(move || pick_vault_folder_blocking(&picker, &title))
            .await
            .map_err(map_join_err)?;
    let Some(parent) = picked else {
        return Ok(None);
    };
    tauri::async_runtime::spawn_blocking(move || {
        register(&app, &vault_id, &selection_id, &operation_id, |_| {
            Ok(Work::Preview {
                snapshot_id,
                target: parent.join(target_name),
            })
        })
        .map(Some)
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
pub(crate) async fn preview_backup_upgrade(
    app: AppHandle,
    vault_id: String,
    selection_id: String,
    operation_id: String,
    snapshot_id: Option<String>,
) -> Result<BackupJobStatus, ServiceError> {
    if snapshot_id.as_ref().is_some_and(|id| !canonical_id(id)) {
        return Err(svc_err(ErrorCode::InvalidInput, "error.invalidSnapshotId"));
    }
    tauri::async_runtime::spawn_blocking(move || {
        register(&app, &vault_id, &selection_id, &operation_id, |_| {
            Ok(Work::UpgradePreview { snapshot_id })
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
pub(crate) async fn preview_backup_overwrite(
    app: AppHandle,
    vault_id: String,
    selection_id: String,
    operation_id: String,
    snapshot_id: String,
) -> Result<BackupJobStatus, ServiceError> {
    if !canonical_id(&snapshot_id) {
        return Err(svc_err(ErrorCode::InvalidInput, "error.invalidSnapshotId"));
    }
    tauri::async_runtime::spawn_blocking(move || {
        register(&app, &vault_id, &selection_id, &operation_id, |_| {
            Ok(Work::OverwritePreview { snapshot_id })
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
pub(crate) fn backup_job_status(
    app: AppHandle,
    vault_id: String,
    job_id: String,
) -> Result<BackupJobStatus, ServiceError> {
    app.state::<BackupHost>()
        .context
        .lock()
        .map_err(|_| poison_err())?
        .job(&vault_id, &job_id)?
        .view()
}
#[tauri::command]
pub(crate) fn cancel_backup_task(
    app: AppHandle,
    vault_id: String,
    job_id: String,
) -> Result<BackupJobStatus, ServiceError> {
    let job = app
        .state::<BackupHost>()
        .context
        .lock()
        .map_err(|_| poison_err())?
        .job(&vault_id, &job_id)?;
    job.cancel.store(true, Ordering::Relaxed);
    job.view()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_manual_backup_accepts_known_legacy_readonly_without_enabling_daily_writes() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "legacy host").unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        std::fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let storage = VaultStorage::open(source.path()).unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        let selection = context.select(&storage, destination.path().into()).unwrap();
        let selected = context.selected(&storage, &selection.selection_id).unwrap();
        let (job, _) = context
            .start(selected.clone(), Work::Create, &op())
            .unwrap();
        job.finish(execute(&job, &storage));
        let Some(BackupResult::Snapshot(snapshot)) = job.view().unwrap().result else {
            panic!("missing legacy snapshot")
        };
        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(job.view().unwrap().status, "succeeded");
        let (job, _) = context.start(selected, Work::Daily, &op()).unwrap();
        assert_eq!(
            execute(&job, &storage).unwrap_err().message_key,
            "error.dailyBackupReadOnly"
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(!storage.vault().writable);
    }

    #[test]
    fn retention_jobs_use_persisted_count_and_return_a_read_only_preview() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let settings = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "retention host").unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        context.bind_preferences(&storage, settings.path().into());
        let selection = context.select(&storage, destination.path().into()).unwrap();
        for _ in 0..3 {
            let selected = context.selected(&storage, &selection.selection_id).unwrap();
            let (job, _) = context.start(selected, Work::Create, &op()).unwrap();
            job.finish(execute(&job, &storage));
            assert_eq!(job.view().unwrap().status, "succeeded");
        }
        let selected = context.selection.as_ref().unwrap();
        let preferences = crate::backup_preferences::set_retention(
            settings.path(),
            &selected.source,
            &selected.vault_id,
            selected.preferences_revision,
            1,
            &selected.destination,
        )
        .unwrap();
        context.bind(&storage).unwrap();
        context.bind_preferences(&storage, settings.path().into());
        let selected = context.selection.clone().unwrap();
        assert_eq!(selected.retained_snapshots, 1);
        assert_eq!(selected.preferences_revision, preferences.revision);
        let before = std::fs::read_dir(destination.path()).unwrap().count();
        let (job, _) = context
            .start(
                selected.clone(),
                Work::Retention {
                    retained_snapshots: selected.retained_snapshots,
                },
                &op(),
            )
            .unwrap();
        job.finish(execute(&job, &storage));
        let view = job.view().unwrap();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["kind"], "retention");
        assert_eq!(json["result"]["kind"], "retention");
        let Some(BackupResult::Retention(plan)) = view.result else {
            panic!("expected preview");
        };
        assert_eq!(plan.current.plan.retained.len(), 1);
        assert_eq!(plan.current.plan.candidates.len(), 2);
        assert_eq!(
            std::fs::read_dir(destination.path()).unwrap().count(),
            before
        );
    }
    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    #[test]
    fn daily_jobs_use_native_time_and_serialize_the_due_result_for_the_frontend() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "daily host fixture").unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        let selection = context.select(&storage, destination.path().into()).unwrap();
        let selected = context.selected(&storage, &selection.selection_id).unwrap();
        let (job, _) = context.start(selected.clone(), Work::Daily, &op()).unwrap();
        job.finish(execute(&job, &storage));
        let view = job.view().unwrap();
        assert_eq!(view.status, "succeeded");
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["kind"], "daily");
        assert_eq!(json["result"]["kind"], "daily");
        assert_eq!(json["result"]["value"]["status"], "created");
        assert!(json["result"]["value"]["nextCheckAt"].is_u64());
        let (job, _) = context.start(selected, Work::Daily, &op()).unwrap();
        job.finish(execute(&job, &storage));
        let Some(BackupResult::Daily(DailyBackupResult::NotDue { .. })) =
            job.view().unwrap().result
        else {
            panic!("expected not due");
        };
    }
    #[test]
    fn reopening_remembers_destination_but_rotates_handles_and_unavailable_backup_does_not_replace_vault(
    ) {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let preferences = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "settings fixture").unwrap();
        let mut first = Context::default();
        first.bind(&storage).unwrap();
        first.bind_preferences(&storage, preferences.path().into());
        let old = first.select(&storage, destination.path().into()).unwrap();
        drop(first);
        let mut reopened = Context::default();
        reopened.bind(&storage).unwrap();
        reopened.bind_preferences(&storage, preferences.path().into());
        assert!(reopened.configuration_error.is_none());
        assert_eq!(
            reopened.saved_destination.as_deref(),
            Some(old.destination.as_str())
        );
        assert_ne!(reopened.selection.as_ref().unwrap().id, old.selection_id);
        assert!(reopened.selected(&storage, &old.selection_id).is_err());
        std::fs::remove_dir(destination.path()).unwrap();
        reopened.bind(&storage).unwrap();
        reopened.bind_preferences(&storage, preferences.path().into());
        assert!(reopened
            .selection
            .as_ref()
            .unwrap()
            .available_bytes
            .is_none());
        assert!(reopened.configuration_error.is_none());
        assert_eq!(
            reopened.saved_destination.as_deref(),
            Some(old.destination.as_str())
        );
        assert!(storage.list().unwrap().is_empty());
        let selected = reopened.selection.clone().unwrap();
        let (job, _) = reopened.start(selected, Work::List, &op()).unwrap();
        job.finish(execute(&job, &storage));
        assert_eq!(job.view().unwrap().status, "failed");
    }
    #[test]
    fn running_job_blocks_session_changes_but_status_and_cancel_remain_available() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "test").unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        let selection = context.select(&storage, destination.path().into()).unwrap();
        assert!(selection.available_bytes.is_some_and(|bytes| bytes > 0));
        assert!(selection
            .checked_at
            .as_ref()
            .is_some_and(|date| !date.is_empty()));
        let selected = context.selected(&storage, &selection.selection_id).unwrap();
        let operation = op();
        let (job, start) = context
            .start(selected.clone(), Work::Create, &operation)
            .unwrap();
        assert!(start);
        assert!(
            !context
                .start(selected.clone(), Work::Create, &operation)
                .unwrap()
                .1
        );
        assert_eq!(
            context
                .start(selected.clone(), Work::List, &operation)
                .unwrap_err()
                .code,
            ErrorCode::IdempotencyMismatch
        );
        assert_eq!(
            context.bind(&storage).unwrap_err().code,
            ErrorCode::FileLocked
        );
        assert_eq!(context.idle().unwrap_err().code, ErrorCode::FileLocked);
        let active = context.job(&storage.vault().vault_id, &operation).unwrap();
        active.cancel.store(true, Ordering::Relaxed);
        assert!(active.view().unwrap().cancel_requested);
        assert_eq!(
            context.job(&op(), &operation).unwrap_err().code,
            ErrorCode::InvalidInput
        );
        job.finish(execute(&job, &storage));
        assert_eq!(job.view().unwrap().status, "cancelled");
        context.idle().unwrap();
        context.bind(&storage).unwrap();
        assert!(context.selection.is_none());
        assert!(context.selected(&storage, &selection.selection_id).is_err());
    }
    #[test]
    fn copied_vault_identity_cannot_reuse_another_roots_selection() {
        let source = tempfile::tempdir().unwrap();
        let copied = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "original").unwrap();
        let copy = VaultStorage::create(copied.path(), "copy").unwrap();
        drop(copy);
        let path = copied.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        meta["vaultId"] = storage.vault().vault_id.clone().into();
        std::fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let copy = VaultStorage::open(copied.path()).unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        let selection = context.select(&storage, destination.path().into()).unwrap();
        assert_eq!(
            context
                .selected(&copy, &selection.selection_id)
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
        context.bind(&copy).unwrap();
        assert!(context.selected(&copy, &selection.selection_id).is_err());
    }
    #[test]
    fn create_preview_and_restore_execute_against_captured_scope_and_keep_plan_private() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(source.path(), "test").unwrap();
        let note = storage.create_note("a.md", "原文", &op()).unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        let selection = context.select(&storage, destination.path().into()).unwrap();
        let selected = context.selected(&storage, &selection.selection_id).unwrap();
        let (job, _) = context
            .start(selected.clone(), Work::Create, &op())
            .unwrap();
        job.finish(execute(&job, &storage));
        let Some(BackupResult::Snapshot(snapshot)) = job.view().unwrap().result else {
            panic!("missing snapshot")
        };
        let (job, _) = context
            .start(
                selected.clone(),
                Work::Preview {
                    snapshot_id: snapshot.snapshot_id,
                    target: parent.path().join("restored"),
                },
                &op(),
            )
            .unwrap();
        let BackupResult::Preview(mut preview) = execute(&job, &storage).unwrap() else {
            panic!("missing preview")
        };
        let json = serde_json::to_value(&preview).unwrap();
        assert!(json.get("plan").is_none());
        let plan = preview.plan.take().unwrap();
        job.finish(Ok(BackupResult::Preview(preview)));
        let token = op();
        context.plan = Some(PinnedPlan {
            token: token.clone(),
            plan: plan.clone(),
            created_at: std::time::Instant::now(),
        });
        assert!(context.restore_work(Some("wrong-token")).is_err());
        let work = context.restore_work(Some(&token)).unwrap();
        context.plan.as_mut().unwrap().created_at =
            std::time::Instant::now() - std::time::Duration::from_secs(601);
        assert_eq!(
            context.restore_work(Some(&token)).unwrap_err().code,
            ErrorCode::VersionConflict
        );
        let (job, _) = context.start(selected, work, &op()).unwrap();
        job.finish(execute(&job, &storage));
        assert_eq!(job.view().unwrap().status, "succeeded");
        let restored = VaultStorage::open(plan.target).unwrap();
        let document = restored.read(&note.note_id).unwrap();
        assert_eq!(document.source, note.source);
        assert_eq!(document.local_revision, 1);
    }
}
