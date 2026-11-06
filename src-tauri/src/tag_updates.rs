use super::*;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        LazyLock,
    },
};

type Key = (String, String);
static CANCELLATIONS: LazyLock<Mutex<HashMap<Key, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn request_cancel(vault_id: &str, operation_id: &str) -> Result<(), ServiceError> {
    if let Some(flag) = CANCELLATIONS
        .lock()
        .map_err(|_| poison_err())?
        .get(&(vault_id.into(), operation_id.into()))
    {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}

pub(super) async fn run(
    app: AppHandle,
    vault_id: String,
    operation_id: String,
    plan: Option<onote_domain::TagWritePlan>,
) -> Result<onote_domain::TagBatchResult, ServiceError> {
    let key = (vault_id.clone(), operation_id.clone());
    let flag = Arc::new(AtomicBool::new(false));
    {
        let mut active = CANCELLATIONS.lock().map_err(|_| poison_err())?;
        if active.contains_key(&key) {
            return Err(svc_err(ErrorCode::FileLocked, "error.writerBusy"));
        }
        active.insert(key.clone(), flag.clone());
    }
    // Cancellation must be signalled before waiting for the application state lock.
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_mut(state, &vault_id)?;
            let cancelled = || flag.load(Ordering::Relaxed);
            match plan {
                Some(plan) => storage.apply_tag_update_with_cancel(&plan, &operation_id, cancelled),
                None => storage.resume_tag_update_with_cancel(&operation_id, cancelled),
            }
        })
    })
    .await
    .map_err(map_join_err);
    CANCELLATIONS.lock().map_err(|_| poison_err())?.remove(&key);
    outcome?
}
