use super::*;
use onote_history_backup::{
    SnapshotOverwritePlan, SnapshotOverwriteResult, SnapshotReferenceReport,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OverwritePreview {
    source: String,
    plan_token: String,
    operation_id: String,
    snapshot: Snapshot,
    safety_snapshot: Snapshot,
    target: String,
    previous_root: String,
    note_count: u64,
    reference_report: SnapshotReferenceReport,
    #[serde(skip)]
    plan: SnapshotOverwritePlan,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Overwritten {
    pub(super) result: SnapshotOverwriteResult,
    pub(super) vault: Option<Vault>,
}

#[derive(Debug)]
pub(super) struct PinnedOverwrite {
    token: String,
    selection: Selection,
    plan: SnapshotOverwritePlan,
    created_at: std::time::Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverwriteWork {
    plan: SnapshotOverwritePlan,
}

fn view(plan: SnapshotOverwritePlan) -> OverwritePreview {
    OverwritePreview {
        source: plan.source.display().to_string(),
        plan_token: String::new(),
        operation_id: plan.operation_id.clone(),
        snapshot: plan.restore.snapshot.clone(),
        safety_snapshot: plan.safety_snapshot.clone(),
        target: plan.source.display().to_string(),
        previous_root: plan.previous_root.display().to_string(),
        note_count: plan.note_count,
        reference_report: plan.restore.reference_report.clone(),
        plan,
    }
}

pub(super) fn reopen_required(value: &Overwritten) -> ServiceError {
    let mut failure = svc_err(ErrorCode::RescanRequired, "error.overwriteReopen");
    failure.operation_id = Some(value.result.operation_id.clone());
    failure.retryable = true;
    failure
}

impl Context {
    pub(super) fn pin_overwrite(
        &mut self,
        selection: &Selection,
        preview: &mut OverwritePreview,
    ) -> Result<(), ServiceError> {
        if self.selection.as_ref() != Some(selection) {
            return Err(stale());
        }
        let token = uuid::Uuid::new_v4().to_string();
        self.overwrite = Some(PinnedOverwrite {
            token: token.clone(),
            selection: selection.clone(),
            plan: preview.plan.clone(),
            created_at: std::time::Instant::now(),
        });
        preview.plan_token = token;
        Ok(())
    }

    pub(super) fn overwrite_work(&self, token: Option<&str>) -> Result<Work, ServiceError> {
        let pinned = self
            .overwrite
            .as_ref()
            .filter(|pinned| {
                Some(pinned.token.as_str()) == token
                    && pinned.created_at.elapsed() < std::time::Duration::from_secs(600)
                    && self.selection.as_ref() == Some(&pinned.selection)
            })
            .ok_or_else(|| svc_err(ErrorCode::VersionConflict, "error.overwritePreviewExpired"))?;
        Ok(Work::Overwrite(OverwriteWork {
            plan: pinned.plan.clone(),
        }))
    }
}

pub(super) fn preview(
    backup: &BackupStore,
    storage: &VaultStorage,
    snapshot_id: &str,
    operation: &str,
    cancel: &AtomicBool,
    progress: impl FnMut(SnapshotProgress),
) -> Result<OverwritePreview, ServiceError> {
    Ok(view(backup.plan_overwrite(
        storage,
        snapshot_id,
        operation,
        cancel,
        progress,
    )?))
}

fn clear_after_overwrite(app: &AppHandle) -> Result<(), ServiceError> {
    let host = app.state::<BackupHost>();
    let mut context = host.context.lock().map_err(|_| poison_err())?;
    context.plan = None;
    context.cleanup.clear();
    context.upgrades.clear();
    context.overwrite = None;
    Ok(())
}

pub(super) fn execute(
    app: &AppHandle,
    state: &mut AppState,
    job: &Job,
    work: &OverwriteWork,
) -> Result<BackupResult, ServiceError> {
    let backup = BackupStore::open(
        &job.selection.source,
        &job.selection.vault_id,
        &job.selection.destination,
    )?;
    let progress = |progress| job.progress(progress);
    {
        let storage = scoped_vault_ref(state, &job.selection.vault_id)?;
        backup.stage_overwrite(storage, &work.plan, &job.cancel, progress)?;
    }

    state.vault = None;
    state.recovery = None;
    state.history = None;

    let committed = backup.commit_overwrite(&work.plan, &job.cancel, progress);
    let reopened = prepare_vault(job.selection.source.clone(), false);
    match (committed, reopened) {
        (Ok(result), Ok((vault, storage, recovery, history)))
            if vault.vault_id == job.selection.vault_id
                && vault.schema_version == 3
                && vault.writable
                && !vault.sync_enabled =>
        {
            state.vault = Some(storage);
            state.recovery = Some(recovery);
            state.history = Some(history);
            clear_after_overwrite(app)?;
            Ok(BackupResult::Overwritten(Overwritten {
                result,
                vault: Some(vault),
            }))
        }
        (Ok(result), _) => {
            clear_after_overwrite(app)?;
            Err(reopen_required(&Overwritten {
                result,
                vault: None,
            }))
        }
        (Err(failure), Ok((vault, storage, recovery, history)))
            if vault.vault_id == job.selection.vault_id
                && vault.schema_version == 3
                && vault.writable =>
        {
            state.vault = Some(storage);
            state.recovery = Some(recovery);
            state.history = Some(history);
            clear_after_overwrite(app)?;
            Err(failure)
        }
        (Err(failure), _) => {
            clear_after_overwrite(app)?;
            Err(failure)
        }
    }
}
