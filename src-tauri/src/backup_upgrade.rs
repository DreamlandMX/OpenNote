use super::*;
use onote_history_backup::SnapshotUpgradePlan;
use onote_vault_storage::SchemaUpgradeResult;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpgradePreview {
    source: String,
    plan_token: String,
    operation_id: Option<String>,
    snapshot: Snapshot,
    from_schema: u32,
    to_schema: u32,
    note_count: u64,
    #[serde(skip)]
    plan: SnapshotUpgradePlan,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpgradePreviews {
    current: Option<UpgradePreview>,
    pending: Vec<UpgradePreview>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Upgraded {
    pub(super) migration: SchemaUpgradeResult,
    pub(super) vault: Option<Vault>,
}
pub(super) fn reopen_required(value: &Upgraded) -> ServiceError {
    let mut failure = svc_err(ErrorCode::RescanRequired, "error.upgradeReopenRequired");
    failure.operation_id = Some(value.migration.operation_id.clone());
    failure.retryable = true;
    failure
}
#[derive(Debug)]
pub(super) struct PinnedUpgrade {
    token: String,
    selection: Selection,
    plan: SnapshotUpgradePlan,
    operation_id: Option<String>,
    created_at: std::time::Instant,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UpgradeWork {
    plan: SnapshotUpgradePlan,
    operation_id: Option<String>,
    settings_root: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UpgradeAbandonmentWork {
    operation_id: String,
}
fn view(plan: SnapshotUpgradePlan, operation_id: Option<String>) -> UpgradePreview {
    UpgradePreview {
        source: String::new(),
        plan_token: String::new(),
        operation_id,
        snapshot: plan.snapshot.clone(),
        from_schema: plan.from_schema,
        to_schema: plan.to_schema,
        note_count: plan.note_count,
        plan,
    }
}
pub(super) fn preview(
    backup: &BackupStore,
    storage: &VaultStorage,
    snapshot: Option<&str>,
    cancel: &AtomicBool,
) -> Result<UpgradePreviews, ServiceError> {
    let pending = backup
        .pending_upgrades(cancel)?
        .into_iter()
        .map(|p| view(p.plan, Some(p.operation_id)))
        .collect::<Vec<_>>();
    let current = if let Some(id) = snapshot {
        if !pending.is_empty() {
            return Err(svc_err(
                ErrorCode::VersionConflict,
                "error.upgradeInterrupted",
            ));
        }
        Some(view(backup.plan_upgrade(storage, id, cancel)?, None))
    } else {
        None
    };
    Ok(UpgradePreviews { current, pending })
}
impl Context {
    pub(super) fn pin_upgrades(
        &mut self,
        selection: &Selection,
        previews: &mut UpgradePreviews,
    ) -> Result<(), ServiceError> {
        if self.selection.as_ref() != Some(selection) {
            return Err(stale());
        }
        self.upgrades.clear();
        for value in previews
            .current
            .iter_mut()
            .chain(previews.pending.iter_mut())
        {
            let token = uuid::Uuid::new_v4().to_string();
            value.source = selection.source.display().to_string();
            self.upgrades.push(PinnedUpgrade {
                token: token.clone(),
                selection: selection.clone(),
                plan: value.plan.clone(),
                operation_id: value.operation_id.clone(),
                created_at: std::time::Instant::now(),
            });
            value.plan_token = token;
        }
        Ok(())
    }
    pub(super) fn upgrade_work(&self, token: Option<&str>) -> Result<Work, ServiceError> {
        let pinned = self.pinned_upgrade(token)?;
        Ok(Work::Upgrade(UpgradeWork {
            plan: pinned.plan.clone(),
            operation_id: pinned.operation_id.clone(),
            settings_root: self.settings_root.clone().ok_or_else(stale)?,
        }))
    }
    fn pinned_upgrade(&self, token: Option<&str>) -> Result<&PinnedUpgrade, ServiceError> {
        self.upgrades
            .iter()
            .find(|p| {
                Some(p.token.as_str()) == token
                    && p.created_at.elapsed() < std::time::Duration::from_secs(600)
                    && self.selection.as_ref() == Some(&p.selection)
            })
            .ok_or_else(|| svc_err(ErrorCode::VersionConflict, "error.upgradePreviewExpired"))
    }
    pub(super) fn upgrade_abandonment_work(
        &self,
        token: Option<&str>,
    ) -> Result<Work, ServiceError> {
        let pinned = self.pinned_upgrade(token)?;
        let operation_id = pinned
            .operation_id
            .clone()
            .ok_or_else(|| svc_err(ErrorCode::InvalidInput, "error.upgradeAbandonUnavailable"))?;
        Ok(Work::UpgradeAbandonment(UpgradeAbandonmentWork {
            operation_id,
        }))
    }
}
impl UpgradeWork {
    pub(super) fn execute(
        &self,
        job: &Job,
        backup: &BackupStore,
        storage: &VaultStorage,
    ) -> Result<SchemaUpgradeResult, ServiceError> {
        let operation = self.operation_id.clone().unwrap_or(job.view()?.job_id);
        crate::backup_preferences::with_retention(
            &self.settings_root,
            &job.selection.source,
            &job.selection.vault_id,
            job.selection.preferences_revision,
            job.selection.retained_snapshots,
            &job.selection.destination,
            || {
                backup.apply_upgrade(storage, &self.plan, &operation, &job.cancel, |progress| {
                    job.progress(progress)
                })
            },
        )
    }
}
impl UpgradeAbandonmentWork {
    pub(super) fn execute(
        &self,
        job: &Job,
        backup: &BackupStore,
        storage: &VaultStorage,
    ) -> Result<onote_history_backup::SnapshotUpgradeAbandonment, ServiceError> {
        backup.abandon_upgrade(storage, &self.operation_id, &job.cancel)
    }
}
pub(super) fn refresh_state(
    state: &mut AppState,
    job: &Job,
    result: &mut BackupResult,
) -> Result<(), ServiceError> {
    let BackupResult::Upgraded(upgraded) = result else {
        return Ok(());
    };
    let previous = scoped_vault_ref(state, &job.selection.vault_id)?;
    if std::fs::canonicalize(previous.root_path()).map_err(|_| stale())? != job.selection.source {
        return Err(stale());
    }
    let (vault, storage, recovery, history) = prepare_vault(job.selection.source.clone(), false)
        .map_err(|_| reopen_required(upgraded))?;
    if vault.vault_id != job.selection.vault_id
        || vault.schema_version != upgraded.migration.to_schema
    {
        return Err(stale());
    }
    state.vault = Some(storage);
    state.recovery = Some(recovery);
    state.history = Some(history);
    upgraded.vault = Some(vault);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    #[test]
    fn changed_device_preferences_refuse_registered_upgrade_and_invalidate_its_token() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let settings = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "settings scope").unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        std::fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let original = std::fs::read(&path).unwrap();
        let storage = VaultStorage::open(source.path()).unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let snapshot = backup
            .create(&storage, &op(), &AtomicBool::new(false), |_| {})
            .unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        context.bind_preferences(&storage, settings.path().into());
        let selection = context.select(&storage, destination.path().into()).unwrap();
        let selected = context.selected(&storage, &selection.selection_id).unwrap();
        let mut previews = preview(
            &backup,
            &storage,
            Some(&snapshot.snapshot_id),
            &AtomicBool::new(false),
        )
        .unwrap();
        context.pin_upgrades(&selected, &mut previews).unwrap();
        let token = previews.current.as_ref().unwrap().plan_token.clone();
        let (job, _) = context
            .start(
                selected.clone(),
                context.upgrade_work(Some(&token)).unwrap(),
                &op(),
            )
            .unwrap();
        crate::backup_preferences::set_retention(
            settings.path(),
            &selected.source,
            &selected.vault_id,
            selected.preferences_revision,
            3,
            &selected.destination,
        )
        .unwrap();
        assert_eq!(
            execute(&job, &storage).unwrap_err().code,
            ErrorCode::VersionConflict
        );
        job.finish(Err(stale()));
        context.refresh_preferences(&storage).unwrap();
        assert!(context.upgrade_work(Some(&token)).is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(!source.path().join(".opennote/upgrade-v1-v2").exists());
    }
    #[test]
    fn native_upgrade_resumes_original_operation_and_rebinds_history_before_success() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let settings = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(source.path(), "native upgrade").unwrap();
        let note = storage.create_note("a.md", "original", &op()).unwrap();
        drop(storage);
        let path = source.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        std::fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let storage = VaultStorage::open(source.path()).unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let snapshot = backup
            .create(&storage, &op(), &AtomicBool::new(false), |_| {})
            .unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        context.bind_preferences(&storage, settings.path().into());
        let selection = context.select(&storage, destination.path().into()).unwrap();
        let selected = context.selected(&storage, &selection.selection_id).unwrap();
        let (preview_job, _) = context
            .start(
                selected.clone(),
                Work::UpgradePreview {
                    snapshot_id: Some(snapshot.snapshot_id.clone()),
                },
                &op(),
            )
            .unwrap();
        let BackupResult::UpgradePreview(mut previews) = execute(&preview_job, &storage).unwrap()
        else {
            panic!("missing upgrade preview")
        };
        context.pin_upgrades(&selected, &mut previews).unwrap();
        let json = serde_json::to_value(&previews).unwrap();
        assert!(json["current"].get("plan").is_none());
        assert!(json["current"].get("planId").is_none());
        assert!(context.upgrade_work(Some("forged-token")).is_err());
        assert!(context
            .upgrade_abandonment_work(Some("forged-token"))
            .is_err());
        let token = previews.current.as_ref().unwrap().plan_token.clone();
        assert_eq!(
            context
                .upgrade_abandonment_work(Some(&token))
                .unwrap_err()
                .message_key,
            "error.upgradeAbandonUnavailable"
        );
        context.upgrades[0].created_at =
            std::time::Instant::now() - std::time::Duration::from_secs(601);
        assert!(context.upgrade_work(Some(&token)).is_err());
        assert!(context.upgrade_abandonment_work(Some(&token)).is_err());
        context.upgrades[0].created_at = std::time::Instant::now();
        context.selection = None;
        assert!(context.upgrade_work(Some(&token)).is_err());
        assert!(context.upgrade_abandonment_work(Some(&token)).is_err());
        context.selection = Some(selected.clone());
        preview_job.finish(Ok(BackupResult::UpgradePreview(previews)));
        let original = op();
        let work = context.upgrade_work(Some(&token)).unwrap();
        let (job, _) = context.start(selected.clone(), work, &original).unwrap();
        storage.push_failpoint(onote_vault_storage::Failpoint::UpgradeMetadata);
        job.finish(execute(&job, &storage));
        assert_eq!(job.view().unwrap().status, "failed");
        let mut state = AppState {
            vault: Some(storage),
            recovery: None,
            history: None,
        };
        let (discovery, _) = context
            .start(
                selected.clone(),
                Work::UpgradePreview { snapshot_id: None },
                &op(),
            )
            .unwrap();
        let BackupResult::UpgradePreview(mut pending) =
            execute(&discovery, state.vault.as_ref().unwrap()).unwrap()
        else {
            panic!("missing pending upgrade")
        };
        assert_eq!(
            pending.pending[0].operation_id.as_deref(),
            Some(original.as_str())
        );
        context.pin_upgrades(&selected, &mut pending).unwrap();
        let abandonment = context
            .upgrade_abandonment_work(Some(&pending.pending[0].plan_token))
            .unwrap();
        assert!(matches!(
            abandonment,
            Work::UpgradeAbandonment(UpgradeAbandonmentWork { .. })
        ));
        let work = context
            .upgrade_work(Some(&pending.pending[0].plan_token))
            .unwrap();
        discovery.finish(Ok(BackupResult::UpgradePreview(pending)));
        let retry = op();
        let (job, _) = context.start(selected, work, &retry).unwrap();
        let mut result = execute(&job, state.vault.as_ref().unwrap()).unwrap();
        let meta_path = source.path().join(".opennote/vault.json");
        let valid_meta = std::fs::read(&meta_path).unwrap();
        std::fs::write(&meta_path, b"temporarily unreadable metadata").unwrap();
        let failure = refresh_state(&mut state, &job, &mut result).unwrap_err();
        assert_eq!(failure.message_key, "error.upgradeReopenRequired");
        assert_eq!(failure.operation_id.as_deref(), Some(original.as_str()));
        assert!(!state.vault.as_ref().unwrap().vault().writable);
        std::fs::write(meta_path, valid_meta).unwrap();
        refresh_state(&mut state, &job, &mut result).unwrap();
        job.finish(Ok(result));
        let view = job.view().unwrap();
        assert_eq!(view.status, "succeeded");
        let Some(BackupResult::Upgraded(value)) = view.result else {
            panic!("missing upgrade result")
        };
        assert_eq!(value.migration.operation_id, original);
        assert_ne!(value.migration.operation_id, retry);
        assert!(value.vault.unwrap().writable);
        let storage = state.vault.as_mut().unwrap();
        assert_eq!(storage.vault().schema_version, 3);
        assert_eq!(storage.read(&note.note_id).unwrap().source, note.source);
        storage
            .save(&note.note_id, "new", note.local_revision, &op())
            .unwrap();
        assert_eq!(
            state
                .history
                .as_ref()
                .unwrap()
                .list(&note.note_id)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn finishing_without_session_rebind_cannot_report_success() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "unfinished rebind").unwrap();
        let mut context = Context::default();
        context.bind(&storage).unwrap();
        let selected = context.select(&storage, destination.path().into()).unwrap();
        let selected = context.selected(&storage, &selected.selection_id).unwrap();
        let operation = op();
        let (job, _) = context.start(selected, Work::List, &operation).unwrap();
        job.finish(Ok(BackupResult::Upgraded(Upgraded {
            migration: SchemaUpgradeResult {
                operation_id: operation,
                manifest_hash: "a".repeat(64),
                from_schema: 1,
                to_schema: 2,
                note_count: 0,
            },
            vault: None,
        })));
        assert_eq!(job.view().unwrap().status, "failed");
        assert_eq!(
            job.view().unwrap().error.unwrap().message_key,
            "error.upgradeReopenRequired"
        );
        assert!(job.view().unwrap().result.is_none());
    }
    #[test]
    fn pending_preview_token_abandons_original_request_and_replays_after_reopen() {
        use onote_vault_storage::Failpoint;

        for started in [false, true] {
            let source = tempfile::tempdir().unwrap();
            let destination = tempfile::tempdir().unwrap();
            let settings = tempfile::tempdir().unwrap();
            let storage = VaultStorage::create(source.path(), "abandon native").unwrap();
            drop(storage);
            let path = source.path().join(".opennote/vault.json");
            let mut meta: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            meta["schemaVersion"] = 1.into();
            std::fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
            let storage = VaultStorage::open(source.path()).unwrap();
            let backup =
                BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                    .unwrap();
            let snapshot = backup
                .create(&storage, &op(), &AtomicBool::new(false), |_| {})
                .unwrap();
            let mut context = Context::default();
            context.bind(&storage).unwrap();
            context.bind_preferences(&storage, settings.path().into());
            let selection = context.select(&storage, destination.path().into()).unwrap();
            let selected = context.selected(&storage, &selection.selection_id).unwrap();
            let (preview_job, _) = context
                .start(
                    selected.clone(),
                    Work::UpgradePreview {
                        snapshot_id: Some(snapshot.snapshot_id.clone()),
                    },
                    &op(),
                )
                .unwrap();
            let BackupResult::UpgradePreview(mut previews) =
                execute(&preview_job, &storage).unwrap()
            else {
                panic!("missing upgrade preview")
            };
            context.pin_upgrades(&selected, &mut previews).unwrap();
            let token = previews.current.as_ref().unwrap().plan_token.clone();
            let plan = previews.current.as_ref().unwrap().plan.clone();
            preview_job.finish(Ok(BackupResult::UpgradePreview(previews)));
            let original = op();
            let upgrade = context.upgrade_work(Some(&token)).unwrap();
            let (upgrade_job, _) = context.start(selected.clone(), upgrade, &original).unwrap();
            if started {
                storage.push_failpoint(Failpoint::UpgradePrepared);
                assert!(execute(&upgrade_job, &storage).is_err());
            } else {
                let failure = backup
                    .apply_upgrade(
                        &storage,
                        &plan,
                        &original,
                        &upgrade_job.cancel,
                        |progress| {
                            if progress.phase == "verifying" {
                                upgrade_job.cancel.store(true, Ordering::Relaxed);
                            }
                        },
                    )
                    .unwrap_err();
                assert_eq!(failure.code, ErrorCode::OperationCancelled);
                assert!(!source
                    .path()
                    .join(".opennote/upgrade-v1-v2/request.json")
                    .exists());
            }
            upgrade_job.finish(Err(stale()));
            let (discovery, _) = context
                .start(
                    selected.clone(),
                    Work::UpgradePreview { snapshot_id: None },
                    &op(),
                )
                .unwrap();
            let BackupResult::UpgradePreview(mut pending) = execute(&discovery, &storage).unwrap()
            else {
                panic!("missing pending upgrade")
            };
            context.pin_upgrades(&selected, &mut pending).unwrap();
            let token = pending.pending[0].plan_token.clone();
            discovery.finish(Ok(BackupResult::UpgradePreview(pending)));
            if started {
                let work = context.upgrade_abandonment_work(Some(&token)).unwrap();
                let (job, _) = context.start(selected, work, &op()).unwrap();
                assert_eq!(
                    execute(&job, &storage).unwrap_err().code,
                    ErrorCode::VersionConflict
                );
                job.finish(Err(stale()));
                assert!(!destination
                    .path()
                    .join(format!(".upgrade-journal-{original}/abandoned.json"))
                    .exists());
                assert!(context.upgrade_abandonment_work(Some(&token)).is_ok());
            }

            drop(storage);
            let storage = VaultStorage::open(source.path()).unwrap();
            let mut context = Context::default();
            context.bind(&storage).unwrap();
            context.bind_preferences(&storage, settings.path().into());
            let selection = context.select(&storage, destination.path().into()).unwrap();
            let selected = context.selected(&storage, &selection.selection_id).unwrap();
            let (discovery, _) = context
                .start(
                    selected.clone(),
                    Work::UpgradePreview { snapshot_id: None },
                    &op(),
                )
                .unwrap();
            let BackupResult::UpgradePreview(mut pending) = execute(&discovery, &storage).unwrap()
            else {
                panic!("missing pending upgrade")
            };
            context.pin_upgrades(&selected, &mut pending).unwrap();
            let token = pending.pending[0].plan_token.clone();
            assert_eq!(
                pending.pending[0].operation_id.as_deref(),
                Some(original.as_str())
            );
            discovery.finish(Ok(BackupResult::UpgradePreview(pending)));
            let operation = op();
            let work = context.upgrade_abandonment_work(Some(&token)).unwrap();
            let (job, _) = context.start(selected.clone(), work, &operation).unwrap();
            if started {
                assert_eq!(
                    execute(&job, &storage).unwrap_err().code,
                    ErrorCode::VersionConflict
                );
                assert!(!destination
                    .path()
                    .join(format!(".upgrade-journal-{original}/abandoned.json"))
                    .exists());
                assert_eq!(
                    backup
                        .pending_upgrades(&AtomicBool::new(false))
                        .unwrap()
                        .len(),
                    1
                );
                assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
                continue;
            }
            let BackupResult::UpgradeAbandoned(result) = execute(&job, &storage).unwrap() else {
                panic!("missing abandonment")
            };
            job.finish(Ok(BackupResult::UpgradeAbandoned(result.clone())));
            assert_eq!(result.operation_id, original);
            assert_eq!(result.snapshot_id, snapshot.snapshot_id);
            let backup =
                BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                    .unwrap();
            assert!(backup
                .pending_upgrades(&AtomicBool::new(false))
                .unwrap()
                .is_empty());
            assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
            assert_eq!(
                backup
                    .abandon_upgrade(&storage, &original, &AtomicBool::new(false))
                    .unwrap(),
                result
            );
            let (replay_discovery, _) = context
                .start(selected, Work::UpgradePreview { snapshot_id: None }, &op())
                .unwrap();
            let BackupResult::UpgradePreview(replay) =
                execute(&replay_discovery, &storage).unwrap()
            else {
                panic!("missing abandonment replay")
            };
            assert!(replay.current.is_none() && replay.pending.is_empty());
        }
    }
}
