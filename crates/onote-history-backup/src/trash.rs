use crate::{canonical_uuid, error, hash, safe_io, HistoryStore};
use onote_domain::{ErrorCode, TrashEntry, TrashRecord};
use onote_vault_storage::{validate_trash_record, TrashArchive};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    format_version: u32,
    record: TrashRecord,
    checksum: String,
}

impl HistoryStore {
    fn trash_path(&self, object_id: &str) -> Result<PathBuf, onote_domain::ServiceError> {
        if !canonical_uuid(object_id) {
            return Err(error(ErrorCode::InvalidInput, "error.invalidTrashId"));
        }
        Ok(self
            .root
            .join(".opennote/trash")
            .join(format!("{object_id}.json")))
    }
}

impl TrashArchive for HistoryStore {
    fn capture_deleted(&self, record: &TrashRecord) -> Result<(), onote_domain::ServiceError> {
        validate_trash_record(record, &self.vault_id)?;
        let path = self.trash_path(&record.entry.object_id)?;
        let bytes = serde_json::to_vec(record)
            .map_err(|_| error(ErrorCode::IoDenied, "error.trashSerialize"))?;
        let envelope = Envelope {
            format_version: 1,
            record: record.clone(),
            checksum: hash(&bytes),
        };
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|_| error(ErrorCode::IoDenied, "error.trashSerialize"))?;
        safe_io::publish(&self.root, &path, &bytes)
    }

    fn read_deleted(&self, object_id: &str) -> Result<TrashRecord, onote_domain::ServiceError> {
        let bytes = safe_io::read(&self.root, &self.trash_path(object_id)?)?;
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| error(ErrorCode::IoDenied, "error.trashCorrupt"))?;
        if envelope.format_version != 1 {
            return Err(error(ErrorCode::UnsupportedSchema, "error.trashSchema"));
        }
        validate_trash_record(&envelope.record, &self.vault_id)?;
        if envelope.record.entry.object_id != object_id
            || hash(
                &serde_json::to_vec(&envelope.record)
                    .map_err(|_| error(ErrorCode::IoDenied, "error.trashCorrupt"))?,
            ) != envelope.checksum
        {
            return Err(error(ErrorCode::IoDenied, "error.trashCorrupt"));
        }
        Ok(envelope.record)
    }

    fn purge_deleted(&self, entry: &TrashEntry) -> Result<(), onote_domain::ServiceError> {
        let path = self.trash_path(&entry.object_id)?;
        match safe_io::check(&self.root, &path) {
            Err(e) if e.code == ErrorCode::HistoryMissing => return Ok(()),
            Err(e) => return Err(e),
            Ok(()) => (),
        }
        let record = self.read_deleted(&entry.object_id)?;
        if record.entry != *entry {
            return Err(error(ErrorCode::IoDenied, "error.trashCorrupt"));
        }
        let before = safe_io::read(&self.root, &path)?;
        let quarantine = path
            .parent()
            .ok_or_else(|| error(ErrorCode::InvalidInput, "error.pathEscape"))?
            .join(format!(".pending-purge-{}", uuid::Uuid::new_v4()));
        fs::rename(&path, &quarantine).map_err(safe_io::io)?;
        if safe_io::read(&self.root, &quarantine)? != before {
            return Err(error(ErrorCode::IoDenied, "error.trashChanged"));
        }
        fs::remove_file(quarantine).map_err(safe_io::io)
    }
}
