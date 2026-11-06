use super::*;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Publication {
    format_version: u32,
    source: PathBuf,
    vault_id: String,
    revision: u64,
    pub(super) snapshot: Snapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) supersedes: Vec<(String, String)>,
}
impl BackupStore {
    fn publication_directory(&self) -> Result<PathBuf, ServiceError> {
        let key =
            hash(&serde_json::to_vec(&(&self.source, &self.vault_id)).map_err(|_| incomplete())?);
        Ok(self
            .destination
            .join(format!(".snapshot-publications-{key}")))
    }
    pub(super) fn publication_records(&self) -> Result<Vec<Publication>, ServiceError> {
        let directory = self.publication_directory()?;
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(safe_io::io(e)),
        };
        safe_io::check(&self.destination, &directory)?;
        let mut records = Vec::new();
        let mut ids = HashSet::new();
        for entry in entries {
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(revision) = name
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .filter(|name| name.len() == 20 && name.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|name| name.parse::<u64>().ok())
            else {
                continue;
            };
            safe_io::check(&self.destination, &entry.path())?;
            if entry.metadata().map_err(safe_io::io)?.len() > 1024 * 1024 {
                return Err(incomplete());
            }
            let record: Publication =
                restore::read_record(&self.destination, &entry.path()).map_err(|_| incomplete())?;
            if record.format_version != 1
                || record.source != self.source
                || record.vault_id != self.vault_id
                || record.revision != revision
                || revision == 0
                || revision > 9_007_199_254_740_991
                || record.snapshot.vault_id != self.vault_id
                || !canonical_uuid(&record.snapshot.snapshot_id)
                || !valid_hash(&record.snapshot.manifest_hash)
                || record.snapshot.completion_state != "complete"
                || chrono::DateTime::parse_from_rfc3339(&record.snapshot.created_at).is_err()
                || !ids.insert(record.snapshot.snapshot_id.clone())
            {
                return Err(incomplete());
            }
            let mut predecessors = HashSet::new();
            if record.supersedes.iter().any(|(id, digest)| {
                !canonical_uuid(id)
                    || id == &record.snapshot.snapshot_id
                    || !valid_hash(digest)
                    || !predecessors.insert(id)
            }) {
                return Err(incomplete());
            }
            records.push(record);
        }
        records.sort_by_key(|record| record.revision);
        Ok(records)
    }
    pub(super) fn publication_history(&self) -> Result<Vec<Snapshot>, ServiceError> {
        Ok(self
            .publication_records()?
            .into_iter()
            .map(|record| record.snapshot)
            .collect())
    }
    pub(super) fn reserve_publication(&self, snapshot: &Snapshot) -> Result<(), ServiceError> {
        let records = self.publication_records()?;
        if let Some(record) = records
            .iter()
            .find(|record| record.snapshot.snapshot_id == snapshot.snapshot_id)
        {
            return if record.snapshot == *snapshot {
                Ok(())
            } else {
                Err(incomplete())
            };
        }
        let revision = records
            .last()
            .map(|record| record.revision)
            .unwrap_or(0)
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or_else(incomplete)?;
        let mut supersedes = Vec::new();
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            let entry = entry.map_err(safe_io::io)?;
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .filter(|id| canonical_uuid(id) && *id != snapshot.snapshot_id)
            else {
                continue;
            };
            if records
                .iter()
                .any(|record| record.snapshot.snapshot_id == id)
                || self.not_removed(id).is_err()
                || !self.snapshot_from_source(id).unwrap_or(false)
            {
                continue;
            }
            if let Ok(envelope) = self.envelope(&entry.path(), id) {
                supersedes.push((id.into(), envelope.checksum));
            }
        }
        supersedes.sort();
        let record = Publication {
            format_version: 1,
            source: self.source.clone(),
            vault_id: self.vault_id.clone(),
            revision,
            snapshot: snapshot.clone(),
            supersedes,
        };
        let bytes = restore::record_bytes(&record).map_err(|_| incomplete())?;
        if bytes.len() > 1024 * 1024 {
            return Err(incomplete());
        }
        safe_io::publish(
            &self.destination,
            &self
                .publication_directory()?
                .join(format!("{revision:020}.json")),
            &bytes,
        )
    }
}
#[cfg(test)]
thread_local! { pub(super) static TEST_TIME: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

#[cfg(test)]
mod tests {
    use super::*;
    fn op() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_TIME.with(|value| *value.borrow_mut() = None);
        }
    }
    #[test]
    fn clock_rollback_does_not_make_the_latest_success_a_cleanup_candidate() {
        let _reset = Reset;
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "clock order").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        TEST_TIME.with(|value| *value.borrow_mut() = Some("2030-01-01T00:00:00+00:00".into()));
        let first = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        TEST_TIME.with(|value| *value.borrow_mut() = Some("2000-01-01T00:00:00+00:00".into()));
        let second = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        assert!(first.created_at > second.created_at);
        let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        assert_eq!(plan.retained, vec![second.clone()]);
        assert_eq!(plan.candidates, vec![first]);
        let result = backup
            .run_daily(
                &storage,
                &op(),
                Utc::now().timestamp() as u64,
                &cancel,
                |_| {},
            )
            .unwrap();
        let DailyBackupResult::Unchanged { snapshot, .. } = result else {
            panic!("expected latest-publication baseline");
        };
        assert_eq!(snapshot, second);
        backup.apply_cleanup(&plan, &op(), &cancel, |_| {}).unwrap();
        assert_eq!(backup.verify(&second.snapshot_id).unwrap(), second);
    }
    #[test]
    fn legacy_order_is_preserved_until_a_new_complete_capture_replaces_it() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "legacy order").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        for _ in 0..2 {
            backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        }
        let directory = backup.publication_directory().unwrap();
        for entry in fs::read_dir(&directory).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::remove_dir(directory).unwrap();
        let legacy = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        assert!(legacy.candidates.is_empty());
        assert_eq!(legacy.preserved.len(), 1);
        let latest = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        assert_eq!(plan.retained, vec![latest]);
        assert_eq!(plan.candidates.len(), 2);
    }
    #[test]
    fn corrupted_sequence_blocks_new_publication_and_cleanup_without_hiding_old_backups() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "bad sequence").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        let first = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        let path = backup
            .publication_directory()
            .unwrap()
            .join("00000000000000000001.json");
        fs::write(&path, b"{}").unwrap();
        let next = op();
        assert_eq!(
            backup
                .create(&storage, &next, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::BackupIncomplete
        );
        assert!(!destination.path().join(next).exists());
        assert_eq!(
            backup.plan_retention(1, &cancel, |_| {}).unwrap_err().code,
            ErrorCode::BackupIncomplete
        );
        assert_eq!(backup.list().unwrap(), vec![first.clone()]);
        assert_eq!(backup.verify(&first.snapshot_id).unwrap(), first);
        assert_eq!(fs::read(path).unwrap(), b"{}");
    }
    #[test]
    fn resuming_an_older_ready_capture_does_not_promote_it_over_later_success() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "ready replay").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        let first = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        fs::rename(
            destination.path().join(&first.snapshot_id),
            destination
                .path()
                .join(format!(".staging-{}", first.snapshot_id)),
        )
        .unwrap();
        let second = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        assert_eq!(
            backup
                .create(&storage, &first.snapshot_id, &cancel, |_| {})
                .unwrap(),
            first
        );
        assert_eq!(backup.publication_history().unwrap().len(), 2);
        assert_eq!(
            backup.plan_retention(1, &cancel, |_| {}).unwrap().retained,
            vec![second]
        );
    }
    #[test]
    fn missing_latest_order_record_cannot_retire_the_unordered_latest_snapshot() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let storage = VaultStorage::create(source.path(), "missing order").unwrap();
        let backup =
            BackupStore::open(source.path(), &storage.vault().vault_id, destination.path())
                .unwrap();
        let cancel = AtomicBool::new(false);
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        let latest = backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
        fs::remove_file(
            backup
                .publication_directory()
                .unwrap()
                .join("00000000000000000002.json"),
        )
        .unwrap();
        let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
        assert!(!plan
            .candidates
            .iter()
            .any(|snapshot| snapshot.snapshot_id == latest.snapshot_id));
        assert_eq!(backup.verify(&latest.snapshot_id).unwrap(), latest);
    }
}
