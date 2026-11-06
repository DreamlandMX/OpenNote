use super::*;
use sha2::{Digest, Sha256};

struct SourceState {
    note_id: String,
    path: String,
    hash: Option<String>,
}
fn source_hash(root: &Path, path: &str) -> Option<String> {
    VaultStorage::read_source_snapshot(root, path)
        .ok()
        .map(|source| format!("{:x}", Sha256::digest(source.as_bytes())))
}
fn changed_sources(root: &Path, sources: &mut [SourceState]) -> Vec<String> {
    sources
        .iter_mut()
        .filter_map(|source| {
            let hash = source_hash(root, &source.path);
            if hash == source.hash {
                return None;
            }
            source.hash = hash;
            Some(source.note_id.clone())
        })
        .collect()
}

fn source_states(
    storage: &VaultStorage,
    note_ids: &[String],
) -> Result<Vec<SourceState>, ServiceError> {
    let notes = storage.list()?;
    note_ids
        .iter()
        .map(|id| {
            let note = notes
                .iter()
                .find(|note| &note.note_id == id)
                .ok_or_else(|| svc_err(ErrorCode::ObjectNotFound, "error.noteNotFound"))?;
            Ok(SourceState {
                note_id: id.clone(),
                path: note.relative_path.clone(),
                hash: Some(note.content_hash.clone()),
            })
        })
        .collect()
}

fn validated_ids(watch_id: &str, note_ids: &[String]) -> Result<Vec<String>, ServiceError> {
    let invalid = || svc_err(ErrorCode::InvalidInput, "error.sourceWatchInvalid");
    let canonical =
        |id: &str| uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id);
    if !canonical(watch_id) || note_ids.is_empty() || note_ids.len() > 1000 {
        return Err(invalid());
    }
    let mut sorted = note_ids.to_vec();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != note_ids.len() || note_ids.iter().any(|id| !canonical(id)) {
        return Err(invalid());
    }
    Ok(sorted)
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceEvent {
    watch_id: String,
    vault_id: String,
    note_ids: Vec<String>,
    error: Option<ServiceError>,
}

fn unavailable() -> ServiceError {
    svc_err(ErrorCode::IoDenied, "error.sourceWatchUnavailable")
}

#[derive(Default)]
pub(super) struct SourceWatchHost(Mutex<BTreeMap<String, Subscription>>);
struct Subscription {
    vault_id: String,
    note_ids: Vec<String>,
    _watch: NativeWatch,
    is_catalog: bool,
}

pub(super) fn clear(app: &AppHandle) -> Result<(), ServiceError> {
    app.state::<SourceWatchHost>()
        .0
        .lock()
        .map_err(|_| poison_err())?
        .clear();
    Ok(())
}

#[tauri::command]
pub(super) async fn start_source_watch(
    app: AppHandle,
    vault_id: String,
    watch_id: String,
    note_ids: Vec<String>,
) -> Result<(), ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            let sorted = validated_ids(&watch_id, &note_ids)?;
            let host = app.state::<SourceWatchHost>();
            let mut subscriptions = host.0.lock().map_err(|_| poison_err())?;
            if let Some(existing) = subscriptions.get(&watch_id) {
                return if existing.vault_id == vault_id
                    && !existing.is_catalog
                    && existing.note_ids == sorted
                {
                    Ok(())
                } else {
                    Err(svc_err(
                        ErrorCode::IdempotencyMismatch,
                        "error.idempotencyMismatch",
                    ))
                };
            }
            if subscriptions.len() >= 8 {
                return Err(svc_err(ErrorCode::FileLocked, "error.writerBusy"));
            }
            let sources = source_states(storage, &sorted)?;
            let handle = app.clone();
            let event_vault = vault_id.clone();
            let event_watch = watch_id.clone();
            let watch = NativeWatch::start(storage.root_path(), sources, move |result| {
                let (note_ids, error) = match result {
                    Ok(ids) => (ids, None),
                    Err(error) => (Vec::new(), Some(error)),
                };
                let _ = handle.emit(
                    "opennote://source-changed",
                    SourceEvent {
                        watch_id: event_watch.clone(),
                        vault_id: event_vault.clone(),
                        note_ids,
                        error,
                    },
                );
            })?;
            subscriptions.insert(
                watch_id,
                Subscription {
                    vault_id,
                    note_ids: sorted,
                    _watch: watch,
                    is_catalog: false,
                },
            );
            Ok(())
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
pub(super) async fn start_catalog_watch(
    app: AppHandle,
    vault_id: String,
    watch_id: String,
) -> Result<(), ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            if !uuid::Uuid::parse_str(&watch_id).is_ok_and(|parsed| parsed.to_string() == watch_id)
            {
                return Err(svc_err(ErrorCode::InvalidInput, "error.sourceWatchInvalid"));
            }
            let host = app.state::<SourceWatchHost>();
            let mut subscriptions = host.0.lock().map_err(|_| poison_err())?;
            if let Some(existing) = subscriptions.get(&watch_id) {
                return if existing.vault_id == vault_id && existing.is_catalog {
                    Ok(())
                } else {
                    Err(svc_err(
                        ErrorCode::IdempotencyMismatch,
                        "error.idempotencyMismatch",
                    ))
                };
            }
            if subscriptions.len() >= 8 {
                return Err(svc_err(ErrorCode::FileLocked, "error.writerBusy"));
            }
            let handle = app.clone();
            let event_vault = vault_id.clone();
            let event_watch = watch_id.clone();
            let watch = NativeWatch::notifications(storage.root_path(), move |event| {
                let _ = handle.emit(
                    "opennote://catalog-changed",
                    SourceEvent {
                        watch_id: event_watch.clone(),
                        vault_id: event_vault.clone(),
                        note_ids: Vec::new(),
                        error: event.err(),
                    },
                );
            })?;
            subscriptions.insert(
                watch_id,
                Subscription {
                    vault_id,
                    note_ids: Vec::new(),
                    _watch: watch,
                    is_catalog: true,
                },
            );
            Ok(())
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
pub(super) async fn stop_source_watch(
    app: AppHandle,
    vault_id: String,
    watch_id: String,
) -> Result<(), ServiceError> {
    let host = app.state::<SourceWatchHost>();
    let mut subscriptions = host.0.lock().map_err(|_| poison_err())?;
    if subscriptions
        .get(&watch_id)
        .is_some_and(|entry| entry.vault_id != vault_id)
    {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.vaultSessionChanged",
        ));
    }
    subscriptions.remove(&watch_id);
    Ok(())
}

#[cfg(windows)]
struct NativeWatch {
    stop: Arc<std::sync::atomic::AtomicBool>,
    _thread: Option<std::thread::JoinHandle<()>>,
}
#[cfg(windows)]
impl Drop for NativeWatch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}
#[cfg(windows)]
impl NativeWatch {
    fn notifications(
        root: &Path,
        mut notify: impl FnMut(Result<(), ServiceError>) + Send + 'static,
    ) -> Result<Self, ServiceError> {
        use std::{
            os::windows::ffi::OsStrExt,
            sync::atomic::{AtomicBool, Ordering},
            time::{Duration, Instant},
        };
        use windows_sys::Win32::{
            Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
            Storage::FileSystem::{
                FindCloseChangeNotification, FindFirstChangeNotificationW,
                FindNextChangeNotification, FILE_NOTIFY_CHANGE_ATTRIBUTES,
                FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
                FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY,
                FILE_NOTIFY_CHANGE_SIZE,
            },
            System::Threading::WaitForSingleObject,
        };
        struct Notification(usize);
        impl Notification {
            fn raw(&self) -> HANDLE {
                self.0 as HANDLE
            }
        }
        impl Drop for Notification {
            fn drop(&mut self) {
                unsafe {
                    FindCloseChangeNotification(self.raw());
                }
            }
        }
        let path: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        // The owned notification is moved into exactly one worker and closed there.
        let raw = unsafe {
            FindFirstChangeNotificationW(
                path.as_ptr(),
                1,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_SECURITY
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(unavailable());
        }
        let notification = Notification(raw as usize);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let thread = std::thread::Builder::new()
            .name("opennote-source-watch".into())
            .spawn(move || {
                let mut due = Some(Instant::now());
                while !cancelled.load(Ordering::Acquire) {
                    // Only the wait times out; source reads run after an actual notification.
                    match unsafe { WaitForSingleObject(notification.raw(), 50) } {
                        WAIT_OBJECT_0 => {
                            if unsafe { FindNextChangeNotification(notification.raw()) } == 0 {
                                if !cancelled.load(Ordering::Acquire) {
                                    notify(Err(unavailable()));
                                }
                                break;
                            }
                            due.get_or_insert_with(|| Instant::now() + Duration::from_millis(300));
                        }
                        WAIT_TIMEOUT => (),
                        _ => {
                            if !cancelled.load(Ordering::Acquire) {
                                notify(Err(unavailable()));
                            }
                            break;
                        }
                    }
                    if due.is_some_and(|deadline| Instant::now() >= deadline) {
                        due = None;
                        if !cancelled.load(Ordering::Acquire) {
                            notify(Ok(()));
                        }
                    }
                }
            })
            .map_err(|_| unavailable())?;
        Ok(Self {
            stop,
            _thread: Some(thread),
        })
    }
}
#[cfg(not(windows))]
struct NativeWatch;
#[cfg(not(windows))]
impl NativeWatch {
    fn notifications(
        _: &Path,
        _: impl FnMut(Result<(), ServiceError>) + Send + 'static,
    ) -> Result<Self, ServiceError> {
        Err(unavailable())
    }
}

impl NativeWatch {
    fn start(
        root: &Path,
        mut sources: Vec<SourceState>,
        mut notify: impl FnMut(Result<Vec<String>, ServiceError>) + Send + 'static,
    ) -> Result<Self, ServiceError> {
        let observed_root = root.to_path_buf();
        Self::notifications(root, move |event| match event {
            Err(error) => notify(Err(error)),
            Ok(()) => {
                let changed = changed_sources(&observed_root, &mut sources);
                if !changed.is_empty() {
                    notify(Ok(changed));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn windows_notifications_refresh_only_changed_sources_and_stop_cleanly() {
        use std::{
            sync::{atomic::Ordering, mpsc},
            time::Duration,
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Source.md");
        std::fs::write(&source, "old source").unwrap();
        let sources = vec![SourceState {
            note_id: "source".into(),
            path: "Source.md".into(),
            hash: source_hash(dir.path(), "Source.md"),
        }];
        let (send, receive) = mpsc::channel();
        let mut watch = NativeWatch::start(dir.path(), sources, move |event| {
            let _ = send.send(event);
        })
        .unwrap();
        std::fs::create_dir(dir.path().join(".opennote")).unwrap();
        std::fs::write(dir.path().join(".opennote/state.sqlite"), "unrelated").unwrap();
        assert!(receive.recv_timeout(Duration::from_millis(450)).is_err());
        std::fs::write(&source, "intermediate source").unwrap();
        std::fs::write(&source, "new source").unwrap();
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(4))
                .unwrap()
                .unwrap(),
            ["source"]
        );
        std::fs::write(&source, "new source").unwrap();
        assert!(receive.recv_timeout(Duration::from_millis(450)).is_err());
        std::fs::remove_file(&source).unwrap();
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(4))
                .unwrap()
                .unwrap(),
            ["source"]
        );
        watch.stop.store(true, Ordering::Release);
        watch._thread.take().unwrap().join().unwrap();
        std::fs::write(&source, "after stop").unwrap();
        assert!(receive.recv_timeout(Duration::from_millis(150)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn directory_notifications_discover_new_notes_and_catalog_noops_do_not_loop() {
        use std::{
            sync::{atomic::Ordering, mpsc},
            time::Duration,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "catalog notifications").unwrap();
        let (send, receive) = mpsc::channel();
        let mut watch = NativeWatch::notifications(dir.path(), move |event| {
            let _ = send.send(event);
        })
        .unwrap();
        receive
            .recv_timeout(Duration::from_secs(4))
            .unwrap()
            .unwrap();
        let first = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        std::fs::write(dir.path().join("New.md"), "new external note").unwrap();
        receive
            .recv_timeout(Duration::from_secs(4))
            .unwrap()
            .unwrap();
        let added = storage
            .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
            .unwrap();
        assert_eq!(added.notes.len(), 1);
        assert_ne!(first.fingerprint, added.fingerprint);
        // At most one extra batch comes from publishing the new catalog row.
        if let Ok(event) = receive.recv_timeout(Duration::from_millis(600)) {
            event.unwrap();
            let same = storage
                .refresh_catalog(VaultStorage::scan_catalog(dir.path()).unwrap())
                .unwrap();
            assert_eq!(same.fingerprint, added.fingerprint);
        }
        assert!(receive.recv_timeout(Duration::from_millis(700)).is_err());
        watch.stop.store(true, Ordering::Release);
        watch._thread.take().unwrap().join().unwrap();
    }

    #[test]
    fn observer_requests_reject_noncanonical_duplicate_and_unbounded_id_lists() {
        let id = uuid::Uuid::new_v4().to_string();
        assert!(validated_ids(&id, &[id.clone()]).is_ok());
        assert!(validated_ids("invalid", &[id.clone()]).is_err());
        assert!(validated_ids(&id, &[]).is_err());
        assert!(validated_ids(&id, &[id.clone(), id.clone()]).is_err());
        assert!(validated_ids(&id, &["../outside.md".into()]).is_err());
        let too_many = (0..1001)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect::<Vec<_>>();
        assert!(validated_ids(&id, &too_many).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn failed_native_registration_reports_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(NativeWatch::start(&dir.path().join("missing"), Vec::new(), |_| ()).is_err());
    }

    #[test]
    fn source_selection_uses_only_current_vault_identities() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(first.path(), "first").unwrap();
        let mut other = VaultStorage::create(second.path(), "second").unwrap();
        let id = || uuid::Uuid::new_v4().to_string();
        let note = storage.create_note("Source.md", "source", &id()).unwrap();
        let foreign = other.create_note("Source.md", "private", &id()).unwrap();
        let selected = source_states(&storage, &[note.note_id.clone()]).unwrap();
        assert_eq!(selected[0].path, "Source.md");
        assert_eq!(selected[0].hash.as_ref(), Some(&note.content_hash));
        assert!(source_states(&storage, &[foreign.note_id]).is_err());
        assert!(source_states(&storage, &["../outside.md".into()]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_real_source_notification_reloads_current_embed_content_without_changing_the_reader() {
        use std::{
            sync::{atomic::Ordering, mpsc},
            time::Duration,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(dir.path(), "embed notifications").unwrap();
        let id = || uuid::Uuid::new_v4().to_string();
        let reader = storage
            .create_note("Reader.md", "![[Source]]\n", &id())
            .unwrap();
        let source = storage
            .create_note("Source.md", "old source\n", &id())
            .unwrap();
        let raw = onote_metadata_links::extract_links(&reader.source)
            .into_iter()
            .next()
            .unwrap();
        assert!(metadata::expand_embed(&storage, &reader.note_id, &raw, &[])
            .unwrap()
            .content
            .unwrap()
            .contains("old source"));
        let (send, receive) = mpsc::channel();
        let mut watch = NativeWatch::start(
            dir.path(),
            source_states(&storage, &[source.note_id.clone()]).unwrap(),
            move |event| {
                let _ = send.send(event);
            },
        )
        .unwrap();
        let new_source = source.source.replace("old source", "new source");
        std::fs::write(dir.path().join("Source.md"), &new_source).unwrap();
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(4))
                .unwrap()
                .unwrap(),
            [source.note_id.clone()]
        );
        let refreshed = metadata::expand_embed(&storage, &reader.note_id, &raw, &[]).unwrap();
        assert!(refreshed.content.unwrap().contains("new source"));
        assert_eq!(
            storage.read(&source.note_id).unwrap().local_revision,
            source.local_revision + 1
        );
        assert_eq!(storage.read(&reader.note_id).unwrap().source, reader.source);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Source.md")).unwrap(),
            new_source
        );
        watch.stop.store(true, Ordering::Release);
        watch._thread.take().unwrap().join().unwrap();
    }

    #[test]
    fn source_changes_compare_bytes_not_mtime_and_keep_deletions_and_reappearances() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Source.md");
        std::fs::write(&path, "first").unwrap();
        let original_time = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut sources = vec![SourceState {
            note_id: "source".into(),
            path: "Source.md".into(),
            hash: source_hash(dir.path(), "Source.md"),
        }];
        assert!(changed_sources(dir.path(), &mut sources).is_empty());
        std::fs::write(&path, "other").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original_time))
            .unwrap();
        assert_eq!(changed_sources(dir.path(), &mut sources), ["source"]);
        assert!(changed_sources(dir.path(), &mut sources).is_empty());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(changed_sources(dir.path(), &mut sources), ["source"]);
        assert!(changed_sources(dir.path(), &mut sources).is_empty());
        std::fs::write(&path, "returned").unwrap();
        assert_eq!(changed_sources(dir.path(), &mut sources), ["source"]);
    }
}
