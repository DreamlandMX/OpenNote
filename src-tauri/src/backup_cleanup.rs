use super::*;
use onote_history_backup::SnapshotCleanupResult;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CleanupPreview {
    pub(super) plan_token: String,
    pub(super) operation_id: Option<String>,
    #[serde(flatten)]
    pub(super) plan: SnapshotRetentionPlan,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RetentionPreview {
    pub(super) current: CleanupPreview,
    pub(super) pending: Vec<CleanupPreview>,
}
#[derive(Debug)]
pub(super) struct PinnedCleanup {
    token: String,
    selection: Selection,
    plan: SnapshotRetentionPlan,
    operation_id: Option<String>,
    created_at: std::time::Instant,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CleanupWork {
    plan: SnapshotRetentionPlan,
    operation_id: Option<String>,
    settings_root: PathBuf,
    cancel: bool,
}
pub(super) fn preview(
    backup: &BackupStore,
    count: u32,
    cancel: &AtomicBool,
    progress: impl FnMut(SnapshotProgress),
) -> Result<RetentionPreview, ServiceError> {
    let pending = backup
        .pending_cleanups()?
        .into_iter()
        .map(|entry| CleanupPreview {
            plan_token: String::new(),
            operation_id: Some(entry.operation_id),
            plan: entry.plan,
        })
        .collect();
    Ok(RetentionPreview {
        current: CleanupPreview {
            plan_token: String::new(),
            operation_id: None,
            plan: backup.plan_retention(count, cancel, progress)?,
        },
        pending,
    })
}
impl Context {
    pub(super) fn pin_cleanup(
        &mut self,
        selection: &Selection,
        view: &mut RetentionPreview,
    ) -> Result<(), ServiceError> {
        if self.selection.as_ref() != Some(selection) {
            return Err(stale());
        }
        self.cleanup.clear();
        for preview in std::iter::once(&mut view.current).chain(view.pending.iter_mut()) {
            let token = uuid::Uuid::new_v4().to_string();
            self.cleanup.push(PinnedCleanup {
                token: token.clone(),
                selection: selection.clone(),
                plan: preview.plan.clone(),
                operation_id: preview.operation_id.clone(),
                created_at: std::time::Instant::now(),
            });
            preview.plan_token = token;
        }
        Ok(())
    }
    pub(super) fn cleanup_work(
        &self,
        token: Option<&str>,
        cancel: bool,
    ) -> Result<Work, ServiceError> {
        let pinned = self
            .cleanup
            .iter()
            .find(|pinned| {
                Some(pinned.token.as_str()) == token
                    && pinned.created_at.elapsed() < std::time::Duration::from_secs(600)
                    && self.selection.as_ref() == Some(&pinned.selection)
            })
            .ok_or_else(|| svc_err(ErrorCode::VersionConflict, "error.cleanupPreviewExpired"))?;
        if cancel && pinned.operation_id.is_none() {
            return Err(stale());
        }
        if !cancel && pinned.plan.retained_snapshots != pinned.selection.retained_snapshots {
            return Err(svc_err(
                ErrorCode::VersionConflict,
                "error.backupPreferencesInvalid",
            ));
        }
        Ok(Work::Cleanup(CleanupWork {
            plan: pinned.plan.clone(),
            operation_id: pinned.operation_id.clone(),
            settings_root: self.settings_root.clone().ok_or_else(stale)?,
            cancel,
        }))
    }
}
impl CleanupWork {
    pub(super) fn kind(&self) -> &'static str {
        if self.cancel {
            "cancelCleanup"
        } else {
            "cleanup"
        }
    }
    pub(super) fn execute(
        &self,
        job: &Job,
        backup: &BackupStore,
    ) -> Result<SnapshotCleanupResult, ServiceError> {
        let operation = self.operation_id.clone().unwrap_or(job.view()?.job_id);
        if self.cancel {
            return backup.resume_cleanup(&operation, &AtomicBool::new(true), |progress| {
                job.progress(progress)
            });
        }
        crate::backup_preferences::with_retention(
            &self.settings_root,
            &job.selection.source,
            &job.selection.vault_id,
            job.selection.preferences_revision,
            self.plan.retained_snapshots,
            &job.selection.destination,
            || {
                backup.apply_cleanup(&self.plan, &operation, &job.cancel, |progress| {
                    job.progress(progress)
                })
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    #[test]
    fn cleanup_tokens_expire_and_fresh_settings_are_checked_before_deletion() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let settings = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "native cleanup").unwrap();
        let root = std::fs::canonicalize(source.path()).unwrap();
        let dest = std::fs::canonicalize(destination.path()).unwrap();
        let prefs = crate::backup_preferences::choose(
            settings.path(),
            &root,
            &storage.vault().vault_id,
            &dest,
        )
        .unwrap();
        crate::backup_preferences::set_retention(
            settings.path(),
            &root,
            &storage.vault().vault_id,
            prefs.revision,
            1,
            &dest,
        )
        .unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        context.bind_preferences(&storage, settings.path().into());
        let selection = context.selection.clone().unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        for _ in 0..3 {
            backup
                .create(&storage, &op(), &AtomicBool::new(false), |_| {})
                .unwrap();
        }
        let mut view = preview(&backup, 1, &AtomicBool::new(false), |_| {}).unwrap();
        context.pin_cleanup(&selection, &mut view).unwrap();
        assert_eq!(view.current.plan.candidates.len(), 2);
        assert_eq!(
            context
                .cleanup_work(Some("forged"), false)
                .unwrap_err()
                .code,
            ErrorCode::VersionConflict
        );
        let work = context
            .cleanup_work(Some(&view.current.plan_token), false)
            .unwrap();
        crate::backup_preferences::set_retention(
            settings.path(),
            &root,
            &storage.vault().vault_id,
            selection.preferences_revision,
            2,
            &dest,
        )
        .unwrap();
        let (job, _) = context.start(selection, work, &op()).unwrap();
        job.finish(super::super::execute(&job, &storage));
        assert_eq!(
            job.view().unwrap().error.unwrap().code,
            ErrorCode::VersionConflict
        );
        assert_eq!(backup.list().unwrap().len(), 3);
        context.cleanup[0].created_at =
            std::time::Instant::now() - std::time::Duration::from_secs(601);
        assert_eq!(
            context
                .cleanup_work(Some(&view.current.plan_token), false)
                .unwrap_err()
                .code,
            ErrorCode::VersionConflict
        );
        context.refresh_preferences(&storage).unwrap();
        let selection = context.selection.clone().unwrap();
        let mut fresh = preview(
            &backup,
            selection.retained_snapshots,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        context.pin_cleanup(&selection, &mut fresh).unwrap();
        let work = context
            .cleanup_work(Some(&fresh.current.plan_token), false)
            .unwrap();
        let (job, _) = context.start(selection, work, &op()).unwrap();
        job.finish(super::super::execute(&job, &storage));
        let Some(BackupResult::Cleanup(result)) = job.view().unwrap().result else {
            panic!("missing cleanup result");
        };
        assert_eq!(result.status, "completed");
        assert_eq!(result.removed.len(), 1);
        assert_eq!(backup.list().unwrap().len(), 2);
    }
    #[test]
    fn interrupted_cleanup_can_be_discovered_and_cancelled_with_real_partial_result() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let settings = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "native interrupted cleanup").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        for _ in 0..3 {
            backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        }
        let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        let operation = op();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = backup.apply_cleanup(&plan, &operation, &cancel, |p| {
                if p.phase == "cleaning" {
                    panic!("interrupted");
                }
            });
        }))
        .is_err());
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        context.bind_preferences(&storage, settings.path().into());
        context.select(&storage, destination.path().into()).unwrap();
        let selection = context.selection.clone().unwrap();
        let mut view = preview(&backup, selection.retained_snapshots, &cancel, |_| {}).unwrap();
        assert_eq!(view.pending.len(), 1);
        context.pin_cleanup(&selection, &mut view).unwrap();
        let work = context
            .cleanup_work(Some(&view.pending[0].plan_token), true)
            .unwrap();
        let (job, _) = context.start(selection, work, &op()).unwrap();
        job.finish(super::super::execute(&job, &storage));
        let status = job.view().unwrap();
        assert_eq!(status.status, "cancelled");
        let Some(BackupResult::Cleanup(result)) = status.result else {
            panic!("missing cancellation result");
        };
        assert_eq!(result.operation_id, operation);
        assert_eq!(result.quarantined.len(), 1);
        assert_eq!(result.remaining.len(), 1);
        assert_eq!(
            backup.verify(&plan.retained[0].snapshot_id).unwrap(),
            plan.retained[0]
        );
    }
}
