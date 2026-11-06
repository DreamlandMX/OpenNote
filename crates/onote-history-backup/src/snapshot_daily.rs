use super::*;

const DAY: u64 = 86400;
const RETRY: u64 = 300;
const MAX_TIME: u64 = 253402300799;

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DailyBackupResult {
    NotDue {
        next_check_at: u64,
    },
    Created {
        snapshot: Snapshot,
        next_check_at: u64,
    },
    Unchanged {
        snapshot: Snapshot,
        next_check_at: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    #[test]
    fn publication_before_schedule_ack_replays_the_same_snapshot_after_restart() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "crash fixture").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let now = Utc::now().timestamp() as u64;
        let operation = op();
        let cancel = AtomicBool::new(false);
        let DailyBackupResult::Created { snapshot, .. } = backup
            .run_daily(&storage, &operation, now, &cancel, |_| {})
            .unwrap()
        else {
            panic!("expected creation");
        };
        let (_, directory) = backup.daily_scope().unwrap();
        let state = backup.daily_state(&directory).unwrap();
        assert!(state.pending.is_none());
        fs::remove_file(directory.join(format!("{:020}.json", state.revision))).unwrap();
        drop(backup);
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let DailyBackupResult::Created {
            snapshot: replay, ..
        } = backup
            .run_daily(&storage, &op(), now + RETRY, &AtomicBool::new(true), |_| {})
            .unwrap()
        else {
            panic!("expected replayed creation");
        };
        assert_eq!(replay, snapshot);
        assert_eq!(backup.list().unwrap().len(), 1);
        assert!(backup.daily_state(&directory).unwrap().pending.is_none());
    }
    #[test]
    fn damaged_schedule_state_is_not_reset_and_the_last_snapshot_remains_readable() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "corruption fixture").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let now = Utc::now().timestamp() as u64;
        let cancel = AtomicBool::new(false);
        let DailyBackupResult::Created { snapshot, .. } = backup
            .run_daily(&storage, &op(), now, &cancel, |_| {})
            .unwrap()
        else {
            panic!("expected creation");
        };
        let (_, directory) = backup.daily_scope().unwrap();
        let state = backup.daily_state(&directory).unwrap();
        let path = directory.join(format!("{:020}.json", state.revision));
        fs::write(&path, b"{}").unwrap();
        assert_eq!(
            backup
                .run_daily(&storage, &op(), now + DAY, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::BackupIncomplete
        );
        assert_eq!(fs::read(path).unwrap(), b"{}");
        assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Anchor {
    id: String,
    manifest_hash: String,
}
impl Anchor {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            id: snapshot.snapshot_id.clone(),
            manifest_hash: snapshot.manifest_hash.clone(),
        }
    }
    fn valid(&self) -> bool {
        canonical_uuid(&self.id) && valid_hash(&self.manifest_hash)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pending {
    operation_id: String,
    baseline: Option<Anchor>,
    started_at: u64,
    last_attempt: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    format_version: u32,
    revision: u64,
    source: PathBuf,
    vault_id: String,
    last_check: Option<u64>,
    last_snapshot: Option<Anchor>,
    pending: Option<Pending>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    state: State,
    checksum: String,
}
fn waiting(now: u64, previous: u64, delay: u64) -> bool {
    now >= previous && now - previous < delay
}

impl BackupStore {
    pub(super) fn daily_dependencies(&self) -> Result<HashSet<String>, ServiceError> {
        let (_, directory) = self.daily_scope()?;
        if !directory.exists() {
            return Ok(HashSet::new());
        }
        let state = self.daily_state(&directory)?;
        let mut protected = HashSet::new();
        if let Some(anchor) = state.last_snapshot {
            protected.insert(anchor.id);
        }
        if let Some(pending) = state.pending {
            if let Some(anchor) = pending.baseline {
                protected.insert(anchor.id);
            }
        }
        Ok(protected)
    }
    fn daily_scope(&self) -> Result<(String, PathBuf), ServiceError> {
        let key =
            hash(&serde_json::to_vec(&(&self.source, &self.vault_id)).map_err(|_| incomplete())?);
        Ok((
            format!("daily-{key}"),
            self.destination.join(format!(".daily-{key}")),
        ))
    }
    fn daily_state(&self, directory: &Path) -> Result<State, ServiceError> {
        safe_io::check(&self.destination, directory)?;
        let mut revision = 0;
        for entry in fs::read_dir(directory).map_err(safe_io::io)? {
            let name = entry.map_err(safe_io::io)?.file_name();
            if let Some(number) = name
                .to_str()
                .and_then(|s| s.strip_suffix(".json"))
                .filter(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()))
            {
                revision = revision.max(number.parse::<u64>().map_err(|_| incomplete())?);
            }
        }
        if revision == 0 {
            return Ok(State {
                format_version: 1,
                revision: 0,
                source: self.source.clone(),
                vault_id: self.vault_id.clone(),
                last_check: None,
                last_snapshot: None,
                pending: None,
            });
        }
        let path = directory.join(format!("{revision:020}.json"));
        safe_io::check(&self.destination, &path)?;
        if fs::metadata(&path).map_err(safe_io::io)?.len() > 1024 * 1024 {
            return Err(incomplete());
        }
        let record: Record = serde_json::from_slice(&safe_io::read(&self.destination, &path)?)
            .map_err(|_| incomplete())?;
        let state = record.state;
        if state.format_version != 1
            || state.revision != revision
            || revision > 9_007_199_254_740_991
            || state.source != self.source
            || state.vault_id != self.vault_id
            || state.last_check.is_some_and(|time| time > MAX_TIME)
            || state
                .last_snapshot
                .as_ref()
                .is_some_and(|anchor| !anchor.valid())
            || state.pending.as_ref().is_some_and(|pending| {
                !canonical_uuid(&pending.operation_id)
                    || pending.started_at > MAX_TIME
                    || pending.last_attempt > MAX_TIME
                    || pending
                        .baseline
                        .as_ref()
                        .is_some_and(|anchor| !anchor.valid())
            })
            || hash(&serde_json::to_vec(&state).map_err(|_| incomplete())?) != record.checksum
        {
            return Err(incomplete());
        }
        Ok(state)
    }
    fn save_daily_state(&self, directory: &Path, state: &mut State) -> Result<(), ServiceError> {
        let revision = state
            .revision
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or_else(incomplete)?;
        let mut next = state.clone();
        next.revision = revision;
        let record = Record {
            checksum: hash(&serde_json::to_vec(&next).map_err(|_| incomplete())?),
            state: next,
        };
        safe_io::publish(
            &self.destination,
            &directory.join(format!("{revision:020}.json")),
            &serde_json::to_vec(&record).map_err(|_| incomplete())?,
        )?;
        *state = record.state;
        Ok(())
    }
    fn latest_owned_snapshot(
        &self,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotProgress),
    ) -> Result<Option<Snapshot>, ServiceError> {
        let mut snapshots = self.list_with_cancel(cancel, progress)?;
        let order: BTreeMap<_, _> = self
            .publication_history()?
            .into_iter()
            .enumerate()
            .map(|(index, snapshot)| (snapshot.snapshot_id, index))
            .collect();
        snapshots.sort_by(|a, b| retention::snapshot_order(a, b, &order));
        for snapshot in snapshots {
            cancelled(cancel)?;
            if self.snapshot_from_source(&snapshot.snapshot_id)? {
                return Ok(Some(snapshot));
            }
        }
        Ok(None)
    }
    fn finish_daily(
        &self,
        storage: &VaultStorage,
        directory: &Path,
        state: &mut State,
        now: u64,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotProgress),
    ) -> Result<DailyBackupResult, ServiceError> {
        let pending = state.pending.clone().ok_or_else(incomplete)?;
        if let Some(anchor) = &pending.baseline {
            if self.verify(&anchor.id)?.manifest_hash != anchor.manifest_hash {
                return Err(incomplete());
            }
        }
        let result = self.create_inner(
            storage,
            &pending.operation_id,
            true,
            pending.baseline.as_ref().map(|anchor| anchor.id.as_str()),
            cancel,
            progress,
        );
        let snapshot = match result {
            Ok(snapshot) => snapshot,
            Err(failure) => {
                if failure.code == ErrorCode::OperationCancelled {
                    state.pending = None;
                    state.last_check = Some(now);
                    self.save_daily_state(directory, state)?;
                }
                return Err(failure);
            }
        };
        state.last_check = Some(now);
        state.last_snapshot = Some(Anchor::from(&snapshot));
        state.pending = None;
        self.save_daily_state(directory, state)?;
        let next_check_at = now.saturating_add(DAY);
        if snapshot.snapshot_id == pending.operation_id {
            Ok(DailyBackupResult::Created {
                snapshot,
                next_check_at,
            })
        } else {
            Ok(DailyBackupResult::Unchanged {
                snapshot,
                next_check_at,
            })
        }
    }
    fn cancel_daily(
        &self,
        storage: &VaultStorage,
        directory: &Path,
        state: &mut State,
        now: u64,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotProgress),
    ) -> Result<DailyBackupResult, ServiceError> {
        if let Some(pending) = &state.pending {
            for path in [
                self.directory(&pending.operation_id)?,
                self.destination
                    .join(format!(".snapshot-unchanged-{}.json", pending.operation_id)),
            ] {
                match fs::symlink_metadata(path) {
                    Ok(_) => {
                        return self.finish_daily(storage, directory, state, now, cancel, progress)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(safe_io::io(e)),
                }
            }
        }
        state.pending = None;
        state.last_check = Some(now);
        self.save_daily_state(directory, state)?;
        Err(error(
            ErrorCode::OperationCancelled,
            "error.operationCancelled",
        ))
    }
    pub fn run_daily(
        &self,
        storage: &VaultStorage,
        operation_id: &str,
        now: u64,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<DailyBackupResult, ServiceError> {
        let _destination_guard = self.lock_operation("destination")?;
        if !canonical_uuid(operation_id)
            || now > MAX_TIME
            || storage.vault().vault_id != self.vault_id
            || fs::canonicalize(storage.root_path()).map_err(safe_io::io)? != self.source
        {
            return Err(error(ErrorCode::InvalidInput, "error.dailyBackupScope"));
        }
        self.not_cleanup_operation(operation_id)?;
        let (key, directory) = self.daily_scope()?;
        let _guard = self.lock_operation(&key)?;
        safe_io::ensure_dir(&self.destination, &directory)?;
        let mut state = self.daily_state(&directory)?;
        if cancel.load(Ordering::Relaxed) {
            return self.cancel_daily(storage, &directory, &mut state, now, cancel, progress);
        }
        let clock_rolled_back = state.last_check.is_some_and(|last| now < last)
            || state
                .pending
                .as_ref()
                .is_some_and(|pending| now < pending.last_attempt);
        if let Some(pending) = &state.pending {
            if waiting(now, pending.last_attempt, RETRY) {
                return Ok(DailyBackupResult::NotDue {
                    next_check_at: pending.last_attempt.saturating_add(RETRY),
                });
            }
        } else if let Some(last) = state.last_check {
            if waiting(now, last, DAY) {
                return Ok(DailyBackupResult::NotDue {
                    next_check_at: last.saturating_add(DAY),
                });
            }
        }
        let latest = match self.latest_owned_snapshot(cancel, &mut progress) {
            Ok(latest) => latest,
            Err(failure) if failure.code == ErrorCode::OperationCancelled => {
                return self.cancel_daily(storage, &directory, &mut state, now, cancel, progress)
            }
            Err(failure) => return Err(failure),
        };
        if cancel.load(Ordering::Relaxed) {
            return self.cancel_daily(storage, &directory, &mut state, now, cancel, progress);
        }
        // A newer successful manual backup can replace an interrupted daily baseline without deleting its staging.
        if let Some(snapshot) = &latest {
            let created = chrono::DateTime::parse_from_rfc3339(&snapshot.created_at)
                .ok()
                .and_then(|date| u64::try_from(date.timestamp()).ok());
            if let Some(created) = created {
                let replaces_pending = state.pending.as_ref().is_none_or(|pending| {
                    snapshot.snapshot_id != pending.operation_id && created >= pending.started_at
                });
                if !clock_rolled_back && replaces_pending && waiting(now, created, DAY) {
                    state.last_check = Some(created);
                    state.last_snapshot = Some(Anchor::from(snapshot));
                    state.pending = None;
                    self.save_daily_state(&directory, &mut state)?;
                    return Ok(DailyBackupResult::NotDue {
                        next_check_at: created.saturating_add(DAY),
                    });
                }
            }
        }
        if let Some(pending) = state.pending.as_mut() {
            pending.last_attempt = now;
        } else {
            state.pending = Some(Pending {
                operation_id: operation_id.into(),
                baseline: latest.as_ref().map(Anchor::from),
                started_at: now,
                last_attempt: now,
            });
        }
        self.save_daily_state(&directory, &mut state)?;
        self.finish_daily(storage, &directory, &mut state, now, cancel, progress)
    }
}
