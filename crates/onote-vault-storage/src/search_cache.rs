//! Rebuildable search sources; authoritative note/state writes never use this database.
use crate::{
    commit,
    errors::{err, SeaRes},
    identity, scan, state, VaultStorage,
};
use onote_domain::{ErrorCode, NoteSummary};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const APPLICATION_ID: i64 = 0x4f4e4958;
const VERSION: i64 = 1;

#[derive(Debug, Clone)]
pub struct SearchCacheSource {
    pub note_id: String,
    pub relative_path: String,
    pub content_hash: String,
    pub source: String,
}

fn unavailable() -> onote_domain::ServiceError {
    err(
        ErrorCode::RescanRequired,
        "error.searchIndexUnavailable",
        &[],
    )
}
fn check(
    continue_work: &mut impl FnMut(usize, usize) -> bool,
    processed: usize,
    total: usize,
) -> SeaRes<()> {
    if continue_work(processed, total) {
        Ok(())
    } else {
        Err(err(
            ErrorCode::OperationCancelled,
            "error.operationCancelled",
            &[],
        ))
    }
}
fn scope(root: &Path, vault: &str, writable: bool) -> SeaRes<(PathBuf, String)> {
    if !commit::is_canonical_uuid(vault) || scan::is_reparse(root) {
        return Err(unavailable());
    }
    let root = fs::canonicalize(root).map_err(|_| unavailable())?;
    let app = root.join(".opennote");
    if scan::is_reparse(&app) || !app.is_dir() {
        return Err(unavailable());
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&state::guarded_read(&root, ".opennote/vault.json")?)
            .map_err(|_| unavailable())?;
    if meta["vaultId"].as_str() != Some(vault)
        || (writable
            && (meta["writable"].as_bool() != Some(true)
                || meta["schemaVersion"].as_u64() != Some(state::SCHEMA_VERSION as u64)))
    {
        return Err(unavailable());
    }
    if writable && crate::upgrade::requires_readonly(&root)? {
        return Err(unavailable());
    }
    for name in [
        "index.sqlite",
        "index.sqlite-journal",
        "index.sqlite-wal",
        "index.sqlite-shm",
        "index.writer.lock",
    ] {
        let path = app.join(name);
        if scan::is_reparse(&path) || (path.exists() && !path.is_file()) {
            return Err(unavailable());
        }
    }
    Ok((app, root.to_string_lossy().into_owned()))
}
fn connect(path: &Path) -> rusqlite::Result<Connection> {
    // immutable prevents a read-only SQLite connection from creating WAL/shm sidecars.
    let uri = path
        .to_string_lossy()
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"._-".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect::<String>();
    let db = Connection::open_with_flags(
        format!("file:{uri}?immutable=1"),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    db.busy_timeout(Duration::from_millis(100))?;
    Ok(db)
}
fn recognized(db: &Connection) -> rusqlite::Result<bool> {
    Ok(
        db.query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))? == APPLICATION_ID
            && db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? == VERSION,
    )
}
fn digest(path: &Path) -> SeaRes<Vec<u8>> {
    use std::io::Read;
    if scan::is_reparse(path) {
        return Err(unavailable());
    }
    let mut file = fs::File::open(path).map_err(|_| unavailable())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let read = file.read(&mut buffer).map_err(|_| unavailable())?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().to_vec())
}
fn healthy(db: &Connection) -> rusqlite::Result<bool> {
    db.prepare("SELECT root, vault FROM scope WHERE singleton=1")?;
    db.prepare("SELECT note_id, path, hash, source FROM sources")?;
    Ok(db.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))? == "ok")
}
fn valid(source: &SearchCacheSource) -> bool {
    commit::is_canonical_uuid(&source.note_id)
        && commit::is_hash(&source.content_hash)
        && crate::naming::safe_relative(&source.relative_path).is_ok()
        && scan::hex_sha256(source.source.as_bytes()) == source.content_hash
        && !matches!(identity::parse_frontmatter(&source.source), identity::Frontmatter::WellFormed { id: identity::BlockId::Valid(ref id), .. } if id != &source.note_id)
}

struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        // Only files with this invocation's create_new UUID belong to us.
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(format!("{}-journal", self.0.display()));
    }
}

impl VaultStorage {
    pub fn read_search_cache(
        root: &Path,
        vault: &str,
        summaries: &[NoteSummary],
        mut continue_work: impl FnMut(usize, usize) -> bool,
    ) -> SeaRes<BTreeMap<String, SearchCacheSource>> {
        check(&mut continue_work, 0, summaries.len())?;
        let (app, root_key) = scope(root, vault, false)?;
        if [
            "index.sqlite-journal",
            "index.sqlite-wal",
            "index.sqlite-shm",
        ]
        .iter()
        .any(|name| app.join(name).exists())
        {
            return Err(unavailable());
        }
        let path = app.join("index.sqlite");
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let db = connect(&path).map_err(|_| unavailable())?;
        if !recognized(&db).map_err(|_| unavailable())? {
            return Err(unavailable());
        }
        let stored: (String, String) = db
            .query_row(
                "SELECT root, vault FROM scope WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| unavailable())?;
        if stored != (root_key, vault.to_owned()) {
            return Ok(BTreeMap::new());
        }
        let mut query = db
            .prepare("SELECT source FROM sources WHERE note_id = ?1 AND path = ?2 AND hash = ?3")
            .map_err(|_| unavailable())?;
        let mut result = BTreeMap::new();
        for (done, note) in summaries.iter().enumerate() {
            check(&mut continue_work, done, summaries.len())?;
            let source: Option<String> = query
                .query_row(
                    params![note.note_id, note.relative_path, note.content_hash],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| unavailable())?;
            if let Some(source) = source {
                let cached = SearchCacheSource {
                    note_id: note.note_id.clone(),
                    relative_path: note.relative_path.clone(),
                    content_hash: note.content_hash.clone(),
                    source,
                };
                if note.vault_id == vault && valid(&cached) {
                    result.insert(note.note_id.clone(), cached);
                }
            }
        }
        check(&mut continue_work, summaries.len(), summaries.len())?;
        Ok(result)
    }

    pub fn write_search_cache<'a>(
        root: &Path,
        vault: &str,
        sources: impl ExactSizeIterator<Item = &'a SearchCacheSource>,
        mut continue_work: impl FnMut(usize, usize) -> bool,
    ) -> SeaRes<()> {
        let total = sources.len();
        check(&mut continue_work, 0, total)?;
        let (app, root_key) = scope(root, vault, true)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(app.join("index.writer.lock"))
            .map_err(|_| unavailable())?;
        lock.try_lock().map_err(|_| unavailable())?;
        // A foreign SQLite writer's sidecars cannot be safely replaced with our snapshot.
        if [
            "index.sqlite-journal",
            "index.sqlite-wal",
            "index.sqlite-shm",
        ]
        .iter()
        .any(|name| app.join(name).exists())
        {
            return Err(unavailable());
        }
        let target = app.join("index.sqlite");
        let before = if target.exists() {
            let bytes = digest(&target)?;
            match connect(&target) {
                Ok(db) => match recognized(&db) {
                    Ok(true) => {}
                    Ok(false) => return Err(unavailable()),
                    Err(_) => {}
                },
                Err(_) => {}
            }
            Some(bytes)
        } else {
            None
        };
        let stage_path = app.join(format!(".index-stage-{}.sqlite", uuid::Uuid::new_v4()));
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage_path)
            .map_err(|_| unavailable())?;
        let stage = Stage(stage_path);
        {
            let mut db = Connection::open_with_flags(
                &stage.0,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(|_| unavailable())?;
            db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE scope(singleton INTEGER PRIMARY KEY CHECK(singleton=1), root TEXT NOT NULL, vault TEXT NOT NULL); CREATE TABLE sources(note_id TEXT PRIMARY KEY, path TEXT NOT NULL, hash TEXT NOT NULL, source TEXT NOT NULL);").map_err(|_| unavailable())?;
            db.pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(|_| unavailable())?;
            db.pragma_update(None, "user_version", VERSION)
                .map_err(|_| unavailable())?;
            let tx = db.transaction().map_err(|_| unavailable())?;
            tx.execute(
                "INSERT INTO scope VALUES(1,?1,?2)",
                params![root_key, vault],
            )
            .map_err(|_| unavailable())?;
            for (done, source) in sources.enumerate() {
                check(&mut continue_work, done, total)?;
                if !valid(source) {
                    return Err(unavailable());
                }
                tx.execute(
                    "INSERT INTO sources VALUES(?1,?2,?3,?4)",
                    params![
                        source.note_id,
                        source.relative_path,
                        source.content_hash,
                        source.source
                    ],
                )
                .map_err(|_| unavailable())?;
            }
            check(&mut continue_work, total, total)?;
            tx.commit().map_err(|_| unavailable())?;
        }
        check(&mut continue_work, total, total)?;
        scope(root, vault, true)?;
        if [
            "index.sqlite-journal",
            "index.sqlite-wal",
            "index.sqlite-shm",
        ]
        .iter()
        .any(|name| app.join(name).exists())
        {
            return Err(unavailable());
        }
        if target.exists() {
            let current = digest(&target)?;
            if before.as_ref() != Some(&current) {
                return Err(unavailable());
            }
            let corrupt = !connect(&target)
                .and_then(|db| healthy(&db))
                .unwrap_or(false);
            if corrupt {
                let mut backup = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(app.join(format!("index-rejected-{}.sqlite", uuid::Uuid::new_v4())))
                    .map_err(|_| unavailable())?;
                let mut original = fs::File::open(&target).map_err(|_| unavailable())?;
                std::io::copy(&mut original, &mut backup)
                    .and_then(|_| backup.sync_all())
                    .map_err(|_| unavailable())?;
                if digest(&target)? != current {
                    return Err(unavailable());
                }
            }
        } else if before.is_some() {
            return Err(unavailable());
        }
        check(&mut continue_work, total, total)?;
        fs::rename(&stage.0, target).map_err(|_| unavailable())?;
        Ok(())
    }
}
