use crate::{
    errors::{self, err, SeaRes},
    gate, ops, rename, scan,
    state::{self, NoteRecord},
    trash, VaultStorage,
};
use onote_domain::{CatalogChange, CatalogChangeKind, CatalogRefresh, ErrorCode, VaultFinding};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
struct WindowsPath(String);
impl Ord for WindowsPath {
    fn cmp(&self, other: &Self) -> Ordering {
        crate::compare_windows_paths(&self.0, &other.0)
    }
}
impl PartialOrd for WindowsPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for WindowsPath {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for WindowsPath {}
fn key(path: &str) -> WindowsPath {
    WindowsPath(path.to_owned())
}
fn changed() -> onote_domain::ServiceError {
    err(ErrorCode::RescanRequired, "error.catalogChanged", &[])
}
fn finding(code: &str, path: &str, message: &str) -> VaultFinding {
    VaultFinding {
        code: code.into(),
        target: Some(path.into()),
        message_key: message.into(),
    }
}

pub struct CatalogScan {
    root: PathBuf,
    metadata_hash: String,
    scanned: scan::ScanResult,
}
pub(crate) struct CatalogPlan {
    pub records: Vec<NoteRecord>,
    pub findings: Vec<VaultFinding>,
    pub unavailable: Vec<String>,
    pub changes: Vec<CatalogChange>,
    removed_paths: Vec<String>,
}
struct Candidate {
    record: NoteRecord,
    previous: Option<NoteRecord>,
}

pub(crate) fn reconcile(
    prior: &[NoteRecord],
    scanned: &[scan::ScannedNote],
    deleted: &HashSet<String>,
    mut findings: Vec<VaultFinding>,
) -> SeaRes<CatalogPlan> {
    if prior.iter().any(|note| {
        !crate::commit::is_canonical_uuid(&note.note_id)
            || !crate::commit::is_hash(&note.content_hash)
            || note.local_revision > 9_007_199_254_740_991
            || note.relative_path.starts_with('/')
            || note.relative_path.contains('\\')
            || note.relative_path.split('/').any(|part| {
                part.is_empty()
                    || matches!(part, "." | "..")
                    || part.contains(':')
                    || part.eq_ignore_ascii_case(".opennote")
            })
    }) {
        return Err(changed());
    }
    let by_id: HashMap<_, _> = prior.iter().map(|p| (p.note_id.as_str(), p)).collect();
    let by_path: BTreeMap<_, _> = prior.iter().map(|p| (key(&p.relative_path), p)).collect();
    if by_id.len() != prior.len() || by_path.len() != prior.len() {
        return Err(changed());
    }
    let mut ids: HashMap<&str, usize> = HashMap::new();
    let mut paths: BTreeMap<WindowsPath, usize> = BTreeMap::new();
    for note in scanned {
        if let Some(id) = &note.embedded_id {
            *ids.entry(id).or_default() += 1;
        }
        *paths.entry(key(&note.relative_path)).or_default() += 1;
    }
    let mut candidates = Vec::new();
    for note in scanned {
        let duplicate = note.embedded_id.as_deref().is_some_and(|id| ids[id] > 1);
        if duplicate || paths[&key(&note.relative_path)] > 1 {
            let (code, message) = if duplicate {
                ("DuplicateNoteId", "finding.duplicateNoteId")
            } else {
                ("CatalogPathCollision", "finding.catalogPathCollision")
            };
            findings.push(finding(code, &note.relative_path, message));
            continue;
        }
        if note
            .embedded_id
            .as_ref()
            .is_some_and(|id| deleted.contains(id))
        {
            findings.push(finding(
                "DeletedNoteReappeared",
                &note.relative_path,
                "finding.deletedNoteReappeared",
            ));
            continue;
        }
        let previous = match note.embedded_id.as_deref() {
            Some(id) => by_id.get(id).copied(),
            None => by_path.get(&key(&note.relative_path)).copied(),
        };
        let note_id = note
            .embedded_id
            .clone()
            .or_else(|| previous.map(|p| p.note_id.clone()))
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let revision = match previous {
            Some(p)
                if p.content_hash != note.content_hash || p.relative_path != note.relative_path =>
            {
                p.local_revision
                    .checked_add(1)
                    .filter(|rev| *rev <= 9_007_199_254_740_991)
                    .ok_or_else(changed)?
            }
            Some(p) => p.local_revision,
            None => 0,
        };
        candidates.push(Candidate {
            record: NoteRecord {
                note_id,
                relative_path: note.relative_path.clone(),
                content_hash: note.content_hash.clone(),
                local_revision: revision,
                read_only: note.read_only,
                id_embedded: note.embedded_id.is_some(),
            },
            previous: previous.cloned(),
        });
    }
    let candidate_ids: HashMap<_, _> = candidates
        .iter()
        .enumerate()
        .map(|(index, c)| (c.record.note_id.as_str(), index))
        .collect();
    if candidate_ids.len() != candidates.len() {
        return Err(changed());
    }
    let mut dependents: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut rejected = HashSet::new();
    let mut queue = VecDeque::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if let Some(occupant) = by_path.get(&key(&candidate.record.relative_path)) {
            if occupant.note_id != candidate.record.note_id {
                dependents.entry(&occupant.note_id).or_default().push(index);
                if candidate_ids
                    .get(occupant.note_id.as_str())
                    .is_none_or(|other| {
                        candidates[*other].record.relative_path == occupant.relative_path
                    })
                {
                    queue.push_back(index);
                }
            }
        }
    }
    while let Some(index) = queue.pop_front() {
        if !rejected.insert(index) {
            continue;
        }
        if let Some(waiting) = dependents.get(candidates[index].record.note_id.as_str()) {
            queue.extend(waiting);
        }
    }
    let accepted: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(index, _)| !rejected.contains(index))
        .map(|(_, c)| c)
        .collect();
    let accepted_ids: HashSet<_> = accepted.iter().map(|c| c.record.note_id.as_str()).collect();
    let mut records: BTreeMap<_, _> = prior
        .iter()
        .map(|p| (key(&p.relative_path), p.clone()))
        .collect();
    let mut unavailable = Vec::new();
    let mut missing_legacy_hashes = HashSet::new();
    for previous in prior {
        if accepted_ids.contains(previous.note_id.as_str()) {
            continue;
        }
        let present = paths.contains_key(&key(&previous.relative_path));
        let (code, message) = if ids
            .get(previous.note_id.as_str())
            .is_some_and(|count| *count > 1)
        {
            ("DuplicateNoteId", "finding.duplicateNoteId")
        } else if present {
            ("CatalogIdentityChanged", "finding.catalogIdentityChanged")
        } else {
            ("CatalogMissingNote", "finding.catalogMissingNote")
        };
        findings.push(finding(code, &previous.relative_path, message));
        unavailable.push(previous.note_id.clone());
        if !present && !previous.id_embedded {
            missing_legacy_hashes.insert(previous.content_hash.as_str());
        }
        if let Some(record) = records.get_mut(&key(&previous.relative_path)) {
            record.read_only = true;
        }
    }
    let mut removed_paths = Vec::new();
    for candidate in &accepted {
        if let Some(previous) = &candidate.previous {
            if previous.relative_path != candidate.record.relative_path {
                removed_paths.push(previous.relative_path.clone());
            }
            records.remove(&key(&previous.relative_path));
        }
    }
    for candidate in accepted {
        if !candidate.record.id_embedded
            && missing_legacy_hashes.contains(candidate.record.content_hash.as_str())
        {
            findings.push(finding(
                "CatalogLegacyMoveUnresolved",
                &candidate.record.relative_path,
                "finding.catalogLegacyMoveUnresolved",
            ));
        }
        records.insert(
            key(&candidate.record.relative_path),
            candidate.record.clone(),
        );
    }
    let records: Vec<_> = records.into_values().collect();
    let mut changes = Vec::new();
    for note in &records {
        let previous = by_id.get(note.note_id.as_str()).copied();
        if previous.is_some_and(|p| p == note) {
            continue;
        }
        let kind = match previous {
            None => CatalogChangeKind::Added,
            Some(_) if unavailable.contains(&note.note_id) => CatalogChangeKind::Unavailable,
            Some(p) if p.relative_path != note.relative_path => CatalogChangeKind::Moved,
            Some(p) if p.content_hash != note.content_hash => CatalogChangeKind::Updated,
            Some(_) => CatalogChangeKind::ReadOnlyChanged,
        };
        changes.push(CatalogChange {
            note_id: note.note_id.clone(),
            kind,
            previous_path: previous.map(|p| p.relative_path.clone()),
            relative_path: if unavailable.contains(&note.note_id) {
                None
            } else {
                Some(note.relative_path.clone())
            },
        });
    }
    findings.sort_by(|a, b| (&a.code, &a.target).cmp(&(&b.code, &b.target)));
    findings.dedup();
    unavailable.sort();
    Ok(CatalogPlan {
        records,
        findings,
        unavailable,
        changes,
        removed_paths,
    })
}

pub(crate) fn persist(conn: &rusqlite::Connection, plan: &CatalogPlan) -> SeaRes<()> {
    if plan.changes.is_empty() && plan.removed_paths.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction().map_err(errors::database)?;
    for path in &plan.removed_paths {
        tx.execute("DELETE FROM notes WHERE relative_path=?1", [path])
            .map_err(errors::database)?;
    }
    state::persist_notes(&tx, &plan.records)?;
    tx.commit().map_err(errors::database)
}
fn outcome(
    records: &[NoteRecord],
    vault_id: &str,
    changes: Vec<CatalogChange>,
    findings: Vec<VaultFinding>,
    unavailable: Vec<String>,
    deferred: bool,
    persisted: bool,
    writable: bool,
) -> SeaRes<CatalogRefresh> {
    let notes: Vec<_> = records
        .iter()
        .map(|record| state::summary(record, vault_id))
        .collect();
    let fingerprint = scan::hex_sha256(
        &serde_json::to_vec(&(
            &notes,
            &findings,
            &unavailable,
            deferred,
            persisted,
            writable,
        ))
        .map_err(|_| changed())?,
    );
    Ok(CatalogRefresh {
        notes,
        changes,
        unavailable_note_ids: unavailable,
        findings,
        fingerprint,
        deferred,
        persisted,
        writable,
    })
}
impl VaultStorage {
    pub fn scan_catalog(root: &Path) -> SeaRes<CatalogScan> {
        let root =
            fs::canonicalize(root).map_err(|e| errors::io(&e, "error.vaultRootUnavailable"))?;
        let before = state::guarded_read(&root, ".opennote/vault.json")?;
        let scanned = scan::scan_vault(&root)?;
        if state::guarded_read(&root, ".opennote/vault.json")? != before {
            return Err(changed());
        }
        Ok(CatalogScan {
            root,
            metadata_hash: scan::hex_sha256(&before),
            scanned,
        })
    }
    pub fn refresh_catalog(&mut self, snapshot: CatalogScan) -> SeaRes<CatalogRefresh> {
        if fs::canonicalize(&self.root).map_err(|e| errors::io(&e, "error.vaultRootUnavailable"))?
            != snapshot.root
        {
            return Err(changed());
        }
        let bytes = state::guarded_read(&self.root, ".opennote/vault.json")?;
        let metadata: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| changed())?;
        if scan::hex_sha256(&bytes) != snapshot.metadata_hash
            || metadata["vaultId"].as_str() != Some(self.vault_info.vault_id.as_str())
            || metadata["schemaVersion"].as_u64() != Some(self.vault_info.schema_version as u64)
        {
            return Err(changed());
        }
        let persisted = self.vault_info.writable
            && self.vault_info.schema_version == state::SCHEMA_VERSION
            && metadata["writable"].as_bool() == Some(true);
        if metadata["writable"].as_bool() != Some(true) {
            self.vault_info.writable = false;
        }
        let write_gate = if persisted {
            Some(gate::acquire(&self.root)?)
        } else {
            None
        };
        let _guard = write_gate
            .as_ref()
            .map(|gate| gate::lock(gate))
            .transpose()?;
        if scan::hex_sha256(&state::guarded_read(&self.root, ".opennote/vault.json")?)
            != snapshot.metadata_hash
        {
            return Err(changed());
        }
        let current_conn = self
            .conn
            .as_ref()
            .filter(|_| self.vault_info.schema_version == state::SCHEMA_VERSION);
        let prior = match current_conn {
            Some(conn) => state::load_notes(conn)?,
            None => self.notes.borrow().clone(),
        };
        let mut deleted = HashSet::new();
        if let Some(conn) = current_conn {
            let (paths, ids) = trash::protected(conn)?;
            if !ops::list_prepared(conn)?.is_empty()
                || !rename::pending(conn)?.is_empty()
                || !paths.is_empty()
                || !ids.is_empty()
            {
                return outcome(
                    &prior,
                    &self.vault_info.vault_id,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    true,
                    false,
                    persisted,
                );
            }
            deleted.extend(
                trash::tombstones(conn)?
                    .into_iter()
                    .map(|entry| entry.note_id),
            );
        }
        let mut plan = reconcile(
            &prior,
            &snapshot.scanned.notes,
            &deleted,
            snapshot.scanned.findings,
        )?;
        let previous_by_id: HashMap<_, _> = prior
            .iter()
            .map(|note| (note.note_id.as_str(), note))
            .collect();
        let observed_paths: BTreeSet<_> = snapshot
            .scanned
            .notes
            .iter()
            .map(|note| key(&note.relative_path))
            .collect();
        for record in &plan.records {
            let previous = previous_by_id.get(record.note_id.as_str()).copied();
            if previous.is_none_or(|p| {
                p.content_hash != record.content_hash || p.relative_path != record.relative_path
            }) {
                let fresh = state::guarded_read(&self.root, &record.relative_path)?;
                if scan::hex_sha256(&fresh) != record.content_hash {
                    return Err(changed());
                }
            }
        }
        for id in &plan.unavailable {
            if let Some(previous) = previous_by_id.get(id.as_str()) {
                if !observed_paths.contains(&key(&previous.relative_path))
                    && state::guarded_read(&self.root, &previous.relative_path).is_ok()
                {
                    return Err(changed());
                }
            }
        }
        if persisted && (!plan.changes.is_empty() || !plan.removed_paths.is_empty()) {
            persist(self.conn.as_ref().ok_or_else(changed)?, &plan)?;
        }
        if !persisted {
            for record in &mut plan.records {
                record.read_only = true;
                if let Some(previous) = previous_by_id.get(record.note_id.as_str()) {
                    record.local_revision = previous.local_revision;
                }
            }
            self.vault_info.writable = false;
        }
        *self.notes.borrow_mut() = plan.records.clone();
        outcome(
            &plan.records,
            &self.vault_info.vault_id,
            plan.changes,
            plan.findings,
            plan.unavailable,
            false,
            persisted,
            persisted,
        )
    }
}
