use super::*;

pub(crate) fn readonly_connection(root: &Path) -> SeaRes<Connection> {
    let directory = root.join(".opennote");
    for name in ["state.sqlite", "state.sqlite-wal", "state.sqlite-shm"] {
        refuse_reparse(&directory.join(name), "error.stateReparse")?;
    }
    Connection::open_with_flags(
        directory.join("state.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(errors::database)
}

fn catalog(root: &Path) -> SeaRes<Vec<NoteRecord>> {
    let conn = readonly_connection(root)?;
    let records = load_notes(&conn)?;
    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    if records.iter().any(|record| {
        !Uuid::parse_str(&record.note_id).is_ok_and(|id| id.to_string() == record.note_id)
            || record.local_revision > 9_007_199_254_740_991
            || record.content_hash.len() != 64
            || !record
                .content_hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || !ids.insert(&record.note_id)
            || !paths.insert(&record.relative_path)
    }) {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.stateReadFailed",
            &[],
        ));
    }
    Ok(records)
}

/// Compatibility inspection never initializes or migrates authoritative state.
pub(crate) fn open_if_incompatible(root: &Path) -> SeaRes<Option<OpenedVault>> {
    inspect(root, false)
}

pub(crate) fn inspect(root: &Path, interrupted: bool) -> SeaRes<Option<OpenedVault>> {
    let directory = root.join(".opennote");
    refuse_reparse(&directory, "error.opennoteReparse")?;
    let meta_path = directory.join("vault.json");
    refuse_reparse(&meta_path, "error.metaReparse")?;
    let bytes = match fs::read(meta_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(errors::io(&e, "error.vaultMetaReadDenied")),
    };
    let meta: VaultMeta = serde_json::from_slice(&bytes)
        .map_err(|_| errors::err(ErrorCode::InvalidInput, "error.vaultMetaInvalid", &[]))?;
    if meta.schema_version == SCHEMA_VERSION && !interrupted {
        return Ok(None);
    }
    let scanned = scan::scan_vault(root)?;
    let mut findings = scanned.findings;
    let older = meta.schema_version <= SCHEMA_VERSION;
    findings.push(onote_domain::VaultFinding {
        code: if interrupted {
            "SchemaUpgradeInterrupted"
        } else if older {
            "SchemaUpgradeRequired"
        } else {
            "SchemaNewerThanSupported"
        }
        .into(),
        target: None,
        message_key: if interrupted {
            "finding.schemaUpgradeInterrupted"
        } else if older {
            "finding.schemaUpgradeRequired"
        } else {
            "finding.schemaNewerThanSupported"
        }
        .into(),
    });
    let prior = if older {
        match catalog(root) {
            Ok(records) => records,
            Err(_) => {
                findings.push(onote_domain::VaultFinding {
                    code: "LegacyCatalogUnavailable".into(),
                    target: None,
                    message_key: "finding.legacyCatalogUnavailable".into(),
                });
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let by_path: std::collections::HashMap<_, _> = prior
        .iter()
        .map(|record| (record.relative_path.as_str(), record))
        .collect();
    let notes: Vec<NoteRecord> = scanned
        .notes
        .into_iter()
        .map(|scanned| {
            let previous = by_path
                .get(scanned.relative_path.as_str())
                .copied()
                .filter(|record| {
                    scanned
                        .embedded_id
                        .as_ref()
                        .is_none_or(|id| id == &record.note_id)
                });
            let mut record = record_for(&[], scanned);
            if let Some(previous) = previous {
                record.note_id = previous.note_id.clone();
                record.local_revision = previous.local_revision;
            }
            record.read_only = true;
            record
        })
        .collect();
    let mut seen_ids = HashSet::new();
    for note in &notes {
        if !seen_ids.insert(&note.note_id)
            && !findings.iter().any(|finding| {
                finding.code == "DuplicateNoteId"
                    && finding.target.as_ref() == Some(&note.relative_path)
            })
        {
            findings.push(onote_domain::VaultFinding {
                code: "DuplicateNoteId".into(),
                target: Some(note.relative_path.clone()),
                message_key: "finding.duplicateNoteId".into(),
            });
        }
    }
    let fatal_findings = findings
        .iter()
        .filter(|finding| finding.code == "DuplicateNoteId")
        .count();
    Ok(Some(OpenedVault {
        root: root.into(),
        conn: None,
        vault_info: Vault {
            vault_id: meta.vault_id,
            schema_version: meta.schema_version,
            display_name: meta.display_name,
            sync_enabled: false,
            writable: false,
            findings,
        },
        notes,
        fatal_findings,
        failpoints: std::cell::RefCell::new(Vec::new()),
    }))
}
