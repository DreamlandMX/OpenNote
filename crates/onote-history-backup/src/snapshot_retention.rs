use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotPreservationReason {
    Unverified,
    SourceUnknown,
    ProtectedDependency,
    CleanupPending,
    OrderUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreservedSnapshot {
    pub snapshot_id: String,
    pub reason: SnapshotPreservationReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotRetentionPlan {
    pub plan_id: String,
    pub retained_snapshots: u32,
    pub retained: Vec<Snapshot>,
    pub candidates: Vec<Snapshot>,
    pub preserved: Vec<PreservedSnapshot>,
    // Logical payload bytes; filesystem allocation and hard links can change actual reclaimed space.
    pub candidate_bytes: u64,
}
pub(super) fn snapshot_order(
    a: &Snapshot,
    b: &Snapshot,
    order: &BTreeMap<String, usize>,
) -> std::cmp::Ordering {
    let a_time = chrono::DateTime::parse_from_rfc3339(&a.created_at).ok();
    let b_time = chrono::DateTime::parse_from_rfc3339(&b.created_at).ok();
    order
        .get(&b.snapshot_id)
        .cmp(&order.get(&a.snapshot_id))
        .then_with(|| b_time.cmp(&a_time))
        .then_with(|| a.snapshot_id.cmp(&b.snapshot_id))
}

impl BackupStore {
    pub fn plan_retention(
        &self,
        retained_snapshots: u32,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotProgress),
    ) -> Result<SnapshotRetentionPlan, ServiceError> {
        if retained_snapshots == 0 {
            return Err(error(
                ErrorCode::InvalidInput,
                "error.backupPreferencesInvalid",
            ));
        }
        cancelled(cancel)?;
        safe_io::check(&self.destination, &self.destination)?;
        let mut protected = self.capture_dependencies(cancel)?;
        protected.extend(self.daily_dependencies()?);
        protected.extend(self.restore_dependencies(cancel)?);
        protected.extend(self.upgrade_dependencies(cancel)?);
        protected.extend(self.overwrite_dependencies(cancel)?);
        let records = self.publication_records()?;
        let publications: Vec<_> = records
            .iter()
            .map(|record| record.snapshot.clone())
            .collect();
        let order: BTreeMap<_, _> = publications
            .iter()
            .enumerate()
            .map(|(index, snapshot)| (snapshot.snapshot_id.clone(), index))
            .collect();
        let mut verified = Vec::new();
        let mut preserved = Vec::new();
        let mut completed = 0;
        for entry in fs::read_dir(&self.destination).map_err(safe_io::io)? {
            cancelled(cancel)?;
            let name = entry.map_err(safe_io::io)?.file_name();
            let Some(id) = name.to_str().filter(|id| canonical_uuid(id)) else {
                continue;
            };
            if self.not_removed(id).is_err() {
                preserved.push(PreservedSnapshot {
                    snapshot_id: id.into(),
                    reason: SnapshotPreservationReason::CleanupPending,
                });
                continue;
            }
            match self.verify_directory(&self.directory(id)?, id, cancel) {
                Ok(snapshot) => match self.snapshot_from_source(id) {
                    Ok(true) => {
                        if publications.iter().any(|record| {
                            record.snapshot_id == snapshot.snapshot_id && record != &snapshot
                        }) || self.cleanup_files(&snapshot, cancel).is_err()
                        {
                            cancelled(cancel)?;
                            preserved.push(PreservedSnapshot {
                                snapshot_id: id.into(),
                                reason: SnapshotPreservationReason::Unverified,
                            });
                        } else {
                            verified.push(snapshot);
                        }
                    }
                    Ok(false) => preserved.push(PreservedSnapshot {
                        snapshot_id: id.into(),
                        reason: SnapshotPreservationReason::SourceUnknown,
                    }),
                    Err(_) => preserved.push(PreservedSnapshot {
                        snapshot_id: id.into(),
                        reason: SnapshotPreservationReason::Unverified,
                    }),
                },
                Err(failure) if failure.code == ErrorCode::OperationCancelled => {
                    return Err(failure)
                }
                Err(_) => preserved.push(PreservedSnapshot {
                    snapshot_id: id.into(),
                    reason: SnapshotPreservationReason::Unverified,
                }),
            }
            completed += 1;
            progress(SnapshotProgress {
                phase: "listing".into(),
                completed_objects: completed,
                total_objects: 0,
            });
        }
        cancelled(cancel)?;
        // Unknown order is protected unless a later verified capture explicitly observed this predecessor.
        let predecessors: HashSet<_> = records
            .iter()
            .filter(|record| verified.contains(&record.snapshot))
            .flat_map(|record| record.supersedes.iter().cloned())
            .collect();
        let mut unordered = HashSet::new();
        for snapshot in &verified {
            if !order.contains_key(&snapshot.snapshot_id)
                && !predecessors
                    .contains(&(snapshot.snapshot_id.clone(), snapshot.manifest_hash.clone()))
            {
                protected.insert(snapshot.snapshot_id.clone());
                unordered.insert(snapshot.snapshot_id.clone());
            }
        }
        verified.sort_by(|a, b| snapshot_order(a, b, &order));
        preserved.sort_by(|a, b| a.snapshot_id.cmp(&b.snapshot_id));
        let expired = verified.split_off((retained_snapshots as usize).min(verified.len()));
        let mut candidates = Vec::new();
        for snapshot in expired {
            if protected.contains(&snapshot.snapshot_id) {
                preserved.push(PreservedSnapshot {
                    reason: if unordered.contains(&snapshot.snapshot_id) {
                        SnapshotPreservationReason::OrderUnknown
                    } else {
                        SnapshotPreservationReason::ProtectedDependency
                    },
                    snapshot_id: snapshot.snapshot_id,
                });
            } else {
                candidates.push(snapshot);
            }
        }
        preserved.sort_by(|a, b| a.snapshot_id.cmp(&b.snapshot_id));
        let candidate_bytes = candidates
            .iter()
            .try_fold(0u64, |sum, snapshot| sum.checked_add(snapshot.total_bytes))
            .ok_or_else(incomplete)?;
        let mut plan = SnapshotRetentionPlan {
            plan_id: String::new(),
            retained_snapshots,
            retained: verified,
            candidates,
            preserved,
            candidate_bytes,
        };
        plan.plan_id = hash(
            &serde_json::to_vec(&(&self.source, &self.destination, &self.vault_id, &plan))
                .map_err(|_| incomplete())?,
        );
        Ok(plan)
    }
}
