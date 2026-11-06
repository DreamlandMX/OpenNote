use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use onote_domain::ErrorCode;

use crate::errors::{self, SeaRes};

/// One-writer-per-vault coordination across instances AND processes: a
/// process-local mutex keyed by the canonical vault root serializes
/// same-process operations, and an OS-level exclusive lock on
/// `<canonical root>/.opennote/writer.lock` (std `File::try_lock`) held for
/// the entire create/save/recovery protocol rejects writers from other
/// processes or alias paths of the same vault.
static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<Gate>>>> = OnceLock::new();

pub(crate) struct Gate {
    /// Process-local serialization on the canonical key.
    mutex: &'static Mutex<()>,
    /// Cross-process lock file; never truncated.
    lock_path: PathBuf,
}

/// Held for the duration of one write/recovery/initialize protocol; the OS
/// file lock is owned by the guard and released only on Drop.
pub(crate) struct GateGuard {
    _mutex: MutexGuard<'static, ()>,
    _file: File,
}

fn table() -> &'static Mutex<HashMap<PathBuf, Arc<Gate>>> {
    GATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Get (creating if needed) the per-vault gate for the canonical root so
/// alias paths of the same vault share one gate.
pub(crate) fn acquire(root: &Path) -> SeaRes<Arc<Gate>> {
    let key = canonical_root(root);
    let mut map = table()
        .lock()
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.vaultLockBroken", &[]))?;
    Ok(map
        .entry(key.clone())
        .or_insert_with(|| {
            Arc::new(Gate {
                // The static map outlives every guard; one leaked mutex per vault.
                mutex: Box::leak(Box::new(Mutex::new(()))),
                lock_path: key.join(".opennote").join("writer.lock"),
            })
        })
        .clone())
}

/// Lock the vault gate for the duration of one write/recovery operation.
/// When another process already holds the OS lock this fails fast with a
/// retryable FILE_LOCKED instead of waiting or spinning.
pub(crate) fn lock(gate: &Gate) -> SeaRes<GateGuard> {
    let _mutex = gate
        .mutex
        .lock()
        .map_err(|_| errors::err(ErrorCode::IoDenied, "error.vaultLockBroken", &[]))?;
    let file = open_lock_file(&gate.lock_path)?;
    match file.try_lock() {
        Ok(()) => Ok(GateGuard {
            _mutex,
            _file: file,
        }),
        Err(std::fs::TryLockError::WouldBlock) => Err(errors::file_locked()),
        Err(std::fs::TryLockError::Error(e)) => Err(errors::io(&e, "error.writerLockUnavailable")),
    }
}

fn open_lock_file(path: &Path) -> SeaRes<File> {
    // A reparse/symlink lock file must never be followed or trusted.
    if crate::scan::is_reparse(path) {
        return Err(errors::err(
            ErrorCode::IoDenied,
            "error.writerLockReparse",
            &[],
        ));
    }
    // create + read + write WITHOUT truncate: existing bytes stay intact.
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| errors::io(&e, "error.writerLockUnavailable"))
}
