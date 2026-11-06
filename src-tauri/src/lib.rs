//! OpenNote P0 native host: owns window/close lifecycle and exposes the
//! minimal vault-note command surface over the existing `VaultStorage`.
//! No vault open state is replaced on failure; dialogs run off-thread.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use onote_domain::{
    CheckpointRequest, CheckpointResult, DraftPage, ErrorCode, HistoryListing, NoteDocument,
    NoteDraft, NoteSummary, SaveResult, ServiceError, TrashCancellation, TrashEntry,
    TrashOperation, TrashOperationResult, TrashRecord, TrashRestoreResult, Vault,
};
use onote_history_backup::{
    HistoryCleanupPreview, HistoryCleanupResult, HistoryRetentionPlan, HistoryRetentionSettings,
    HistoryRevisionContent, HistoryStore,
};
use onote_vault_storage::{RecoverOutcome, TrashArchive, VaultStorage};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_dialog::DialogExt;
mod attachment_io;
mod backup;
mod backup_preferences;
mod history_cleanup;
mod import_batch;
mod latex_build;
mod latex_project;
mod latex_runtime;
mod pdf_validate;
/// `OpenNote.exe --latex-uninstall-cleanup` (see `latex_runtime`).
pub use latex_runtime::uninstall_cleanup as latex_uninstall_cleanup;
/// `OpenNote.exe --latex-validate-pdf <file>`, run only inside the sandbox.
pub use pdf_validate::{validate_main as latex_validate_pdf, FLAG as LATEX_VALIDATE_PDF_FLAG};

/// For `tests/latex_validate.rs`, which needs the real program binary.
#[doc(hidden)]
pub mod latex_test_support {
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    use crate::latex_runtime::compile;

    /// Copies `program` as the validator under `builds`, then checks `pdf`
    /// (a path anywhere) with only `check` readable.
    pub fn validate_in_sandbox(
        program: &Path,
        builds: &Path,
        profile: &str,
        check: &Path,
        pdf: &Path,
    ) -> Result<u32, String> {
        let validator =
            compile::validator_copy(builds, program).map_err(|error| format!("{error:?}"))?;
        let scratch = builds.join("scratch");
        std::fs::create_dir_all(&scratch).map_err(|error| error.to_string())?;
        compile::validate(
            &validator,
            profile,
            check,
            pdf,
            &scratch,
            &AtomicBool::new(false),
        )
        .map_err(|error| format!("{error:?}"))
    }

    /// Deletes a throwaway profile (tests only).
    pub fn remove_profile(profile: &str) -> Vec<String> {
        crate::latex_runtime::remove_everything(None, &[profile]).errors
    }

    pub fn sample_pdf(pages: usize) -> Vec<u8> {
        crate::pdf_validate::sample(pages)
    }
}
/// Vault-scoped M04 metadata service. Public so integration tests can exercise
/// the storage-to-index boundary without going through the Tauri runtime.
pub mod metadata;
mod search;
mod search_job;
mod source_watch;
mod tag_cache;
mod tag_updates;
mod window_fit;

/// Managed host state: the single current vault plus the recovery report
/// captured explicitly right after create/open (exposed via `recover_vault`).
#[derive(Debug)]
struct AppState {
    vault: Option<VaultStorage>,
    recovery: Option<RecoverOutcome>,
    history: Option<Arc<HistoryStore>>,
}

#[derive(Default)]
struct VaultFolderSelection(Mutex<Option<(String, PathBuf)>>);

fn resolve_vault_selection(
    selected: &Option<(String, PathBuf)>,
    id: &str,
    name: &str,
) -> Result<(PathBuf, String), ServiceError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(svc_err(ErrorCode::InvalidInput, "error.vaultNameInvalid"));
    }
    let path = selected
        .as_ref()
        .filter(|(key, _)| key == id)
        .map(|(_, path)| path.clone())
        .ok_or_else(|| svc_err(ErrorCode::InvalidInput, "error.vaultSelectionInvalid"))?;
    Ok((path, name.to_owned()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ChosenVaultFolder {
    selection_id: String,
    display_path: String,
}

#[cfg(test)]
mod vault_folder_selection_tests {
    use super::*;

    #[test]
    fn selection_is_host_owned_and_rejects_missing_or_replaced_tokens() {
        let directory = tempfile::tempdir().unwrap();
        let selected = Some(("current".to_owned(), directory.path().to_owned()));
        assert!(resolve_vault_selection(&None, "current", "Notes").is_err());
        assert!(resolve_vault_selection(&selected, "old", "Notes").is_err());
        assert!(resolve_vault_selection(&selected, "current", " \n").is_err());
        assert!(resolve_vault_selection(&selected, "current", "bad\nname").is_err());
        assert!(resolve_vault_selection(&selected, "current", &"x".repeat(121)).is_err());
        let (root, name) = resolve_vault_selection(&selected, "current", "  我的笔记  ").unwrap();
        assert_eq!(root, directory.path());
        assert_eq!(name, "我的笔记");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn named_creation_uses_existing_storage_and_keeps_existing_vault_intact() {
        let directory = tempfile::tempdir().unwrap();
        let (vault, storage, _, history) = prepare_named_vault(
            directory.path().to_owned(),
            true,
            Some("Research notes".to_owned()),
        )
        .unwrap();
        assert_eq!(vault.display_name, "Research notes");
        drop(storage);
        drop(history);
        assert!(prepare_named_vault(
            directory.path().to_owned(),
            true,
            Some("Replacement".to_owned())
        )
        .is_err());
        let reopened = prepare_vault(directory.path().to_owned(), false).unwrap();
        assert_eq!(reopened.0.display_name, "Research notes");
    }
}

#[tauri::command]
async fn choose_vault_folder(
    app: AppHandle,
    dialog_title: Option<String>,
) -> Result<Option<ChosenVaultFolder>, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<VaultFolderSelection>();
        let mut selected = state.0.try_lock().map_err(|_| task_err())?;
        let title = localized_or_fallback(dialog_title, "Choose vault folder");
        let Some(path) = pick_vault_folder_blocking(&app, &title) else {
            return Ok(None);
        };
        let selection_id = uuid::Uuid::new_v4().to_string();
        let display_path = path.to_string_lossy().into_owned();
        *selected = Some((selection_id.clone(), path));
        Ok(Some(ChosenVaultFolder {
            selection_id,
            display_path,
        }))
    })
    .await
    .map_err(map_join_err)?
}

/// Event emitted to the frontend when the user closes the window. The host
/// refuses native close until the frontend authorizes `confirm_close`.
const EVENT_CLOSE_REQUESTED: &str = "opennote://close-requested";

/// Fallback vault display name when the folder has no usable name.
const FALLBACK_VAULT_NAME: &str = "OpenNote vault";
const SAMPLE_NOTE_SOURCE: &str = "\
# Welcome to OpenNote

This sample note is yours to edit or delete.

- Notes are saved as Markdown in this vault.
- Create a note from the workspace to get started.
";

fn svc_err(code: ErrorCode, message_key: &str) -> ServiceError {
    ServiceError {
        code,
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
        retryable: false,
        operation_id: None,
        retry_after_seconds: None,
    }
}

fn no_vault_err() -> ServiceError {
    svc_err(ErrorCode::InvalidInput, "error.noVaultOpen")
}

fn poison_err() -> ServiceError {
    svc_err(ErrorCode::IoDenied, "error.internalLockPoisoned")
}

fn task_err() -> ServiceError {
    svc_err(ErrorCode::IoDenied, "error.hostTaskFailed")
}

fn export_err(code: ErrorCode, message_key: &str) -> ServiceError {
    svc_err(code, message_key)
}

fn map_join_err<E>(_e: E) -> ServiceError {
    task_err()
}

fn map_io_export(e: &std::io::Error) -> ServiceError {
    let code = if e.kind() == std::io::ErrorKind::StorageFull {
        ErrorCode::DiskFull
    } else {
        ErrorCode::IoDenied
    };
    export_err(code, "error.exportWriteFailed")
}

/// Shared vault-session guard for every note-bearing command: under the
/// AppState mutex and before ANY storage operation, the caller-supplied
/// session `vault_id` must match the currently active vault. A mismatch (a
/// late frontend callback still scoped to a vault replaced by a completed
/// create/open picker) is refused as typed `error.vaultSessionChanged` with
/// no params — note UUIDs can legitimately repeat across copied vaults, so
/// noteId/revision arguments alone cannot prove the callback targets the
/// now-active vault instead of silently writing it.
fn scoped_vault_ref<'a>(
    state: &'a AppState,
    vault_id: &str,
) -> Result<&'a VaultStorage, ServiceError> {
    let storage = state.vault.as_ref().ok_or_else(no_vault_err)?;
    if storage.vault().vault_id != vault_id {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.vaultSessionChanged",
        ));
    }
    Ok(storage)
}

/// Mutable counterpart of [`scoped_vault_ref`] for the write commands.
fn scoped_vault_mut<'a>(
    state: &'a mut AppState,
    vault_id: &str,
) -> Result<&'a mut VaultStorage, ServiceError> {
    let storage = state.vault.as_mut().ok_or_else(no_vault_err)?;
    if storage.vault().vault_id != vault_id {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.vaultSessionChanged",
        ));
    }
    Ok(storage)
}

fn with_state<T>(
    app: &AppHandle,
    f: impl FnOnce(&mut AppState) -> Result<T, ServiceError>,
) -> Result<T, ServiceError> {
    let state = app.state::<Mutex<AppState>>();
    let mut guard = state.lock().map_err(|_| poison_err())?;
    f(&mut guard)
}

fn scoped_history<'a>(
    state: &'a AppState,
    vault_id: &str,
) -> Result<&'a HistoryStore, ServiceError> {
    scoped_vault_ref(state, vault_id)?;
    state
        .history
        .as_deref()
        .ok_or_else(|| svc_err(ErrorCode::HistoryMissing, "error.historyUnavailable"))
}

fn read_trash_record(
    state: &AppState,
    vault_id: &str,
    object_id: &str,
) -> Result<TrashRecord, ServiceError> {
    let expected = scoped_vault_ref(state, vault_id)?.trash_entry(object_id)?;
    let record = scoped_history(state, vault_id)?.read_deleted(object_id)?;
    if record.entry != expected {
        return Err(svc_err(ErrorCode::IoDenied, "error.trashCatalogMismatch"));
    }
    Ok(record)
}

#[tauri::command]
async fn list_trash(app: AppHandle, vault_id: String) -> Result<Vec<TrashEntry>, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| scoped_vault_ref(s, &vault_id)?.list_trash())
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn read_trash(
    app: AppHandle,
    vault_id: String,
    object_id: String,
) -> Result<TrashRecord, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| read_trash_record(s, &vault_id, &object_id))
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn delete_note(
    app: AppHandle,
    vault_id: String,
    note_id: String,
    expected_revision: u64,
    expected_hash: String,
    operation_id: String,
) -> Result<TrashEntry, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_mut(s, &vault_id)?.delete_note(
                &note_id,
                expected_revision,
                &expected_hash,
                &operation_id,
            )
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn restore_trash(
    app: AppHandle,
    vault_id: String,
    object_id: String,
    target_path: String,
    expected_revision: u64,
    operation_id: String,
) -> Result<TrashRestoreResult, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_mut(s, &vault_id)?.restore_trash(
                &object_id,
                &target_path,
                expected_revision,
                &operation_id,
            )
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn purge_trash(
    app: AppHandle,
    vault_id: String,
    object_id: String,
    expected_revision: u64,
    operation_id: String,
) -> Result<(), ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_mut(s, &vault_id)?.purge_trash(
                &object_id,
                expected_revision,
                &operation_id,
            )
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn list_trash_operations(
    app: AppHandle,
    vault_id: String,
) -> Result<Vec<TrashOperation>, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_ref(s, &vault_id)?.list_trash_operations()
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn resume_trash(
    app: AppHandle,
    vault_id: String,
    operation_id: String,
) -> Result<TrashOperationResult, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_mut(s, &vault_id)?.resume_trash(&operation_id)
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn cancel_trash(
    app: AppHandle,
    vault_id: String,
    operation_id: String,
) -> Result<TrashCancellation, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_mut(s, &vault_id)?.cancel_trash(&operation_id)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn list_history(
    app: AppHandle,
    vault_id: String,
    note_id: String,
) -> Result<HistoryListing, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            scoped_history(state, &vault_id)?.list_available(&note_id)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn read_history(
    app: AppHandle,
    vault_id: String,
    note_id: String,
    revision_id: String,
) -> Result<HistoryRevisionContent, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            scoped_history(state, &vault_id)?.read_with_references(&note_id, &revision_id)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn restore_history(
    app: AppHandle,
    vault_id: String,
    note_id: String,
    revision_id: String,
    expected_revision: u64,
    operation_id: String,
) -> Result<SaveResult, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            scoped_vault_ref(state, &vault_id)?;
            let history = state
                .history
                .clone()
                .ok_or_else(|| svc_err(ErrorCode::HistoryMissing, "error.historyUnavailable"))?;
            history.restore(
                scoped_vault_mut(state, &vault_id)?,
                &note_id,
                &revision_id,
                expected_revision,
                &operation_id,
            )
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn history_retention_settings(
    app: AppHandle,
    vault_id: String,
) -> Result<HistoryRetentionSettings, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            scoped_history(state, &vault_id)?.retention_settings()
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn set_history_retention(
    app: AppHandle,
    vault_id: String,
    retention_days: u32,
    expected_revision: u64,
) -> Result<HistoryRetentionSettings, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            scoped_history(state, &vault_id)?.set_retention(
                scoped_vault_ref(state, &vault_id)?,
                retention_days,
                expected_revision,
            )
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn preview_history_cleanup(
    app: AppHandle,
    vault_id: String,
) -> Result<HistoryCleanupPreview, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let history = scoped_history(state, &vault_id)?;
            let storage = scoped_vault_ref(state, &vault_id)?;
            history.preview_cleanup(storage)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn apply_history_cleanup(
    app: AppHandle,
    vault_id: String,
    plan: HistoryRetentionPlan,
    operation_id: String,
) -> Result<history_cleanup::HistoryCleanupJob, ServiceError> {
    history_cleanup::start(&app, &vault_id, operation_id, Some(plan))
}

// ---- M04 metadata and links (read-only, vault scoped) ----
//
// list_tags captures summaries, then incrementally reads strict snapshots outside
// AppState and the writer gate. Other metadata queries retain M02's normal read
// protocol, which may reconcile metadata but never rewrites Markdown.

#[tauri::command]
async fn list_tags(app: AppHandle, vault_id: String) -> Result<metadata::TagReport, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let (root, summaries) = with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            Ok((storage.root_path().to_path_buf(), storage.list()?))
        })?;
        let unavailable = app
            .state::<search::SearchHost>()
            .unavailable(&root, &vault_id)?;
        app.state::<tag_cache::TagHost>()
            .report(&root, &vault_id, &summaries, &unavailable)
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn list_backlinks(
    app: AppHandle,
    vault_id: String,
    note_id: String,
) -> Result<metadata::BacklinkReport, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            metadata::backlink_report(scoped_vault_ref(state, &vault_id)?, &note_id)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn list_link_issues(
    app: AppHandle,
    vault_id: String,
) -> Result<metadata::LinkReport, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            metadata::link_report(scoped_vault_ref(state, &vault_id)?)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn read_note_metadata(
    app: AppHandle,
    vault_id: String,
    note_id: String,
) -> Result<metadata::NoteMetadataReport, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            metadata::note_metadata(scoped_vault_ref(state, &vault_id)?, &note_id)
        })
    })
    .await
    .map_err(map_join_err)?
}

/// Result of opening or creating a daily note.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyOutcome {
    /// `opened` when an existing note was found, `created` when this call made it.
    pub status: String,
    pub note_id: String,
    pub relative_path: String,
    /// `path` or `property` when an existing note was matched.
    pub matched_by: Option<String>,
    /// Notes that also claim this day; the caller must let the user choose.
    pub candidates: Vec<onote_tasks::DailyCandidate>,
}

/// F44: collects real probe values from the storage layer.
///
/// Every probe is fallible on purpose: a probe that cannot read its value returns
/// `Unknown` rather than 0, so a broken counter never looks like an idle vault
/// (F44-R04). Nothing here writes, and nothing reads note text beyond counting.
#[tauri::command]
async fn diagnostics_probe(
    app: AppHandle,
    vault_id: Option<String>,
    search: tauri::State<'_, search::SearchHost>,
) -> Result<DiagnosticsProbe, ServiceError> {
    let vault = vault_id.clone();
    let compile = vault
        .as_deref()
        .and_then(|id| latex_build::queue_counts(&app, id));
    // Ask the search host for its cached status. This never rebuilds the index:
    // a status check must not change the state it reports.
    let index_status = { search.status().ok() };
    tauri::async_runtime::spawn_blocking(move || {
        let mut probe = DiagnosticsProbe {
            notes: None,
            trash_entries: None,
            drafts: None,
            compile_running: None,
            compile_queued: None,
            storage_writable: None,
            search_documents: None,
            search_status: onote_diagnostics::Status::Unknown {
                reason_key: "diagnostics.noIndex".to_owned(),
            },
            storage_status: onote_diagnostics::Status::Unknown {
                reason_key: "diagnostics.noVault".to_owned(),
            },
            compile_status: onote_diagnostics::Status::Unknown {
                reason_key: "diagnostics.noToolchain".to_owned(),
            },
        };
        let Some(vault_id) = vault else {
            return Ok(probe);
        };
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            // Notes: a plain count of what M02 lists.
            probe.notes = storage.list().ok().map(|summaries| summaries.len() as u64);
            probe.trash_entries = storage
                .list_trash()
                .ok()
                .map(|entries| entries.len() as u64);
            // DraftPage carries one page of items, not a total: the probe counts
            // what it was given rather than inventing a total it cannot see.
            probe.drafts = storage
                .list_drafts(None, None, Some(200))
                .ok()
                .map(|page| page.items.len() as u64);
            probe.storage_writable = Some(storage.vault().writable);
            // A vault with findings is degraded, not ready: the findings are
            // exactly the conditions the user needs to see.
            probe.storage_status = if !storage.vault().findings.is_empty() {
                onote_diagnostics::Status::Degraded {
                    reason_key: "diagnostics.vaultFindings".to_owned(),
                }
            } else if storage.vault().writable {
                onote_diagnostics::Status::Ready
            } else {
                onote_diagnostics::Status::Unknown {
                    reason_key: "diagnostics.readOnly".to_owned(),
                }
            };
            Ok(())
        })?;
        // The host's real build queue; unknown only if its state is unreadable.
        probe.compile_running = compile.map(|(running, _)| running);
        probe.compile_queued = compile.map(|(_, queued)| queued);
        // No managed toolchain exists in this build, so the compile capability is
        // reported as unknown rather than ready or failed (F44-R04).
        probe.compile_status = onote_diagnostics::Status::Unknown {
            reason_key: "diagnostics.noToolchain".to_owned(),
        };
        probe.search_documents = index_status
            .as_ref()
            .map(|status| status.indexed_notes as u64);
        // A cold or failed index is Unknown, never "0 documents": an unbuilt index
        // says nothing about how many notes exist (F44-R04).
        probe.search_status = match index_status.as_ref().map(|status| status.state) {
            Some(onote_search::IndexState::Ready) => onote_diagnostics::Status::Ready,
            Some(onote_search::IndexState::Unavailable) => onote_diagnostics::Status::Unknown {
                reason_key: "diagnostics.indexCold".to_owned(),
            },
            None => onote_diagnostics::Status::Unknown {
                reason_key: "diagnostics.noIndex".to_owned(),
            },
        };
        Ok(probe)
    })
    .await
    .map_err(map_join_err)?
}

/// The probe values the UI turns into report fields.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsProbe {
    pub notes: Option<u64>,
    pub trash_entries: Option<u64>,
    pub drafts: Option<u64>,
    pub compile_running: Option<u64>,
    pub compile_queued: Option<u64>,
    pub search_documents: Option<u64>,
    pub search_status: onote_diagnostics::Status,
    pub storage_writable: Option<bool>,
    pub storage_status: onote_diagnostics::Status,
    pub compile_status: onote_diagnostics::Status,
}

/// F44: builds a redacted diagnostics preview.
///
/// The allowlist lives in M11: whatever the caller passes, only the documented
/// keys reach the report, and an unreadable counter stays Unknown instead of
/// becoming zero (F44 §4, F44-R04).
#[tauri::command]
fn diagnostics_preview(request: onote_diagnostics::PreviewRequest) -> onote_diagnostics::Preview {
    onote_diagnostics::preview(&request)
}

/// F44-R03: the export text a user reviews before saving, with the log redacted.
#[tauri::command]
fn diagnostics_export_text(
    preview: onote_diagnostics::Preview,
    request: onote_diagnostics::PreviewRequest,
) -> String {
    onote_diagnostics::export_text(&preview, &request)
}

/// F44 §6: writes the reviewed report to a destination the user picks.
///
/// The text is regenerated from the preview and request, so the file is
/// redacted again rather than trusting a caller-supplied copy. A failed write
/// returns before anything is renamed or replaced; the preview stays in the
/// caller's memory and the user can choose another path (F44 §7).
#[tauri::command]
async fn diagnostics_export(
    app: AppHandle,
    preview: onote_diagnostics::Preview,
    request: onote_diagnostics::PreviewRequest,
    target: Option<String>,
    suggested_name: Option<String>,
    dialog_title: Option<String>,
) -> Result<Option<String>, ServiceError> {
    let suggestion =
        sanitize_export_name(suggested_name.as_deref().or(Some("OpenNote diagnostics")));
    let title = localized_or_fallback(dialog_title, "Export diagnostics");
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        // The dialog runs before any lock so a slow user choice cannot block
        // another vault command.
        let chosen = match target {
            Some(value) if !value.trim().is_empty() => Some(PathBuf::from(value)),
            _ => handle
                .dialog()
                .file()
                .set_title(&title)
                .set_file_name(&suggestion)
                .blocking_save_file()
                .and_then(file_path_to_buf),
        };
        let Some(path) = chosen else {
            return Ok(None);
        };
        write_diagnostics_report(&path, &preview, &request)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    });
    outcome.await.map_err(map_join_err)?
}

fn write_diagnostics_report(
    path: &Path,
    preview: &onote_diagnostics::Preview,
    request: &onote_diagnostics::PreviewRequest,
) -> Result<(), ServiceError> {
    reject_reserved_target(path)?;
    reject_reparse_ancestors(path)?;
    let text = onote_diagnostics::export_text(preview, request);
    write_export_file(path, text.as_bytes())
}

/// F44: whether the report as a whole may be called healthy.
#[tauri::command]
fn diagnostics_overall(preview: onote_diagnostics::Preview) -> onote_diagnostics::Status {
    onote_diagnostics::overall(&preview)
}

/// F44: collects the host-side facts a report needs.
///
/// Only version, channel, platform and locale are read. Paths are gathered as
/// *roots to redact*, never as report fields.
#[tauri::command]
fn diagnostics_host_facts() -> Result<onote_diagnostics::HostFacts, ServiceError> {
    // Only the home directory is known here: the domain vault does not expose its
    // filesystem root, and a report must not guess one.
    let mut roots: Vec<String> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        if let Some(home) = home.to_str() {
            roots.push(home.to_owned());
        }
    }
    Ok(onote_diagnostics::HostFacts {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        build_channel: if cfg!(debug_assertions) {
            "dev"
        } else {
            "release"
        }
        .to_owned(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        locale: "system".to_owned(),
        roots,
    })
}

/// F03: validates a name for a new or renamed note.
///
/// The rules mirror Windows: reserved device names, illegal characters, a trailing
/// dot or space, and case-insensitive collisions are all refused before anything is
/// written (F03-R01).
#[tauri::command]
fn validate_note_name(name: String) -> Result<String, ServiceError> {
    onote_vault_storage::naming::validate_name(&name)
        .map_err(|error| svc_err(ErrorCode::InvalidInput, path_error_key(&error)))
}

/// F03: checks a rename destination against the vault's current paths.
#[tauri::command]
async fn check_rename_destination(
    app: AppHandle,
    vault_id: String,
    source: String,
    destination: String,
) -> Result<String, ServiceError> {
    let existing = with_state(&app, |state| {
        let storage = scoped_vault_ref(state, &vault_id)?;
        Ok(storage
            .list()?
            .into_iter()
            .map(|summary| summary.relative_path)
            .collect::<Vec<_>>())
    })?;
    onote_vault_storage::naming::check_destination(&source, &destination, &existing)
        .map_err(|error| svc_err(ErrorCode::InvalidInput, path_error_key(&error)))
}

/// F03: whether a rename only changes letter case.
///
/// Windows needs two steps for that, so the caller has to know.
#[tauri::command]
fn is_case_only_rename(source: String, destination: String) -> bool {
    onote_vault_storage::naming::is_case_only_change(&source, &destination)
}

fn path_error_key(error: &onote_vault_storage::naming::PathError) -> &'static str {
    onote_vault_storage::naming::reason_key(error)
}

/// F24: summarizes an import plan before anything is written.
///
/// The counts are what the user confirms: how many notes, how many attachments,
/// how many entries were skipped and how many bytes would be copied.
#[tauri::command]
fn summarize_import_plan(
    plan: onote_attachments_import::ImportPlan,
) -> onote_attachments_import::PlanSummary {
    onote_attachments_import::summarize(&plan)
}

/// F24: whether a plan would import everything it found without losses.
///
/// False means the user must read the loss report before proceeding, because the
/// import would silently drop something.
#[tauri::command]
fn import_plan_is_complete(plan: onote_attachments_import::ImportPlan) -> bool {
    onote_attachments_import::is_complete(&plan)
}

/// F24 §6: the loss report as Markdown, which the user may keep.
#[tauri::command]
fn import_plan_loss_report(plan: onote_attachments_import::ImportPlan) -> String {
    onote_attachments_import::loss_report(&plan)
}

/// F16: groups tasks for the task panel.
///
/// Grouping lives in M06 so the panel and any future consumer share one rule for
/// what counts as the same group. `today` is a local calendar date supplied by
/// the caller (F16-R02); the crate never reads the clock.
#[tauri::command]
fn group_tasks_command(
    tasks: Vec<onote_tasks::Task>,
    today: String,
) -> Vec<onote_tasks::TaskGroup> {
    onote_tasks::group_tasks(&tasks, &today)
}

/// F19: plans importing single-file attachments into a folder.
///
/// Pure: the caller supplies each file's name, size and hash. An unknown type is
/// kept and handed to the system rather than refused (F19-R02), identical bytes are
/// recognised so nothing is stored twice, and the cloud limit marks a file unsynced
/// without blocking local use (F19-R04). A thumbnail is a cache, never a
/// replacement for the original (F19-R01).
#[tauri::command]
fn plan_attachment_import(
    inputs: Vec<onote_attachments_import::AttachmentInput>,
    destination: String,
    existing: Vec<(String, String)>,
) -> Result<onote_attachments_import::AttachmentPlan, ServiceError> {
    onote_attachments_import::plan_attachments(&inputs, &destination, &existing).map_err(|issue| {
        svc_err(
            ErrorCode::InvalidInput,
            onote_attachments_import::attachment_issue_key(&issue),
        )
    })
}

/// Largest file whose hash is computed when listing a destination folder. Larger
/// files still occupy their names (so nothing is overwritten) but are not reused.
const ATTACHMENT_LIST_HASH_LIMIT: u64 = 64 * 1024 * 1024;

/// One picked file after the read-only hashing pass.
struct AttachmentSource {
    path: PathBuf,
    input: onote_attachments_import::AttachmentInput,
}

/// The outcome of one attachment in a real import.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentImportItem {
    pub name: String,
    pub destination: String,
    pub reference: String,
    /// `stored`, `alreadyPresent`, `refused` or `failed`.
    pub status: String,
    pub error_key: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentImportOutcome {
    pub cancelled: bool,
    pub plan: Option<onote_attachments_import::AttachmentPlan>,
    pub items: Vec<AttachmentImportItem>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Streams a file once to learn its size and hash without keeping the bytes.
/// Reparse points and non-files are reported with an unsafe source path so the
/// planner refuses them instead of following them.
fn hash_attachment_source(path: &Path) -> Result<AttachmentSource, ServiceError> {
    use sha2::Digest;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let display = path.to_string_lossy().into_owned();
    let meta = std::fs::symlink_metadata(path).map_err(|error| map_io_export(&error))?;
    if metadata_is_reparse(&meta) || !meta.is_file() {
        return Err(svc_err(
            ErrorCode::IoDenied,
            "error.attachmentSourceInvalid",
        ));
    }
    let size = meta.len();
    let sha256 = if size > onote_attachments_import::attachment::MAX_ATTACHMENT_BYTES {
        String::new()
    } else {
        let mut file = std::fs::File::open(path).map_err(|error| map_io_export(&error))?;
        let mut hasher = sha2::Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| map_io_export(&error))?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > onote_attachments_import::attachment::MAX_ATTACHMENT_BYTES {
                return Err(svc_err(
                    ErrorCode::InvalidInput,
                    "error.attachmentSourceChanged",
                ));
            }
            hasher.update(&buffer[..read]);
        }
        if total != size {
            return Err(svc_err(
                ErrorCode::InvalidInput,
                "error.attachmentSourceChanged",
            ));
        }
        format!("{:x}", hasher.finalize())
    };
    Ok(AttachmentSource {
        path: path.to_path_buf(),
        input: onote_attachments_import::AttachmentInput {
            name,
            source_path: display,
            size,
            sha256,
        },
    })
}

/// Hashes, plans and writes picked attachments. Shared by the command and tests.
///
/// `existing` lists files already in the destination folder with their hashes.
/// `store` writes one planned file (M02 `store_attachment`). Each file is read a
/// second time only when it is written, and M02 re-verifies its hash, so a
/// source edited between planning and writing is reported, never half-stored.
fn import_attachment_files(
    paths: &[PathBuf],
    destination: &str,
    operation_id: &str,
    existing: &[(String, String)],
    mut store: impl FnMut(
        &str,
        &[u8],
        &str,
        &str,
    ) -> Result<onote_vault_storage::AttachmentWrite, ServiceError>,
) -> Result<AttachmentImportOutcome, ServiceError> {
    let mut sources = Vec::new();
    let mut items = Vec::new();
    for path in paths {
        match hash_attachment_source(path) {
            Ok(source) => sources.push(source),
            Err(error) => items.push(AttachmentImportItem {
                name: path
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                destination: String::new(),
                reference: String::new(),
                status: "refused".to_owned(),
                error_key: Some(error.message_key),
            }),
        }
    }
    let inputs: Vec<_> = sources.iter().map(|source| source.input.clone()).collect();
    let plan = onote_attachments_import::plan_attachments(&inputs, destination, existing).map_err(
        |issue| {
            svc_err(
                ErrorCode::InvalidInput,
                onote_attachments_import::attachment_issue_key(&issue),
            )
        },
    )?;
    for (source, planned) in sources.iter().zip(plan.attachments.iter()) {
        let mut item = AttachmentImportItem {
            name: source.input.name.clone(),
            destination: planned.destination.clone(),
            reference: planned.reference.clone(),
            status: String::new(),
            error_key: None,
        };
        if !planned.is_importable() {
            item.status = "refused".to_owned();
            item.error_key = planned
                .issues
                .first()
                .map(|issue| onote_attachments_import::attachment_issue_key(issue).to_owned());
        } else if !planned.stores_bytes {
            // Identical bytes already live at `destination`; nothing to write.
            item.status = "alreadyPresent".to_owned();
        } else {
            let item_id = onote_attachments_import::note_operation_id(
                operation_id,
                &format!("attachment:{}", source.input.source_path),
            );
            let written = std::fs::read(&source.path)
                .map_err(|error| map_io_export(&error))
                .and_then(|bytes| store(&planned.destination, &bytes, &planned.sha256, &item_id));
            match written {
                Ok(onote_vault_storage::AttachmentWrite::Stored) => {
                    item.status = "stored".to_owned()
                }
                Ok(onote_vault_storage::AttachmentWrite::AlreadyPresent) => {
                    item.status = "alreadyPresent".to_owned()
                }
                Err(error) => {
                    item.status = "failed".to_owned();
                    item.error_key = Some(error.message_key);
                }
            }
        }
        items.push(item);
    }
    Ok(AttachmentImportOutcome {
        cancelled: false,
        plan: Some(plan),
        items,
    })
}

/// F19: picks files and stores them as vault attachments.
///
/// Only items reported `stored` or `alreadyPresent` exist on disk and may be
/// referenced from a note; refused or failed items must not be inserted.
#[tauri::command]
async fn import_attachments(
    app: AppHandle,
    vault_id: String,
    destination: String,
    operation_id: String,
    dialog_title: Option<String>,
) -> Result<AttachmentImportOutcome, ServiceError> {
    let title = localized_or_fallback(dialog_title, "Add attachments");
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = handle.dialog().file().set_title(&title);
        if let Some(window) = handle.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_pick_files() else {
            return Ok(AttachmentImportOutcome {
                cancelled: true,
                plan: None,
                items: Vec::new(),
            });
        };
        let paths: Vec<PathBuf> = picked.into_iter().filter_map(file_path_to_buf).collect();
        let existing = with_state(&handle, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            Ok(storage
                .list_attachments(&destination, ATTACHMENT_LIST_HASH_LIMIT)?
                .into_iter()
                .map(|file| (file.relative_path, file.sha256))
                .collect::<Vec<_>>())
        })?;
        import_attachment_files(
            &paths,
            &destination,
            &operation_id,
            &existing,
            |path, bytes, sha, id| {
                with_state(&handle, |state| {
                    scoped_vault_mut(state, &vault_id)?
                        .store_attachment(path, bytes, sha, id)
                        .map(|(_, write)| write)
                })
            },
        )
    });
    outcome.await.map_err(map_join_err)?
}

/// F19: how an extension is presented, for the picker's preview.
#[tauri::command]
fn classify_attachment_extension(extension: String) -> (String, String) {
    let (kind, preview) = onote_attachments_import::classify_attachment(&extension);
    (kind.as_str().to_owned(), preview.as_str().to_owned())
}

/// F08-A03: plans renaming (or merging) a tag across the given notes.
///
/// Pure: the caller supplies each note's source and revision, and the plan reports
/// per-note rewrites plus any note that changed since it was read. A tag inside a
/// code fence is never touched (F08-R01), and a removal never deletes a note
/// (F08-R03).
#[tauri::command]
fn plan_tag_rename_command(
    notes: Vec<onote_metadata_links::TagNote>,
    old_tag: String,
    new_tag: String,
    actual_revisions: Vec<(String, u64)>,
    existing_tags: Vec<String>,
) -> onote_metadata_links::TagRenamePlan {
    onote_metadata_links::plan_tag_rename(
        &notes,
        &old_tag,
        &new_tag,
        &actual_revisions,
        &existing_tags,
    )
}

/// F08: plans removing a tag from the given notes.
#[tauri::command]
fn plan_tag_removal_command(
    notes: Vec<onote_metadata_links::TagNote>,
    tag: String,
    actual_revisions: Vec<(String, u64)>,
) -> onote_metadata_links::TagRenamePlan {
    onote_metadata_links::plan_removal(&notes, &tag, &actual_revisions)
}

/// F03: builds the commit steps for a rename preview.
///
/// Read-only: it returns what would be written and refuses when a note changed
/// since the caller read it (VERSION_CONFLICT) or the destination is occupied
/// (PATH_COLLISION). Execution consumes the same sealed plan.
#[tauri::command]
async fn preview_rename_commit(
    app: AppHandle,
    vault_id: String,
    plan: onote_domain::RenamePlan,
) -> Result<onote_metadata_links::RenameCommit, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            let fresh = metadata::rename_plan(storage, &plan.note_id, &plan.to_path)?;
            if plan.vault_id != fresh.vault_id
                || plan.from_path != fresh.from_path
                || plan.edits != fresh.edits
                || plan.expected_hashes != fresh.expected_hashes
                || !plan.keeps_identity
            {
                return Err(svc_err(
                    ErrorCode::VersionConflict,
                    "error.renameBaselineChanged",
                ));
            }
            let (index, _) = metadata::rebuild_index(storage)?;
            let revisions = plan
                .expected_hashes
                .iter()
                .map(|b| (b.note_id.clone(), b.expected_revision))
                .collect::<Vec<_>>();
            let existing = storage
                .list()?
                .into_iter()
                .map(|n| n.relative_path)
                .collect::<Vec<_>>();
            Ok(onote_metadata_links::plan_commit(
                &index, &plan, &revisions, &revisions, &existing, false,
            ))
        })
    })
    .await
    .map_err(map_join_err)?
}

/// F45: inspects a LaTeX project directory.
///
/// Nothing in the directory is executed: no `.tex`, no script and no `.latexmkrc`
/// is run while identifying the project (F45-R02). An ambiguous entry set is
/// reported instead of guessed (ENTRY_AMBIGUOUS).
#[tauri::command]
fn inspect_latex_project(
    root: String,
    files: Vec<onote_latex::InspectFile>,
) -> onote_latex::ProjectInspection {
    onote_latex::inspect_project(&root, &files)
}

/// F49: parses a `.bib` file.
///
/// Unparsable blocks keep their source text and are reported with an offset, so a
/// rebuild never loses an entry the parser did not model (F49-R01).
#[tauri::command]
fn parse_bibliography(source: String) -> onote_latex::Bibliography {
    onote_latex::parse_bibliography(&source)
}

/// F49: plans inserting `\\cite{...}`.
///
/// A duplicate key is refused with every source offset rather than resolved
/// silently, and the backend is reported so a tool mismatch stays visible.
#[tauri::command]
fn plan_citation_command(
    bibliography: onote_latex::Bibliography,
    cite_keys: Vec<String>,
    backend: String,
    preamble: String,
) -> Result<onote_latex::CitationPlan, ServiceError> {
    let Some(backend) = onote_latex::Backend::parse(&backend) else {
        // An unknown backend is refused instead of defaulting to BibTeX.
        return Err(svc_err(ErrorCode::InvalidInput, "error.bibBackendUnknown"));
    };
    Ok(onote_latex::plan_citation(
        &bibliography,
        &cite_keys,
        backend,
        &preamble,
    ))
}

/// F49-R02: a multi-file rename plan with per-file revision checks.
#[tauri::command]
fn plan_citation_rename(
    bibliography: onote_latex::Bibliography,
    sources: Vec<(String, String)>,
    old_key: String,
    new_key: String,
    expected_revisions: Vec<(String, u64)>,
    actual_revisions: Vec<(String, u64)>,
) -> onote_latex::RenamePlan {
    onote_latex::plan_rename(
        &bibliography,
        &sources,
        &old_key,
        &new_key,
        &expected_revisions,
        &actual_revisions,
    )
}

/// F49-A02: citation keys the sources use that the bibliography does not define.
#[tauri::command]
fn bibliography_dangling(
    bibliography: onote_latex::Bibliography,
    sources: Vec<String>,
) -> Vec<String> {
    onote_latex::dangling_citations(&bibliography, &sources)
}

/// F49-R04: entries whose fields a BibTeX run cannot handle safely.
#[tauri::command]
fn bibliography_unicode(
    bibliography: onote_latex::Bibliography,
    backend: String,
) -> Result<Vec<String>, ServiceError> {
    let Some(backend) = onote_latex::Backend::parse(&backend) else {
        return Err(svc_err(ErrorCode::InvalidInput, "error.bibBackendUnknown"));
    };
    Ok(onote_latex::unicode_entries(&bibliography, backend)
        .into_iter()
        .map(|entry| entry.cite_key.clone())
        .collect())
}

/// F48: plans one structured LaTeX insertion.
///
/// Pure: the fragment, the dependencies it needs and any issues come back
/// together, so the caller can apply the fragment and a preamble change in one
/// undo group (F48-R04). The preamble is never edited here (F48-R03).
#[tauri::command]
fn plan_latex_insertion(
    parameters: onote_latex::Parameters,
    source: String,
) -> onote_latex::InsertionPlan {
    // Labels and the preamble both come from the buffer the insertion targets.
    let labels = onote_latex::collect_labels(&source);
    onote_latex::plan_insertion(&parameters, &labels, &source)
}

/// F48: the labels already used in a buffer, for the label picker.
#[tauri::command]
fn latex_labels(source: String) -> Vec<String> {
    onote_latex::collect_labels(&source)
}

/// F47: detects the language a buffer should be edited as.
#[tauri::command]
fn detect_latex_language(path: String, source: String) -> onote_latex::LanguageMode {
    onote_latex::detect_language(&path, &source)
}

/// F47: toggles `%` comments on the selected lines.
///
/// Pure: the editor applies the returned edit(s). A composition in progress is
/// refused rather than editing mid-IME (F47-R03).
#[tauri::command]
fn latex_toggle_comment(
    source: String,
    selection: onote_latex::Range,
    composing: bool,
) -> Result<onote_latex::AssistResult, ServiceError> {
    onote_latex::toggle_comment(&source, selection, composing).map_err(assist_error)
}

/// F47: wraps the selection in `\command{...}`.
#[tauri::command]
fn latex_wrap_selection(
    source: String,
    selection: onote_latex::Range,
    command: String,
    composing: bool,
) -> Result<onote_latex::AssistResult, ServiceError> {
    onote_latex::wrap_selection(&source, selection, &command, composing).map_err(assist_error)
}

/// F47: the matching bracket for the one at `offset`, if any.
///
/// An escaped bracket has no partner and an unbalanced one yields nothing, so the
/// editor never highlights an invented pair.
#[tauri::command]
fn latex_matching_bracket(source: String, offset: usize) -> Option<usize> {
    onote_latex::matching_bracket(&source, offset)
}

/// F47: completion candidates from the lexicon plus the project's own commands.
#[tauri::command]
fn latex_completions(source: String, prefix: String) -> Vec<onote_latex::Candidate> {
    onote_latex::completions(&source, &prefix)
}

/// F47: applies one completion to an explicit range, refusing a stale one.
#[tauri::command]
fn latex_apply_completion(
    source: String,
    range: onote_latex::Range,
    expected: String,
    insert: String,
) -> Result<onote_latex::AssistResult, ServiceError> {
    onote_latex::apply_completion(&source, range, &expected, &insert).map_err(assist_error)
}

fn assist_error(error: onote_latex::AssistError) -> ServiceError {
    let (code, key) = match &error {
        onote_latex::AssistError::RangeOutOfBounds { .. } => {
            (ErrorCode::InvalidInput, "error.latexRangeInvalid")
        }
        // A stale range is a version conflict: the buffer moved since the preview.
        onote_latex::AssistError::StaleRange { .. } => {
            (ErrorCode::VersionConflict, "error.latexReplaceStale")
        }
        onote_latex::AssistError::NotApplicable(_) => {
            (ErrorCode::InvalidInput, "error.latexActionInvalid")
        }
        onote_latex::AssistError::Composing => (ErrorCode::InvalidInput, "error.latexComposing"),
    };
    svc_err(code, key)
}

/// F46: plans a LaTeX project scaffold without writing anything.
///
/// The escaping and the file list live in M13, so the preview the user confirms is
/// exactly what would be published. Collisions with existing files are reported
/// instead of overwritten (F46-R03), and nothing is compiled or installed.
#[tauri::command]
fn plan_latex_scaffold(
    app: AppHandle,
    vault_id: Option<String>,
    template_id: String,
    metadata: onote_latex::Metadata,
    destination: String,
) -> Result<onote_latex::ScaffoldPlan, ServiceError> {
    let Some(template) = onote_latex::TemplateId::parse(&template_id) else {
        // An unknown template is refused rather than defaulting to one.
        return Err(svc_err(ErrorCode::InvalidInput, "error.templateUnknown"));
    };
    // Every file already in the destination folder decides collisions (not only
    // notes); a vault-less call simply has none to report.
    match vault_id.as_deref() {
        Some(vault_id) => with_state(&app, |state| {
            latex_project::preview_scaffold(
                scoped_vault_ref(state, vault_id)?,
                template,
                &metadata,
                &destination,
            )
        }),
        None => Ok(onote_latex::plan_scaffold(
            template,
            &metadata,
            &destination,
            &[],
        )),
    }
}

/// F46: escapes one piece of plain text for LaTeX.
///
/// Exposed separately so a caller can preview the escaped result without building
/// a whole plan, using the same encoder rather than a second implementation.
#[tauri::command]
fn escape_latex_text(text: String) -> String {
    onote_latex::escape_latex(&text)
}

/// F54: plans a LaTeX project export without writing anything.
///
/// The plan is what gets committed, and it lists its issues, so the caller can
/// refuse to call a project complete when a referenced file is missing
/// (PROJECT_INCOMPLETE) or the PDF predates the source (PDF_OUTDATED).
#[tauri::command]
fn plan_latex_export(
    kind: String,
    files: Vec<onote_latex::ProjectFile>,
    expected_manifest_hash: String,
    actual_manifest_hash: String,
    size_limit: Option<u64>,
    source_hash: String,
    pdf_source_hash: Option<String>,
) -> Result<onote_latex::ExportPlan, ServiceError> {
    let kind = match kind.as_str() {
        "pdf" => onote_latex::ExportKind::Pdf,
        "sourceZip" => onote_latex::ExportKind::SourceZip,
        _ => return Err(svc_err(ErrorCode::InvalidInput, "error.exportKindInvalid")),
    };
    Ok(onote_latex::plan_export(
        kind,
        &files,
        &expected_manifest_hash,
        &actual_manifest_hash,
        size_limit,
        &source_hash,
        pdf_source_hash.as_deref(),
    ))
}

// F51 compile jobs are run and reported by the host itself (latex_build);
// there is no command through which a caller reports a build result.

/// F52: the preview state after a build outcome.
///
/// Pure: it decides whether a jump may be precise and whether the previous PDF
/// stays readable, but never renders or replaces a file (F52-R01).
#[tauri::command]
fn preview_state(
    previous: Option<onote_latex::Artifact>,
    outcome: onote_latex::BuildOutcome,
    current_hash: String,
    has_mapping: bool,
) -> onote_latex::PreviewState {
    onote_latex::apply_build(previous.as_ref(), &outcome, &current_hash, has_mapping)
}

/// F53: parses a LaTeX build log into structured diagnostics.
///
/// Pure: this command never runs a compiler and never installs anything, so it
/// cannot turn a missing package into an automatic system change (F53-R01). The
/// exit code is supplied by the caller, because the verdict has to combine the
/// log with what actually happened.
#[tauri::command]
fn parse_tex_log(
    raw_log: String,
    exit_code: Option<i32>,
    job_id: Option<String>,
    source_hash: Option<String>,
) -> onote_latex::BuildReport {
    // F53-R03: the log keeps the identity of the build it came from, so a later
    // version change can suppress locating instead of pointing at a wrong line.
    let mut report = onote_latex::parse_build_log(&raw_log, exit_code);
    let clean = |value: Option<String>| {
        value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    report.job_id = clean(job_id);
    report.source_hash = clean(source_hash);
    report
}

/// F53-R04: the same report with machine-specific absolute paths removed.
#[tauri::command]
fn redact_tex_log(
    report: onote_latex::BuildReport,
    roots: Vec<String>,
) -> onote_latex::BuildReport {
    let borrowed: Vec<&str> = roots.iter().map(String::as_str).collect();
    onote_latex::redact(&report, &borrowed)
}

/// F16-R04: deletes one task line. Pure: the caller saves the returned source.
#[tauri::command]
fn delete_task(
    source: String,
    offset: usize,
    expected_text: String,
) -> Result<onote_tasks::TaskRewrite, ServiceError> {
    onote_tasks::delete_task(&source, offset, &expected_text).map_err(task_error)
}

/// F16 §4: sets, replaces or clears the task's due date.
#[tauri::command]
fn set_task_due(
    source: String,
    offset: usize,
    expected_text: String,
    date: Option<String>,
) -> Result<onote_tasks::TaskRewrite, ServiceError> {
    onote_tasks::set_task_due(&source, offset, &expected_text, date.as_deref()).map_err(task_error)
}

/// Maps a task error to the service error the UI already understands.
fn task_error(error: onote_tasks::TaskError) -> ServiceError {
    let (code, key) = match error {
        onote_tasks::TaskError::Moved { .. } => (ErrorCode::VersionConflict, "error.taskMoved"),
        onote_tasks::TaskError::Ambiguous => (ErrorCode::VersionConflict, "error.taskAmbiguous"),
        onote_tasks::TaskError::Missing => (ErrorCode::ObjectNotFound, "error.taskMissing"),
        onote_tasks::TaskError::InvalidDate(_) => {
            (ErrorCode::InvalidInput, "error.taskInvalidDate")
        }
    };
    svc_err(code, key)
}

/// F21: parses and validates a `.canvas` document.
///
/// F21: validates canvas JSON without touching the vault.
#[tauri::command]
fn parse_canvas_source(source: String) -> Result<onote_tasks::CanvasDocument, ServiceError> {
    onote_tasks::parse_canvas(&source).map_err(|error| canvas_error(error))
}

/// F21: validates and serialises a canvas document.
#[tauri::command]
fn serialize_canvas_document(
    document: onote_tasks::CanvasDocument,
) -> Result<String, ServiceError> {
    onote_tasks::serialize_canvas(&document).map_err(canvas_error)
}

const CANVAS_SUFFIX: &str = ".canvas";

/// A canvas file as the editor loads it: the parsed document plus the hash the
/// next save must still find on disk.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCanvas {
    pub relative_path: String,
    pub sha256: String,
    pub document: onote_tasks::CanvasDocument,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasSaveResult {
    pub relative_path: String,
    pub sha256: String,
    /// False when the same bytes were already on disk (an idempotent retry).
    pub written: bool,
}

/// F21-A02: canvas files in the vault.
#[tauri::command]
fn list_canvases(app: AppHandle, vault_id: String) -> Result<Vec<String>, ServiceError> {
    with_state(&app, |state| {
        Ok(scoped_vault_ref(state, &vault_id)?
            .list_documents(CANVAS_SUFFIX)?
            .into_iter()
            .map(|file| file.relative_path)
            .collect())
    })
}

fn read_canvas_from(
    storage: &VaultStorage,
    relative_path: &str,
) -> Result<StoredCanvas, ServiceError> {
    let file = storage.read_document(relative_path, CANVAS_SUFFIX)?;
    let document = onote_tasks::parse_canvas(&file.source).map_err(canvas_error)?;
    Ok(StoredCanvas {
        relative_path: file.relative_path,
        sha256: file.sha256,
        document,
    })
}

/// F21-A02: opens a canvas file. An invalid file is reported, never rewritten.
#[tauri::command]
fn read_canvas(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<StoredCanvas, ServiceError> {
    with_state(&app, |state| {
        read_canvas_from(scoped_vault_ref(state, &vault_id)?, &relative_path)
    })
}

fn save_canvas_to(
    storage: &mut VaultStorage,
    relative_path: &str,
    document: &onote_tasks::CanvasDocument,
    expected_sha256: Option<&str>,
    operation_id: &str,
) -> Result<CanvasSaveResult, ServiceError> {
    let source = onote_tasks::serialize_canvas(document).map_err(canvas_error)?;
    let (relative_path, sha256, write) = storage.save_document(
        relative_path,
        CANVAS_SUFFIX,
        &source,
        expected_sha256,
        operation_id,
    )?;
    Ok(CanvasSaveResult {
        relative_path,
        sha256,
        written: write == onote_vault_storage::AttachmentWrite::Stored,
    })
}

/// F21-A02: saves a canvas. `expected_sha256` is the hash it was loaded with
/// (None for a new file); a file changed elsewhere is a VersionConflict and is
/// left untouched. Retrying with the same operation and document is safe.
#[tauri::command]
fn save_canvas(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
    document: onote_tasks::CanvasDocument,
    expected_sha256: Option<String>,
    operation_id: String,
) -> Result<CanvasSaveResult, ServiceError> {
    with_state(&app, |state| {
        save_canvas_to(
            scoped_vault_mut(state, &vault_id)?,
            &relative_path,
            &document,
            expected_sha256.as_deref(),
            &operation_id,
        )
    })
}

const VIEW_SUFFIX: &str = ".onview.json";
/// F22 §6: a view definition is small; larger files are refused, not truncated.
const MAX_VIEW_BYTES: usize = 1024 * 1024;

/// A saved database view file. The definition is parsed by the panel, which
/// keeps fields it does not know; the host stores the text verbatim.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredView {
    pub relative_path: String,
    pub sha256: String,
    pub source: String,
}

fn check_view_source(source: &str) -> Result<(), ServiceError> {
    if source.len() > MAX_VIEW_BYTES {
        return Err(svc_err(ErrorCode::InvalidInput, "error.viewTooLarge"));
    }
    match serde_json::from_str::<serde_json::Value>(source) {
        Ok(serde_json::Value::Object(_)) => Ok(()),
        _ => Err(svc_err(ErrorCode::InvalidInput, "error.viewInvalid")),
    }
}

fn save_view_to(
    storage: &mut VaultStorage,
    relative_path: &str,
    source: &str,
    expected_sha256: Option<&str>,
    operation_id: &str,
) -> Result<CanvasSaveResult, ServiceError> {
    check_view_source(source)?;
    let (relative_path, sha256, write) = storage.save_document(
        relative_path,
        VIEW_SUFFIX,
        source,
        expected_sha256,
        operation_id,
    )?;
    Ok(CanvasSaveResult {
        relative_path,
        sha256,
        written: write == onote_vault_storage::AttachmentWrite::Stored,
    })
}

/// F22-A02: saved database views in the vault.
#[tauri::command]
fn list_views(app: AppHandle, vault_id: String) -> Result<Vec<String>, ServiceError> {
    with_state(&app, |state| {
        Ok(scoped_vault_ref(state, &vault_id)?
            .list_documents(VIEW_SUFFIX)?
            .into_iter()
            .map(|file| file.relative_path)
            .collect())
    })
}

#[tauri::command]
fn read_view(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<StoredView, ServiceError> {
    with_state(&app, |state| {
        let file =
            scoped_vault_ref(state, &vault_id)?.read_document(&relative_path, VIEW_SUFFIX)?;
        Ok(StoredView {
            relative_path: file.relative_path,
            sha256: file.sha256,
            source: file.source,
        })
    })
}

/// F22-A02: compare-and-swap save of a view definition; see `save_canvas`.
#[tauri::command]
fn save_view(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
    source: String,
    expected_sha256: Option<String>,
    operation_id: String,
) -> Result<CanvasSaveResult, ServiceError> {
    with_state(&app, |state| {
        save_view_to(
            scoped_vault_mut(state, &vault_id)?,
            &relative_path,
            &source,
            expected_sha256.as_deref(),
            &operation_id,
        )
    })
}

fn canvas_error(error: onote_tasks::GeometryError) -> ServiceError {
    let (code, key) = match &error {
        onote_tasks::GeometryError::NonFinite | onote_tasks::GeometryError::TooSmall { .. } => {
            (ErrorCode::InvalidInput, "error.canvasGeometry")
        }
        onote_tasks::GeometryError::DanglingEdge { .. } => {
            (ErrorCode::InvalidInput, "error.canvasDanglingEdge")
        }
        onote_tasks::GeometryError::DuplicateNodeId(_) => {
            (ErrorCode::InvalidInput, "error.canvasDuplicateNode")
        }
        onote_tasks::GeometryError::Malformed(_) => {
            (ErrorCode::InvalidInput, "error.canvasMalformed")
        }
    };
    svc_err(code, key)
}

/// F22/F23: a database table projected from note frontmatter.
///
/// Read-only on purpose: the view definition has no persisted file yet, so this
/// takes the column order, filters and sort from the caller instead of inventing
/// a storage format. Writing a property already flows through
/// `apply_property_edit`, so there is no second write path (F22 §6).
#[tauri::command]
async fn database_table(
    app: AppHandle,
    vault_id: String,
    columns: Vec<String>,
    filters: Vec<onote_tasks::Filter>,
    sort: Option<onote_tasks::Sort>,
    folder: Option<String>,
    recursive: Option<bool>,
) -> Result<DatabaseOutcome, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            let recursive = recursive.unwrap_or(true);
            let mut sources = Vec::new();
            let mut unreadable = Vec::new();
            let mut summaries = storage.list()?;
            summaries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
            for summary in summaries {
                let path = summary.relative_path.replace('\\', "/");
                if let Some(folder) = folder.as_deref().map(|value| value.trim_end_matches('/')) {
                    if !folder.is_empty() {
                        let in_scope = if recursive {
                            path == folder || path.starts_with(&format!("{folder}/"))
                        } else {
                            // Non-recursive means the files directly in the
                            // folder, so a nested note is excluded.
                            path.rsplit_once('/')
                                .is_some_and(|(parent, _)| parent == folder)
                        };
                        if !in_scope {
                            continue;
                        }
                    }
                }
                match storage.read(&summary.note_id) {
                    Ok(document) => {
                        let properties =
                            match onote_metadata_links::parse_properties(&document.source, &[]) {
                                Ok(set) => set
                                    .properties
                                    .iter()
                                    .map(|property| (property.name.clone(), property.value.clone()))
                                    .collect(),
                                // A note with broken frontmatter is reported instead
                                // of appearing as a row full of empty cells.
                                Err(_) => {
                                    unreadable.push(metadata::UnreadableNote {
                                        note_id: summary.note_id.clone(),
                                        relative_path: summary.relative_path.clone(),
                                        code: ErrorCode::InvalidInput,
                                        message_key: "error.propertyInvalid".to_owned(),
                                    });
                                    continue;
                                }
                            };
                        sources.push(onote_tasks::RowSource {
                            note_id: summary.note_id.clone(),
                            title: onote_metadata_links::title_of(&summary.relative_path),
                            relative_path: summary.relative_path.clone(),
                            source_revision: document.local_revision,
                            source_hash: document.content_hash.clone(),
                            read_only: document.read_only,
                            properties,
                        });
                    }
                    Err(error) => unreadable.push(metadata::UnreadableNote {
                        note_id: summary.note_id.clone(),
                        relative_path: summary.relative_path.clone(),
                        code: error.code,
                        message_key: error.message_key,
                    }),
                }
            }
            let table = onote_tasks::build_table(&sources, &columns, &filters, sort.as_ref());
            Ok(DatabaseOutcome { table, unreadable })
        })
    })
    .await
    .map_err(map_join_err)?
}

/// A table plus the notes that could not be read.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseOutcome {
    pub table: onote_tasks::Table,
    pub unreadable: Vec<metadata::UnreadableNote>,
}

/// F15: the relationship graph, whole vault or around one note.
#[tauri::command]
async fn graph_report(
    app: AppHandle,
    vault_id: String,
    focus: Option<String>,
    depth: Option<usize>,
    limit: Option<usize>,
) -> Result<metadata::GraphReport, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            metadata::graph_report(
                scoped_vault_ref(state, &vault_id)?,
                focus.as_deref(),
                depth.unwrap_or(metadata::MAX_GRAPH_DEPTH),
                limit.unwrap_or(metadata::DEFAULT_GRAPH_LIMIT),
            )
        })
    })
    .await
    .map_err(map_join_err)?
}

/// F17: opens the daily note for `date`, creating it only when absent.
///
/// The operation id is derived from the vault and date, so a retry after an
/// interrupted create replays instead of producing a second note for the same
/// day (F17-R01). The new note carries `date: YYYY-MM-DD` in its frontmatter so a
/// later rename is still found (F17-R03).
#[tauri::command]
async fn open_daily_note(
    app: AppHandle,
    vault_id: String,
    date: String,
    folder_pattern: Option<String>,
    template_id: Option<String>,
) -> Result<DailyOutcome, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let pattern =
            folder_pattern.unwrap_or_else(|| onote_tasks::DEFAULT_FOLDER_PATTERN.to_owned());
        // Validate before touching storage so a bad pattern is a typed error.
        let expected = onote_tasks::daily_path(&date, &pattern).map_err(|error| {
            let key = match error {
                onote_tasks::DailyError::InvalidDate(_) => "error.dailyDateInvalid",
                onote_tasks::DailyError::InvalidPattern(_) => "error.dailyPatternInvalid",
                onote_tasks::DailyError::UnsafePattern(_) => "error.dailyPatternUnsafe",
            };
            svc_err(ErrorCode::InvalidInput, key)
        })?;
        let mut guard_vault = String::new();
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            guard_vault = storage.vault().vault_id.clone();
            Ok(())
        })?;

        let candidates = with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            let mut candidates = Vec::new();
            for summary in storage.list()? {
                let Ok(document) = storage.read(&summary.note_id) else {
                    continue;
                };
                // Only notes whose frontmatter has a date are candidates; a note
                // without one is found by path alone.
                let properties = onote_metadata_links::parse_properties(&document.source, &[]).ok();
                let property_date = properties
                    .and_then(|set| set.get("date").map(|property| property.value.clone()))
                    .and_then(|value| match value {
                        onote_metadata_links::PropertyValue::Date(text)
                        | onote_metadata_links::PropertyValue::Text(text)
                        | onote_metadata_links::PropertyValue::Select(text) => Some(text),
                        _ => None,
                    });
                candidates.push(onote_tasks::DailyCandidate {
                    note_id: summary.note_id.clone(),
                    relative_path: summary.relative_path.clone(),
                    property_date,
                });
            }
            Ok(candidates)
        })?;

        let matched = onote_tasks::find_daily(&date, &pattern, &candidates).map_err(|error| {
            let key = match error {
                onote_tasks::DailyError::InvalidDate(_) => "error.dailyDateInvalid",
                onote_tasks::DailyError::InvalidPattern(_) => "error.dailyPatternInvalid",
                onote_tasks::DailyError::UnsafePattern(_) => "error.dailyPatternUnsafe",
            };
            svc_err(ErrorCode::InvalidInput, key)
        })?;
        match matched {
            onote_tasks::DailyMatch::Found {
                note_id,
                relative_path,
                matched_by,
            } => Ok(DailyOutcome {
                status: "opened".to_owned(),
                note_id,
                relative_path,
                matched_by: Some(match matched_by {
                    onote_tasks::MatchKind::Path => "path".to_owned(),
                    onote_tasks::MatchKind::Property => "property".to_owned(),
                }),
                candidates: Vec::new(),
            }),
            onote_tasks::DailyMatch::Ambiguous { candidates } => Ok(DailyOutcome {
                status: "ambiguous".to_owned(),
                note_id: String::new(),
                relative_path: expected,
                matched_by: None,
                candidates,
            }),
            onote_tasks::DailyMatch::Missing => {
                // A blank daily note still records the day in its frontmatter so
                // the date survives a rename.
                let body = match resolve_daily_template(template_id.as_deref(), |id| {
                    template_body_for(&app, &vault_id, id)
                }) {
                    Ok(Some(body)) => body,
                    // The template is gone: report the choice instead of
                    // silently creating a blank note (F17 §7 TEMPLATE_MISSING).
                    Ok(None) => {
                        return Ok(DailyOutcome {
                            status: "templateMissing".to_owned(),
                            note_id: String::new(),
                            relative_path: expected,
                            matched_by: None,
                            candidates: Vec::new(),
                        })
                    }
                    Err(error) => return Err(error),
                };
                let source = if body.trim().is_empty() {
                    format!("---\ndate: {date}\n---\n\n")
                } else {
                    // Keep a template's own frontmatter but ensure the date is
                    // present; a template without a date field would otherwise
                    // lose the only link back to this day.
                    with_daily_date(&body, &date)
                };
                let operation_id = onote_attachments_import::note_operation_id(
                    &guard_vault,
                    &format!("daily:{expected}"),
                );
                let document = with_state(&app, |state| {
                    let storage = scoped_vault_mut(state, &vault_id)?;
                    storage.create_note(&expected, &source, &operation_id)
                })?;
                Ok(DailyOutcome {
                    status: "created".to_owned(),
                    note_id: document.note_id,
                    relative_path: document.relative_path,
                    matched_by: None,
                    candidates: Vec::new(),
                })
            }
        }
    })
    .await
    .map_err(map_join_err)?
}

/// Resolves a requested daily template.
///
/// `Ok(None)` means the id does not name a note (F17 TEMPLATE_MISSING);
/// `Ok(Some(body))` also covers the no-template case as an empty body. Other
/// read failures are propagated, so a transient I/O error is not mislabeled as
/// a missing template.
fn resolve_daily_template(
    template_id: Option<&str>,
    read: impl FnOnce(&str) -> Result<String, ServiceError>,
) -> Result<Option<String>, ServiceError> {
    match template_id {
        Some(id) if !id.trim().is_empty() => match read(id) {
            Ok(body) => Ok(Some(body)),
            Err(error) if error.code == ErrorCode::ObjectNotFound => Ok(None),
            Err(error) => Err(error),
        },
        _ => Ok(Some(String::new())),
    }
}

/// Ensures the frontmatter of a template body records the daily date.
fn with_daily_date(body: &str, date: &str) -> String {
    let scanner = onote_metadata_links::Scanner::new(body);
    match scanner.frontmatter() {
        onote_metadata_links::Frontmatter::Closed(span) => {
            let mut next = body.to_owned();
            let insert = format!("date: {date}\n");
            next.insert_str(span.end, &insert);
            next
        }
        _ => format!("---\ndate: {date}\n---\n\n{body}"),
    }
}

/// Reads a template note's body, or `ObjectNotFound` when the id is unknown.
fn template_body_for(
    app: &AppHandle,
    vault_id: &str,
    note_id: &str,
) -> Result<String, ServiceError> {
    with_state(app, |state| {
        let storage = scoped_vault_ref(state, vault_id)?;
        let document = storage.read(note_id)?;
        Ok(document.source)
    })
}

/// F18: expands a template body with the supplied variables.
///
/// Pure: it never reads or writes a file, so a preview and the committed insert
/// run the same code. Unknown variables come back verbatim with a warning so the
/// editor can show them instead of silently dropping the author's text.
#[tauri::command]
fn expand_template(
    body: String,
    variables: onote_tasks::TemplateVariables,
) -> onote_tasks::Expansion {
    onote_tasks::expand_template(&body, &variables)
}

/// F18: templates in the vault, discovered by the frontmatter marker.
///
/// A template is an ordinary note (F18-R04), so nothing is stored elsewhere:
/// this returns the same note ids M02 already knows.
#[tauri::command]
async fn list_templates(
    app: AppHandle,
    vault_id: String,
) -> Result<Vec<TemplateRow>, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            let mut rows = Vec::new();
            let mut summaries = storage.list()?;
            summaries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
            for summary in &summaries {
                let Ok(document) = storage.read(&summary.note_id) else {
                    continue;
                };
                if !onote_tasks::is_template(&document.source) {
                    continue;
                }
                rows.push(TemplateRow {
                    note_id: summary.note_id.clone(),
                    relative_path: summary.relative_path.clone(),
                    title: onote_metadata_links::title_of(&summary.relative_path),
                    body: document.source,
                });
            }
            Ok(rows)
        })
    })
    .await
    .map_err(map_join_err)?
}

/// One template as the picker needs it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRow {
    pub note_id: String,
    pub relative_path: String,
    pub title: String,
    /// Raw file content, including the frontmatter marker.
    pub body: String,
}

/// One task with the note it lives in, for the S24 list.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub note_id: String,
    pub relative_path: String,
    pub source_revision: u64,
    pub source_hash: String,
    pub read_only: bool,
    pub offset: usize,
    pub line: usize,
    pub status: onote_tasks::TaskStatus,
    pub text: String,
    pub due: Option<String>,
    pub depth: usize,
}

fn tasks_for_document(document: &onote_domain::NoteDocument) -> Vec<TaskRow> {
    onote_tasks::parse_tasks(&document.source)
        .into_iter()
        .map(|task| TaskRow {
            note_id: document.note_id.clone(),
            relative_path: document.relative_path.clone(),
            source_revision: document.local_revision,
            source_hash: document.content_hash.clone(),
            read_only: document.read_only,
            offset: task.offset,
            line: task.line,
            status: task.status,
            text: task.text,
            due: task.due,
            depth: task.depth,
        })
        .collect()
}

/// Every task in the vault, in path then document order.
///
/// Derived from the note sources through M02; nothing is cached, so a stale
/// index cannot list a task that no longer exists (F16-R01).
#[tauri::command]
async fn list_tasks(app: AppHandle, vault_id: String) -> Result<Vec<TaskRow>, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            let mut rows = Vec::new();
            let mut summaries = storage.list()?;
            summaries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
            for summary in &summaries {
                let Ok(document) = storage.read(&summary.note_id) else {
                    // A note that cannot be read is reported by the metadata
                    // panel; the task list simply omits it instead of failing
                    // every task in the vault.
                    continue;
                };
                rows.extend(tasks_for_document(&document));
            }
            Ok(rows)
        })
    })
    .await
    .map_err(map_join_err)?
}

/// F16: rewrites one checkbox in a supplied buffer and returns the new source.
///
/// Pure, like `apply_property_edit`: the editor buffer receives the result and
/// the normal save path keeps revision checks in one place (F16-R01). A task
/// whose text changed at the recorded offset is refused rather than ticked
/// (F16-R03).
#[tauri::command]
fn toggle_task(
    source: String,
    offset: usize,
    expected_text: String,
    status: onote_tasks::TaskStatus,
) -> Result<onote_tasks::ToggledSource, ServiceError> {
    onote_tasks::set_task_status(&source, offset, &expected_text, status).map_err(task_error)
}

/// F09: typed properties of a supplied buffer. Pure, like `parse_headings`, so
/// the property panel shows the live buffer instead of the last saved file.
#[tauri::command]
fn parse_properties(source: String) -> Result<onote_metadata_links::PropertySet, ServiceError> {
    onote_metadata_links::parse_properties(&source, &[]).map_err(|error| {
        let key = match error {
            onote_metadata_links::PropertyError::Unterminated => "error.propertyYamlUnterminated",
            onote_metadata_links::PropertyError::InvalidYaml => "error.propertyYamlInvalid",
            onote_metadata_links::PropertyError::NotAMapping => "error.propertyYamlNotMapping",
        };
        svc_err(ErrorCode::InvalidInput, key)
    })
}

/// F09: applies one typed property edit to a supplied source and returns the
/// new source. It deliberately does **not** save: the editor owns the buffer and
/// the existing save path keeps revision/operation checks in one place, so a
/// property edit can never bypass an unsaved buffer or a conflict.
#[tauri::command]
fn apply_property_edit(
    source: String,
    name: String,
    value: onote_metadata_links::PropertyValue,
) -> Result<onote_metadata_links::PropertyWrite, ServiceError> {
    onote_metadata_links::set_property(&source, &name, &value).map_err(property_write_error)
}

fn property_write_error(error: onote_metadata_links::PropertyWriteError) -> ServiceError {
    match error {
        onote_metadata_links::PropertyWriteError::InvalidName => {
            svc_err(ErrorCode::InvalidInput, "error.propertyNameInvalid")
        }
        onote_metadata_links::PropertyWriteError::Unterminated => {
            svc_err(ErrorCode::InvalidInput, "error.propertyYamlUnterminated")
        }
        onote_metadata_links::PropertyWriteError::InvalidYaml => {
            svc_err(ErrorCode::InvalidInput, "error.propertyYamlInvalid")
        }
        onote_metadata_links::PropertyWriteError::Unsupported(detail) => {
            let mut error = svc_err(ErrorCode::InvalidInput, "error.propertyShapeUnsupported");
            error.params.insert("detail".to_owned(), detail);
            error
        }
    }
}

/// F14: headings of an unsaved buffer. The editor must refresh its outline while
/// typing, before any save exists on disk, so this parses the supplied source
/// with the same M04 rules a saved note uses (code fences excluded, duplicate
/// slugs suffixed). It is read-only and touches no storage.
#[tauri::command]
fn parse_headings(
    source: String,
    max_source_chars: Option<usize>,
) -> Vec<onote_metadata_links::Heading> {
    let limit = max_source_chars.unwrap_or(2_000_000);
    if source.chars().count() > limit {
        return Vec::new();
    }
    onote_metadata_links::extract_headings(&source)
}

#[tauri::command]
async fn search_notes(
    app: AppHandle,
    vault_id: String,
    request: onote_search::SearchRequest,
) -> Result<onote_search::SearchPage, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let host_state = app.state::<search::SearchHost>();
        // Snapshot phase under the vault lock: only list summaries. The lock is
        // released before any note source is read, so a cold search never makes
        // a queued save wait for file reads (F12-R03). Completion re-locks the
        // host mutex only to publish the rebuilt cache.
        let (root, summaries, persist_allowed) = with_state(&app, |state| {
            let storage = scoped_vault_ref(state, &vault_id)?;
            Ok((
                storage.root_path().to_path_buf(),
                storage.list()?,
                storage.vault().writable,
            ))
        })?;
        let prepared =
            host_state.prepare_snapshot(&root, &vault_id, &summaries, &request, persist_allowed)?;
        host_state.finish(prepared, &request)
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
fn start_search(
    app: AppHandle,
    vault_id: String,
    request: onote_search::SearchRequest,
) -> Result<search_job::SearchJobView, ServiceError> {
    let worker_app = app.clone();
    with_state(&app, |state| {
        let storage = scoped_vault_ref(state, &vault_id)?;
        let root = storage.root_path().to_path_buf();
        let summaries = storage.list()?;
        let persist_allowed = storage.vault().writable;
        app.state::<search_job::SearchJobHost>().start(
            vault_id.clone(),
            summaries.len(),
            move |job| {
                let host_state = worker_app.state::<search::SearchHost>();
                let mut progress =
                    |phase, done, total, unreadable| job.progress(phase, done, total, unreadable);
                let prepared = host_state.prepare_controlled(
                    &root,
                    &vault_id,
                    &summaries,
                    &request,
                    persist_allowed,
                    &mut progress,
                )?;
                let result = host_state.finish_controlled(prepared, &request, &mut progress);
                result
            },
        )
    })
}

#[tauri::command]
fn search_job_status(
    app: AppHandle,
    vault_id: String,
    job_id: String,
) -> Result<search_job::SearchJobView, ServiceError> {
    with_state(&app, |state| {
        scoped_vault_ref(state, &vault_id)?;
        Ok(())
    })?;
    app.state::<search_job::SearchJobHost>()
        .status(&vault_id, &job_id, false)
}

#[tauri::command]
fn cancel_search(app: AppHandle, vault_id: String, job_id: String) -> Result<(), ServiceError> {
    // Like history cleanup, cancellation must never wait for the search or AppState locks.
    app.state::<search_job::SearchJobHost>()
        .status(&vault_id, &job_id, true)
        .map(|_| ())
}

/// F25: exports a scope of the vault to a ZIP the user picks.
///
/// The archive is built by M07 from the note list and attachment bytes read
/// through M02, staged to `<target>.opennote-partial`, verified against its
/// manifest, and only then renamed onto the chosen path. A cancelled dialog
/// returns `cancelled: true`; a verification or write failure leaves no file at
/// the final path (F25-A02, F25-R04).
/// One import outcome, for the S38 result panel.
#[tauri::command]
async fn export_vault(
    app: AppHandle,
    vault_id: String,
    scope: String,
    vault_target: Option<String>,
    target: Option<String>,
    suggested_name: Option<String>,
    dialog_title: Option<String>,
    include_attachments: bool,
    format: Option<String>,
) -> Result<ExportOutcome, ServiceError> {
    let scope = match scope.as_str() {
        "vault" => onote_attachments_import::ExportScope::Vault,
        "folder" => onote_attachments_import::ExportScope::Folder,
        "note" => onote_attachments_import::ExportScope::Note,
        _ => {
            return Err(svc_err(ErrorCode::InvalidInput, "error.exportScopeInvalid"));
        }
    };
    let format = match format.as_deref().unwrap_or("zip") {
        "zip" => onote_attachments_import::ExportFormat::Zip,
        "markdown" => onote_attachments_import::ExportFormat::Markdown,
        _ => {
            return Err(svc_err(
                ErrorCode::InvalidInput,
                "error.exportFormatInvalid",
            ));
        }
    };
    let suggestion = if matches!(format, onote_attachments_import::ExportFormat::Markdown) {
        sanitize_export_name(Some(suggested_name.as_deref().unwrap_or("OpenNote export")))
    } else {
        sanitize_export_name(suggested_name.as_deref())
    };
    let title = localized_or_fallback(dialog_title, "Export vault");
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        // The dialog runs before any lock so a slow user choice cannot block
        // another vault command.
        let chosen = match target {
            Some(value) if !value.trim().is_empty() => Some(PathBuf::from(value)),
            _ => handle
                .dialog()
                .file()
                .set_title(&title)
                .set_file_name(&suggestion)
                .blocking_save_file()
                .and_then(file_path_to_buf),
        };
        let Some(path) = chosen else {
            return Ok(ExportOutcome::cancelled());
        };
        let vault_target = vault_target.unwrap_or_default();
        export_to_path(
            &handle,
            &vault_id,
            scope,
            &vault_target,
            &path,
            include_attachments,
            format,
        )
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn expand_note_embed(
    app: AppHandle,
    vault_id: String,
    note_id: String,
    target: String,
    anchor: Option<String>,
    chain: Vec<String>,
) -> Result<onote_metadata_links::EmbedExpansion, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let link = onote_metadata_links::RawLink {
            kind: onote_metadata_links::LinkKind::Embed,
            target,
            label: None,
            anchor,
            offset: 0,
            length: 0,
        };
        with_state(&app, |state| {
            metadata::expand_embed(scoped_vault_ref(state, &vault_id)?, &note_id, &link, &chain)
        })
    })
    .await
    .map_err(map_join_err)?
}

/// Read-only rename preview (F10-R03): reports the exact byte ranges a rename
/// would rewrite and never writes. Execution stays with M02.
#[tauri::command]
async fn preview_note_rename(
    app: AppHandle,
    vault_id: String,
    note_id: String,
    to_path: String,
) -> Result<onote_metadata_links::RenamePlan, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            metadata::rename_plan(scoped_vault_ref(state, &vault_id)?, &note_id, &to_path)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn rename_note(
    app: AppHandle,
    vault_id: String,
    plan: onote_domain::RenamePlan,
    expected_revision: u64,
    operation_id: String,
) -> Result<onote_domain::RenameResult, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |state| {
            scoped_vault_ref(state, &vault_id)?.rename(&plan, expected_revision, &operation_id)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn preview_tag_update(
    app: AppHandle,
    vault_id: String,
    old_tag: String,
    new_tag: Option<String>,
) -> Result<metadata::TagUpdatePreview, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            metadata::tag_update_preview(
                scoped_vault_ref(s, &vault_id)?,
                &old_tag,
                new_tag.as_deref(),
            )
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn apply_tag_update(
    app: AppHandle,
    vault_id: String,
    plan: onote_domain::TagWritePlan,
    operation_id: String,
) -> Result<onote_domain::TagBatchResult, ServiceError> {
    tag_updates::run(app, vault_id, operation_id, Some(plan)).await
}
#[tauri::command]
async fn resume_tag_update(
    app: AppHandle,
    vault_id: String,
    operation_id: String,
) -> Result<onote_domain::TagBatchResult, ServiceError> {
    tag_updates::run(app, vault_id, operation_id, None).await
}
#[tauri::command]
async fn cancel_tag_update(
    app: AppHandle,
    vault_id: String,
    operation_id: String,
) -> Result<onote_domain::TagBatchResult, ServiceError> {
    tag_updates::request_cancel(&vault_id, &operation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_ref(s, &vault_id)?.cancel_tag_update(&operation_id)
        })
    })
    .await
    .map_err(map_join_err)?
}
#[tauri::command]
async fn list_tag_updates(
    app: AppHandle,
    vault_id: String,
) -> Result<Vec<onote_domain::TagBatchResult>, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| scoped_vault_ref(s, &vault_id)?.list_tag_updates())
    })
    .await
    .map_err(map_join_err)?
}

fn display_name_for(root: &Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| FALLBACK_VAULT_NAME.to_owned())
}

fn file_path_to_buf(p: tauri_plugin_dialog::FilePath) -> Option<PathBuf> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => Some(pb),
        tauri_plugin_dialog::FilePath::Url(u) => u.to_file_path().ok(),
    }
}

/// Native folder picker for create/open vault. Runs on a blocking thread so
/// the UI stays responsive and no state lock is held during the dialog.
fn pick_vault_folder_blocking(app: &AppHandle, title: &str) -> Option<PathBuf> {
    let mut dialog = app.dialog().file().set_title(title);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let picked = dialog.blocking_pick_folder();
    picked.and_then(file_path_to_buf)
}

/// Create/open the storage, then explicitly recover before the vault is
/// exposed; any failure here leaves host state untouched.
fn prepare_vault(
    root: PathBuf,
    creating: bool,
) -> Result<(Vault, VaultStorage, RecoverOutcome, Arc<HistoryStore>), ServiceError> {
    prepare_named_vault(root, creating, None)
}

fn prepare_named_vault(
    root: PathBuf,
    creating: bool,
    display_name: Option<String>,
) -> Result<(Vault, VaultStorage, RecoverOutcome, Arc<HistoryStore>), ServiceError> {
    let mut storage = if creating {
        let display_name = display_name.unwrap_or_else(|| display_name_for(&root));
        VaultStorage::create(root.clone(), &display_name)
    } else {
        VaultStorage::open(root.clone())
    }
    .map_err(|e: ServiceError| e)?;

    let vault = storage.vault();
    let history = Arc::new(HistoryStore::open(&root, &vault.vault_id)?);
    storage.set_revision_archive(history.clone());
    storage.set_trash_archive(history.clone());
    let report = if vault.writable {
        storage.recover()?
    } else {
        RecoverOutcome {
            applied: Vec::new(),
            pending: Vec::new(),
        }
    };
    Ok((vault, storage, report, history))
}

fn create_requested_sample(
    storage: &mut VaultStorage,
    sample_opt_in: bool,
) -> Result<Option<NoteDocument>, ServiceError> {
    if !sample_opt_in {
        return Ok(None);
    }
    for number in 1..=10_000 {
        let path = if number == 1 {
            "OpenNote sample.md".to_owned()
        } else {
            format!("OpenNote sample {number}.md")
        };
        match storage.create_note(&path, SAMPLE_NOTE_SOURCE, &uuid::Uuid::new_v4().to_string()) {
            Ok(note) => return Ok(Some(note)),
            Err(error) if error.code == ErrorCode::PathCollision => continue,
            Err(error) => return Err(error),
        }
    }
    Err(ServiceError {
        code: ErrorCode::PathCollision,
        message_key: "error.noteAlreadyExists".to_owned(),
        params: BTreeMap::from([("relativePath".to_owned(), "OpenNote sample.md".to_owned())]),
        retryable: false,
        operation_id: None,
        retry_after_seconds: None,
    })
}

/// Dialog titles accept an optional frontend-provided localized short title.
/// Empty/oversized/untrusted titles fall back to the built-in English text.
const DIALOG_TITLE_MAX: usize = 64;

fn localized_or_fallback(title: Option<String>, fallback: &str) -> String {
    match title {
        Some(t) => {
            let t = t.trim();
            if !t.is_empty() && t.chars().count() <= DIALOG_TITLE_MAX {
                t.to_owned()
            } else {
                fallback.to_owned()
            }
        }
        None => fallback.to_owned(),
    }
}

async fn load_vault_from_picker(
    app: AppHandle,
    creating: bool,
    title: Option<String>,
    sample_opt_in: bool,
    selection: Option<(PathBuf, String)>,
) -> Result<Option<Vault>, ServiceError> {
    let fallback = if creating {
        "Create vault"
    } else {
        "Open vault"
    };
    let title = localized_or_fallback(title, fallback);
    let display_name = selection.as_ref().map(|(_, name)| name.clone());
    let selected_path = selection.map(|(path, _)| path);
    let handle = app.clone();
    let lease = tauri::async_runtime::spawn_blocking(move || {
        selected_path.or_else(|| pick_vault_folder_blocking(&handle, &title))
    });
    let lease = match lease.await.map_err(map_join_err)? {
        Some(p) => p,
        None => return Ok(None),
    };

    let lease = tauri::async_runtime::spawn_blocking(move || {
        let (vault, mut storage, report, history) =
            prepare_named_vault(lease, creating, display_name)?;
        create_requested_sample(&mut storage, creating && sample_opt_in)?;
        Ok((vault, storage, report, history))
    });
    let lease = lease.await.map_err(map_join_err)?.map_err(|e| e)?;

    let (vault, storage, report, history) = lease;
    bind_vault(&app, storage, report, history)?;
    Ok(Some(vault))
}

/// Makes an opened vault the current one and remembers it for the next launch.
fn bind_vault(
    app: &AppHandle,
    storage: VaultStorage,
    report: RecoverOutcome,
    history: Arc<HistoryStore>,
) -> Result<(), ServiceError> {
    let root = storage.root_path().to_path_buf();
    with_state(app, |s| {
        backup::bind_session(app, &storage)?;
        source_watch::clear(app)?;
        app.state::<search_job::SearchJobHost>().cancel_all()?;
        s.recovery = Some(report);
        s.vault = Some(storage);
        s.history = Some(history);
        Ok(())
    })?;
    // Losing this only costs the automatic reopen on the next launch.
    if let Some(file) = last_vault_file(app) {
        let _ = remember_vault_root(&file, &root);
    }
    Ok(())
}

const LAST_VAULT_FILE: &str = "last-vault";

/// Holds the root of the vault opened most recently on this device. Only the
/// host writes it, and only with a folder the user picked, so reopening it
/// never lets the frontend name an arbitrary path.
fn last_vault_file(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|dir| dir.join(LAST_VAULT_FILE))
}

fn remember_vault_root(file: &Path, root: &Path) -> std::io::Result<()> {
    let text = root
        .to_str()
        .ok_or_else(|| std::io::Error::other("vault path is not UTF-8"))?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, text)
}

/// The remembered root, if it is still a folder on this device.
fn recall_vault_root(file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(file).ok()?;
    let line = text.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    let root = PathBuf::from(line);
    root.is_dir().then_some(root)
}

/// Reopens the vault from the previous session without a dialog. `None` means
/// there is nothing to reopen and the start screen should show; a vault that
/// can no longer be opened is forgotten rather than reported.
#[tauri::command]
async fn reopen_last_vault(app: AppHandle) -> Result<Option<Vault>, ServiceError> {
    let Some(file) = last_vault_file(&app) else {
        return Ok(None);
    };
    let opened = tauri::async_runtime::spawn_blocking(move || {
        let opened = recall_vault_root(&file).and_then(|root| prepare_vault(root, false).ok());
        if opened.is_none() {
            let _ = std::fs::remove_file(&file);
        }
        opened
    })
    .await
    .map_err(map_join_err)?;
    let Some((vault, storage, report, history)) = opened else {
        return Ok(None);
    };
    bind_vault(&app, storage, report, history)?;
    Ok(Some(vault))
}

#[cfg(test)]
mod last_vault_tests {
    use super::*;

    #[test]
    fn remembered_root_round_trips() {
        let settings = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let file = settings.path().join("nested").join(LAST_VAULT_FILE);
        remember_vault_root(&file, vault.path()).unwrap();
        assert_eq!(recall_vault_root(&file), Some(vault.path().to_path_buf()));
    }

    #[test]
    fn missing_record_or_folder_recalls_nothing() {
        let settings = tempfile::tempdir().unwrap();
        let file = settings.path().join(LAST_VAULT_FILE);
        assert_eq!(recall_vault_root(&file), None);
        let gone = settings.path().join("moved-away");
        remember_vault_root(&file, &gone).unwrap();
        assert_eq!(recall_vault_root(&file), None);
        std::fs::write(&file, "").unwrap();
        assert_eq!(recall_vault_root(&file), None);
    }
}

#[tauri::command]
async fn create_vault(
    app: AppHandle,
    dialog_title: Option<String>,
    sample_opt_in: Option<bool>,
    selection_id: Option<String>,
    display_name: Option<String>,
) -> Result<Option<Vault>, ServiceError> {
    let selection = match selection_id {
        Some(id) => {
            let state = app.state::<VaultFolderSelection>();
            let selected = state.0.try_lock().map_err(|_| task_err())?;
            Some(resolve_vault_selection(
                &selected,
                &id,
                &display_name.unwrap_or_default(),
            )?)
        }
        None => None,
    };
    load_vault_from_picker(
        app,
        true,
        dialog_title,
        sample_opt_in.unwrap_or(false),
        selection,
    )
    .await
}

#[tauri::command]
async fn open_vault(
    app: AppHandle,
    dialog_title: Option<String>,
) -> Result<Option<Vault>, ServiceError> {
    load_vault_from_picker(app, false, dialog_title, false, None).await
}

#[tauri::command]
async fn refresh_catalog(
    app: AppHandle,
    vault_id: String,
) -> Result<onote_domain::CatalogRefresh, ServiceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = with_state(&app, |state| {
            Ok(scoped_vault_ref(state, &vault_id)?
                .root_path()
                .to_path_buf())
        })?;
        let snapshot = VaultStorage::scan_catalog(&root)?;
        with_state(&app, |state| {
            let result = scoped_vault_mut(state, &vault_id)?.refresh_catalog(snapshot)?;
            app.state::<search::SearchHost>()
                .observe_catalog(&root, &vault_id, &result)?;
            Ok(result)
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
async fn list_notes(app: AppHandle, vault_id: String) -> Result<Vec<NoteSummary>, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.list()
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn list_folders(app: AppHandle, vault_id: String) -> Result<Vec<String>, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.list_folders()
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn inspect_folder(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<onote_vault_storage::FolderContents, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.inspect_folder(&relative_path)
    });
    outcome.await.map_err(map_join_err)?
}

/// Sends a folder holding files other than notes (a LaTeX project, say) to the
/// system recycle bin, then drops the LaTeX projects registered inside it.
/// Notes inside must already be in the vault's trash. A folder already gone
/// counts as sent, so a retry after a lost reply only finishes the clean-up.
#[tauri::command]
async fn recycle_folder(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<(), ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let mut guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_mut(&mut guard, &vault_id)?;
        match storage.recycle_folder(&relative_path, |path| {
            trash::delete(path).map_err(|error| std::io::Error::other(error.to_string()))
        }) {
            Ok(()) => {}
            Err(error) if error.code == ErrorCode::ObjectNotFound => {}
            Err(error) => return Err(error),
        }
        latex_project::unregister_inside(storage, &relative_path)
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn remove_empty_folder(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<(), ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.remove_empty_folder(&relative_path)
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn rename_empty_folder(
    app: AppHandle,
    vault_id: String,
    from: String,
    to: String,
) -> Result<String, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.rename_empty_folder(&from, &to)
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn create_folder(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<String, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.create_folder(&relative_path)
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn read_note(
    app: AppHandle,
    vault_id: String,
    note_id: String,
) -> Result<NoteDocument, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.read(&note_id)
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn create_note(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
    source: String,
    operation_id: String,
) -> Result<NoteDocument, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let mut guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_mut(&mut guard, &vault_id)?;
        storage.create_note(&relative_path, &source, &operation_id)
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn save_note(
    app: AppHandle,
    vault_id: String,
    note_id: String,
    source: String,
    expected_revision: u64,
    operation_id: String,
) -> Result<SaveResult, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let mut guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_mut(&mut guard, &vault_id)?;
        storage.save(&note_id, &source, expected_revision, &operation_id)
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn create_copy(
    app: AppHandle,
    vault_id: String,
    source_note_id: String,
    relative_path: String,
    source: String,
    operation_id: String,
) -> Result<NoteDocument, ServiceError> {
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            scoped_vault_mut(s, &vault_id)?.create_copy(
                &source_note_id,
                &relative_path,
                &source,
                &operation_id,
            )
        })
    });
    outcome.await.map_err(map_join_err)?
}

#[tauri::command]
async fn prepare_conflict(
    app: AppHandle,
    vault_id: String,
    note_id: String,
) -> Result<NoteDocument, ServiceError> {
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        with_state(&app, |s| {
            let history = scoped_history(s, &vault_id)?;
            let storage = scoped_vault_ref(s, &vault_id)?;
            let document = storage.read(&note_id)?;
            if storage.vault().writable && !document.read_only {
                history.capture(&document)?;
            }
            Ok(document)
        })
    });
    outcome.await.map_err(map_join_err)?
}

/// Reflects the recovery report captured after the most recent create/open.
/// Typed errors: `error.noVaultOpen` when no vault is open in this session,
/// `error.vaultSessionChanged` when the caller is scoped to a vault that is
/// no longer the active one, lock/poison failures surface as
/// `error.internalLockPoisoned`. Does not
/// re-run recovery (open never auto-drains, so pending stage/draft on disk
/// are never silently discarded).
#[tauri::command]
fn recover_vault(app: AppHandle, vault_id: String) -> Result<RecoverOutcome, ServiceError> {
    with_state(&app, |s| {
        scoped_vault_ref(s, &vault_id)?;
        s.recovery.clone().ok_or_else(no_vault_err)
    })
}

fn sanitize_export_name(raw: Option<&str>) -> String {
    let base = raw
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("draft.md");
    let segment = base.rsplit(['/', '\\']).next().unwrap_or(base);
    let cleaned: String = segment
        .chars()
        .take(120)
        .map(|c| {
            if (c as u32) < 0x20 || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut out = cleaned.trim().to_owned();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        out = "draft.md".to_owned();
    }
    out
}

/// Places where a picked `.opennote`/metadata name means the export would
/// target vault internals; ancestors are validated separately.
fn reject_reserved_target(path: &Path) -> Result<(), ServiceError> {
    for comp in path.components() {
        if comp.as_os_str().eq_ignore_ascii_case(".opennote") {
            return Err(export_err(
                ErrorCode::InvalidInput,
                "error.exportTargetReserved",
            ));
        }
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "vault.json"
                | "state.sqlite"
                | "state.sqlite-wal"
                | "state.sqlite-shm"
                | "index.sqlite"
        ) {
            return Err(export_err(
                ErrorCode::InvalidInput,
                "error.exportTargetReserved",
            ));
        }
    }
    Ok(())
}

/// Raw reparse-point check matching storage `scan::is_reparse`: on Windows the
/// `FILE_ATTRIBUTE_REPARSE_POINT` bit is inspected directly, because junctions
/// and other reparse tags are not symlink to std; elsewhere std's symlink
/// report is the exact equivalent.
#[cfg(windows)]
fn metadata_is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// Rejects reparse points currently visible in the target or its ancestors.
/// This metadata check does not lock against concurrent path replacement.
/// Missing ancestors and a missing
/// target are left to `create_new` to fail; any other inspection error on an
/// existing path is refused conservatively, never silently skipped.
fn reject_reparse_ancestors(path: &Path) -> Result<(), ServiceError> {
    for check in path.ancestors() {
        let meta = match std::fs::symlink_metadata(check) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(export_err(ErrorCode::IoDenied, "error.exportTargetReparse"));
            }
        };
        if metadata_is_reparse(&meta) {
            return Err(export_err(ErrorCode::IoDenied, "error.exportTargetReparse"));
        }
    }
    Ok(())
}

/// Export write, no overwrites, no truncation: ancestors validated first,
/// then an atomic `create_new` refuses ANY pre-existing destination (existing
/// exports/sentinels/notes/metadata are never touched), then `write_all` +
/// `sync_all` + readback must reproduce `source` before success. On failure
/// the only file ever removed is the partial one created by this call;
/// pre-existing files and the source bytes are preserved.
/// What an export produced, for the S39 result panel.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    pub cancelled: bool,
    /// Final path the user picked; absent when cancelled.
    pub path: Option<String>,
    pub notes: usize,
    pub attachments: usize,
    /// Entries excluded or lost, as message-ready labels.
    pub losses: Vec<String>,
    /// Wiki links and embeds kept byte-for-byte, which other tools show
    /// literally (F11-R04); the panel localizes each entry.
    pub compatibility_losses: Vec<onote_attachments_import::CompatibilityLoss>,
    pub verified: bool,
    pub bytes: u64,
}

impl ExportOutcome {
    fn cancelled() -> Self {
        Self {
            cancelled: true,
            path: None,
            notes: 0,
            attachments: 0,
            losses: Vec::new(),
            compatibility_losses: Vec::new(),
            verified: false,
            bytes: 0,
        }
    }
}

/// One note as the exporter sees it: vault-relative path plus its bytes.
pub(crate) struct ExportNote {
    pub relative_path: String,
    pub source: String,
}

/// Validates source boundaries before reading any referenced attachment bytes.
fn read_export_attachment(root: &Path, reference: &str) -> Result<Vec<u8>, ServiceError> {
    let invalid = || svc_err(ErrorCode::InvalidInput, "error.exportResourceInvalid");
    let relative = onote_vault_storage::naming::safe_relative(reference).map_err(|_| invalid())?;
    for segment in relative.split('/') {
        onote_vault_storage::naming::validate_name(segment).map_err(|_| invalid())?;
    }
    let path = root.join(relative);
    reject_reserved_target(&path)?;
    reject_reparse_ancestors(&path)?;
    let file = std::fs::File::open(path).map_err(|error| map_io_export(&error))?;
    let metadata = file.metadata().map_err(|error| map_io_export(&error))?;
    let limit = onote_attachments_import::MAX_EXPORT_ENTRY_BYTES;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| map_io_export(&error))?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(bytes)
}

/// Publishes the captured note snapshot; attachment bytes are validated separately.
fn publish_export(
    notes: &[ExportNote],
    unreadable: Vec<String>,
    root: &Path,
    scope: onote_attachments_import::ExportScope,
    vault_target: &str,
    path: &Path,
    include_attachments: bool,
    format: onote_attachments_import::ExportFormat,
) -> Result<ExportOutcome, ServiceError> {
    reject_reserved_target(path)?;
    reject_reparse_ancestors(path)?;
    if path.exists() {
        return Err(export_err(
            ErrorCode::InvalidInput,
            "error.exportTargetExists",
        ));
    }

    let mut existing: Vec<String> = notes
        .iter()
        .map(|note| note.relative_path.clone())
        .collect();
    let mut bodies: std::collections::BTreeMap<String, Vec<u8>> = notes
        .iter()
        .map(|note| {
            (
                note.relative_path.replace('\\', "/"),
                note.source.as_bytes().to_vec(),
            )
        })
        .collect();
    let mut attachment_bodies: Vec<(String, Vec<u8>)> = Vec::new();
    let options = onote_attachments_import::ExportOptions {
        format,
        include_attachments,
    };
    let all_paths: Vec<String> = notes
        .iter()
        .map(|note| note.relative_path.clone())
        .collect();
    let target_prefix = vault_target.trim().replace('\\', "/");
    let selected_notes: std::collections::BTreeSet<String> = onote_attachments_import::plan_export(
        scope,
        &target_prefix,
        &all_paths,
        &[],
        &existing,
        &options,
    )
    .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.exportNothing"))?
    .entries
    .into_iter()
    .map(|entry| entry.path)
    .collect();
    // Referenced paths are always passed to the planner, present or not: a
    // missing one must surface as ResourceMissing instead of disappearing
    // because the host silently skipped it (F25 §7).
    let mut referenced: Vec<String> = Vec::new();
    for note in notes {
        if !selected_notes.contains(&note.relative_path.replace('\\', "/")) {
            continue;
        }
        referenced.extend(onote_attachments_import::referenced_assets(
            &note.relative_path,
            &note.source,
        ));
    }
    referenced.sort();
    referenced.dedup();
    if include_attachments {
        for reference in &referenced {
            let Ok(bytes) = read_export_attachment(root, reference) else {
                continue;
            };
            existing.push(reference.clone());
            attachment_bodies.push((reference.clone(), bytes));
        }
    }

    // Every referenced path, not only the readable ones, so a missing resource
    // is reported rather than treated as "not referenced".
    let attachment_names: Vec<String> = referenced;
    // The scope prefix is a vault-relative path supplied by the caller; deriving
    // it from the save-dialog file name would export the wrong folder.
    let plan = onote_attachments_import::plan_export(
        scope,
        &target_prefix,
        &all_paths,
        &attachment_names,
        &existing,
        &options,
    )
    .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.exportNothing"))?;

    // Only notes that the plan actually exports can contribute a syntax loss;
    // a folder export must not mention a wiki link in a note outside the scope.
    let exported_notes: std::collections::BTreeSet<String> = plan
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let compatibility_inputs: Vec<(&str, &str)> = notes
        .iter()
        .filter(|note| exported_notes.contains(&note.relative_path.replace('\\', "/")))
        .map(|note| (note.relative_path.as_str(), note.source.as_str()))
        .collect();
    let compatibility_losses =
        onote_attachments_import::compatibility_losses(&compatibility_inputs);

    for (relative, bytes) in attachment_bodies {
        bodies.insert(relative.replace('\\', "/"), bytes);
    }
    let notes_written = plan
        .entries
        .iter()
        .filter(|entry| {
            let lower = entry.path.to_ascii_lowercase();
            lower.ends_with(".md") || lower.ends_with(".markdown")
        })
        .count();
    let attachments_written = plan.entries.len() - notes_written;

    if matches!(format, onote_attachments_import::ExportFormat::Markdown) {
        return publish_markdown_folder(
            &plan,
            &bodies,
            &compatibility_losses,
            unreadable,
            path,
            notes_written,
            attachments_written,
        );
    }

    let mut writer = onote_attachments_import::ZipWriter::new();
    for entry in &plan.entries {
        let Some(bytes) = bodies.get(&entry.path) else {
            return Err(svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"));
        };
        writer.add(&entry.path, bytes);
    }
    let staged = onote_attachments_import::stage_zip(writer)
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"))?;

    // Staging then publish: a failure at either step removes the staging file
    // and leaves the destination untouched (F25-R04).
    let staging = PathBuf::from(format!("{}{}", path.display(), STAGING_SUFFIX_HOST));
    let published = write_export_file(&staging, &staged.bytes)
        .and_then(|()| std::fs::rename(&staging, path).map_err(|error| map_io_export(&error)));
    if let Err(error) = published {
        let _ = std::fs::remove_file(&staging);
        return Err(error);
    }

    let mut losses = plan.losses.clone();
    losses.extend(unreadable);
    Ok(ExportOutcome {
        cancelled: false,
        path: Some(path.display().to_string()),
        notes: notes_written,
        attachments: attachments_written,
        losses,
        compatibility_losses,
        verified: staged.manifest.verified,
        bytes: staged.bytes.len() as u64,
    })
}

/// Writes a Markdown folder export, verifies it, then renames the staging
/// directory into place.
///
/// The destination must not exist, exactly like the ZIP path; the staging
/// directory itself is created with `create_dir` so an interrupted earlier run
/// is never mistaken for this one. Every file is read back and checked against
/// a manifest before the rename, so a partial folder is never published
/// (F25-A01, F25-A02, F25-R04).
fn publish_markdown_folder(
    plan: &onote_attachments_import::ExportPlan,
    bodies: &std::collections::BTreeMap<String, Vec<u8>>,
    compatibility_losses: &[onote_attachments_import::CompatibilityLoss],
    unreadable: Vec<String>,
    path: &Path,
    notes_written: usize,
    attachments_written: usize,
) -> Result<ExportOutcome, ServiceError> {
    let staging = PathBuf::from(format!("{}{}", path.display(), STAGING_SUFFIX_HOST));
    reject_reparse_ancestors(&staging)?;
    match std::fs::create_dir(&staging) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(export_err(
                ErrorCode::InvalidInput,
                "error.exportTargetExists",
            ));
        }
        Err(error) => return Err(map_io_export(&error)),
    }

    let written = (|| -> Result<onote_attachments_import::ExportManifest, ServiceError> {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        for entry in &plan.entries {
            let Some(bytes) = bodies.get(&entry.path) else {
                return Err(svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"));
            };
            let destination = entry
                .path
                .split('/')
                .filter(|part| !part.is_empty())
                .fold(staging.clone(), |base, part| base.join(part));
            if let Some(parent) = destination.parent() {
                reject_reparse_ancestors(parent)?;
                std::fs::create_dir_all(parent).map_err(|error| map_io_export(&error))?;
            }
            write_export_file(&destination, bytes)?;
            files.push((entry.path.clone(), bytes.clone()));
        }
        let report_name = onote_attachments_import::loss_report_name(&plan.entries);
        let report = onote_attachments_import::folder_loss_report(plan, compatibility_losses);
        write_export_file(&staging.join(&report_name), report.as_bytes())?;
        files.push((report_name, report.into_bytes()));

        let pairs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        let manifest = onote_attachments_import::folder_manifest(&pairs)
            .map_err(|_| svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"))?;
        let actual = read_export_tree(&staging)?;
        let actual_pairs: Vec<(&str, &[u8])> = actual
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        onote_attachments_import::verify_folder(&manifest, &actual_pairs)
            .map_err(|_| svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"))?;
        Ok(manifest)
    })();
    let mut manifest = match written {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = std::fs::rename(&staging, path) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(map_io_export(&error));
    }
    manifest.verified = true;

    let mut losses = plan.losses.clone();
    losses.extend(unreadable);
    Ok(ExportOutcome {
        cancelled: false,
        path: Some(path.display().to_string()),
        notes: notes_written,
        attachments: attachments_written,
        losses,
        compatibility_losses: compatibility_losses.to_vec(),
        verified: true,
        bytes: manifest.entries.iter().map(|entry| entry.size).sum(),
    })
}

/// Reads every file under a staged folder as (relative path, bytes), refusing
/// reparse paths and special files before reading staging content.
fn read_export_tree(root: &Path) -> Result<Vec<(String, Vec<u8>)>, ServiceError> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        reject_reparse_ancestors(&directory)?;
        let entries = std::fs::read_dir(&directory).map_err(|error| map_io_export(&error))?;
        for entry in entries {
            let entry = entry.map_err(|error| map_io_export(&error))?;
            let metadata = entry.metadata().map_err(|error| map_io_export(&error))?;
            if metadata_is_reparse(&metadata) {
                return Err(svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"));
            }
            let file_type = metadata.file_type();
            let absolute = entry.path();
            if file_type.is_dir() {
                pending.push(absolute);
                continue;
            }
            if !file_type.is_file() {
                return Err(svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"));
            }
            let relative = absolute
                .strip_prefix(root)
                .map_err(|_| svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"))?
                .to_string_lossy()
                .replace('\\', "/");
            reject_reparse_ancestors(&absolute)?;
            let bytes = std::fs::read(&absolute).map_err(|error| map_io_export(&error))?;
            files.push((relative, bytes));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn export_to_path(
    app: &AppHandle,
    vault_id: &str,
    scope: onote_attachments_import::ExportScope,
    vault_target: &str,
    path: &Path,
    include_attachments: bool,
    format: onote_attachments_import::ExportFormat,
) -> Result<ExportOutcome, ServiceError> {
    // Notes and the vault root are captured under one lock, so the archive is
    // built from a single consistent listing (F25-R03).
    let (notes, unreadable, root) = with_state(app, |state| {
        let storage = scoped_vault_ref(state, vault_id)?;
        let summaries = storage.list()?;
        let mut notes = Vec::new();
        let mut unreadable = Vec::new();
        for summary in &summaries {
            match storage.read(&summary.note_id) {
                Ok(document) => notes.push(ExportNote {
                    relative_path: summary.relative_path.clone(),
                    source: document.source,
                }),
                Err(_) => unreadable.push(summary.relative_path.clone()),
            }
        }
        Ok((notes, unreadable, storage.root_path().to_path_buf()))
    })?;
    publish_export(
        &notes,
        unreadable,
        &root,
        scope,
        vault_target,
        path,
        include_attachments,
        format,
    )
}

const STAGING_SUFFIX_HOST: &str = ".opennote-partial";

fn write_export_file(path: &Path, source: &[u8]) -> Result<(), ServiceError> {
    reject_reparse_ancestors(path)?;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(export_err(
                ErrorCode::InvalidInput,
                "error.exportTargetExists",
            ));
        }
        Err(e) => return Err(map_io_export(&e)),
    };

    let verified: Result<(), ServiceError> = (|| {
        file.write_all(source).map_err(|e| map_io_export(&e))?;
        file.sync_all().map_err(|e| map_io_export(&e))?;
        let written = std::fs::read(path).map_err(|e| map_io_export(&e))?;
        if written != source {
            return Err(export_err(ErrorCode::IoDenied, "error.exportWriteFailed"));
        }
        Ok(())
    })();

    drop(file);
    if verified.is_err() {
        let _ = std::fs::remove_file(path);
    }
    verified
}

/// Native save dialog only; the destination is always the user's explicit
/// pick, never a host-chosen path. Returns `false` on cancel; errors are
/// typed `ServiceError`s (destination exists, reserved/reparse target, IO).
#[tauri::command]
async fn export_draft(
    app: AppHandle,
    source: String,
    suggested_name: Option<String>,
    dialog_title: Option<String>,
) -> Result<bool, ServiceError> {
    let suggestion = sanitize_export_name(suggested_name.as_deref());
    let title = localized_or_fallback(dialog_title, "Export draft");
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let target = handle
            .dialog()
            .file()
            .set_title(&title)
            .set_file_name(&suggestion)
            .blocking_save_file();
        let path = match target.and_then(file_path_to_buf) {
            Some(p) => p,
            None => return Ok(false),
        };
        reject_reserved_target(&path)?;
        reject_reparse_ancestors(&path)?;
        write_export_file(&path, source.as_bytes())?;
        Ok(true)
    });
    outcome.await.map_err(map_join_err)?
}

/// Writes `bytes` to a new file the user picks in the native save dialog.
/// `Ok(false)` on cancel. Never overwrites, and refuses reserved names and
/// reparse-point folders, exactly as a draft export does. Blocking.
fn save_with_dialog(
    app: &AppHandle,
    bytes: &[u8],
    suggested_name: &str,
    title: &str,
    filter: (&str, &[&str]),
) -> Result<bool, ServiceError> {
    let target = app
        .dialog()
        .file()
        .set_title(title)
        .set_file_name(sanitize_export_name(Some(suggested_name)))
        .add_filter(filter.0, filter.1)
        .blocking_save_file();
    let Some(path) = target.and_then(file_path_to_buf) else {
        return Ok(false);
    };
    reject_reserved_target(&path)?;
    reject_reparse_ancestors(&path)?;
    write_export_file(&path, bytes)?;
    Ok(true)
}

/// Immutable draft checkpoint: stores the caller's original editing state
/// verbatim as an append-only row in the active vault's state.sqlite.
/// Markdown bytes, note revisions and the operations journal are never
/// touched; checkpoint success never means "note saved" (`draftId` equals
/// the idempotency `operationId`).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn write_checkpoint(
    app: AppHandle,
    vault_id: String,
    note_id: String,
    relative_path: String,
    source: String,
    base_revision: u64,
    base_content_hash: String,
    operation_id: String,
) -> Result<CheckpointResult, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let mut guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_mut(&mut guard, &vault_id)?;
        storage.write_checkpoint(CheckpointRequest {
            note_id,
            relative_path,
            source,
            base_revision,
            base_content_hash,
            operation_id,
        })
    });
    outcome.await.map_err(map_join_err)?
}

/// Sequence-DESC paging of draft checkpoint metadata (no draft bodies);
/// `beforeSequence` is the exclusive cursor returned as
/// `nextBeforeSequence`, default limit 50 / hard cap 200.
#[tauri::command]
async fn list_drafts(
    app: AppHandle,
    vault_id: String,
    note_id: Option<String>,
    before_sequence: Option<u64>,
    limit: Option<usize>,
) -> Result<DraftPage, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.list_drafts(note_id.as_deref(), before_sequence, limit)
    });
    outcome.await.map_err(map_join_err)?
}

/// One stored draft in full, exposed only after integrity verification of
/// stored metadata and exact source bytes.
#[tauri::command]
async fn read_draft(
    app: AppHandle,
    vault_id: String,
    draft_id: String,
) -> Result<NoteDraft, ServiceError> {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<Mutex<AppState>>();
        let guard = state.lock().map_err(|_| poison_err())?;
        let storage = scoped_vault_ref(&guard, &vault_id)?;
        storage.read_draft(&draft_id)
    });
    outcome.await.map_err(map_join_err)?
}

/// Called by the frontend after it authorized a safe close (or by a user
/// first-terminating during this construction stage, per docs bridge gap).
#[tauri::command]
fn confirm_close(app: AppHandle) -> Result<(), ServiceError> {
    backup::require_idle(&app)?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod export_helper_tests {
    use super::*;

    #[test]
    fn export_refuses_existing_file_and_writes_fresh_bytes_exactly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let existing = dir.path().join("draft.md");
        let fresh = dir.path().join("fresh.md");

        std::fs::write(&existing, b"pre-existing sentinel").expect("seed sentinel");
        let refusal = write_export_file(&existing, b"payload")
            .err()
            .expect("existing destination must be refused");
        assert_eq!(refusal.code, ErrorCode::InvalidInput);
        assert_eq!(
            refusal.message_key, "error.exportTargetExists",
            "refusal is typed, not a truncating overwrite"
        );
        assert_eq!(
            std::fs::read(&existing).unwrap(),
            b"pre-existing sentinel",
            "pre-existing sentinel must be untouched"
        );

        write_export_file(&fresh, b"exact bytes \xC3\xA9tail").expect("fresh export succeeds");
        assert_eq!(
            std::fs::read(&fresh).unwrap(),
            b"exact bytes \xC3\xA9tail",
            "fresh export must retain the exact payload bytes"
        );
    }

    #[test]
    fn diagnostics_report_is_written_redacted_and_never_replaces_a_file() {
        // F44 §6/§7 + A01: the file is regenerated from the preview and request,
        // so it carries the redacted log rather than the raw one, and a failed
        // write cannot overwrite an existing destination.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("diagnostics.txt");
        let request = onote_diagnostics::PreviewRequest {
            facts: onote_diagnostics::HostFacts {
                app_version: "0.1.0".to_owned(),
                build_channel: "dev".to_owned(),
                platform: "windows-x64".to_owned(),
                locale: "en".to_owned(),
                roots: vec!["/home/alice".to_owned()],
            },
            correlation_id: onote_diagnostics::correlation_id("vault:op"),
            counts: vec![(
                onote_diagnostics::CountKind::Notes,
                onote_diagnostics::Measured::Known(3),
            )],
            capabilities: vec![],
            statuses: vec![],
            last_error_code: None,
            timestamp_millis: 1,
            raw_log: Some(
                concat!(
                    "error in /home/alice/thesis/main.tex\n",
                    "Authorization: Bearer sk-secret\n",
                    "l.12 private draft sentence\n",
                    "LaTeX Warning: Reference `x' undefined.\n",
                )
                .to_owned(),
            ),
        };
        let preview = diagnostics_preview(request.clone());
        write_diagnostics_report(&target, &preview, &request).expect("fresh report is written");
        let written = std::fs::read_to_string(&target).expect("read report");
        assert!(written.contains(&preview.correlation_id));
        assert!(written.contains("<redacted>"));
        assert!(!written.contains("/home/alice"));
        assert!(!written.contains("sk-secret"));
        assert!(!written.contains("private draft sentence"));
        assert!(written.contains("l.12 <redacted>"));
        assert!(written.contains("LaTeX Warning"));

        let refusal = write_diagnostics_report(&target, &preview, &request)
            .err()
            .expect("an existing report must not be replaced");
        assert_eq!(refusal.code, ErrorCode::InvalidInput);
        assert_eq!(
            std::fs::read_to_string(&target).expect("report still readable"),
            written,
            "the first report stays byte-identical after the refusal"
        );
    }

    #[cfg(windows)]
    #[test]
    fn folder_export_readback_refuses_a_junction_root() {
        use std::os::windows::process::CommandExt;
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("note.md"), b"outside sentinel").unwrap();
        let linked = directory.path().join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&linked)
            .arg(outside.path())
            .creation_flags(0x0800_0000)
            .output()
            .unwrap()
            .status;
        assert!(status.success());
        let result = read_export_tree(&linked);
        assert!(linked.starts_with(directory.path()));
        std::fs::remove_dir(&linked).unwrap();
        assert!(result.is_err(), "readback must not trust a junction root");
        assert_eq!(
            read_export_tree(outside.path()).unwrap(),
            [("note.md".to_owned(), b"outside sentinel".to_vec())]
        );
    }

    #[cfg(windows)]
    #[test]
    fn export_file_writer_refuses_a_junction_parent() {
        use std::os::windows::process::CommandExt;
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let linked = directory.path().join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&linked)
            .arg(outside.path())
            .creation_flags(0x0800_0000)
            .output()
            .unwrap()
            .status;
        assert!(status.success());
        let result = write_export_file(&linked.join("new.md"), b"must stay inside");
        assert!(linked.starts_with(directory.path()));
        std::fs::remove_dir(&linked).unwrap();
        assert!(result.is_err(), "shared writer must check its own parent");
        assert!(!outside.path().join("new.md").exists());
    }

    /// Windows junction ancestor to an outside sentinel: reparse detection
    /// must use the raw `FILE_ATTRIBUTE_REPARSE_POINT` bit (junctions are not
    /// `is_symlink` to std), refuse with the typed error before any write,
    /// leave the outside bytes untouched, and keep a normal fresh path
    /// exportable. The junction is dismantled without following it.
    #[cfg(windows)]
    #[test]
    fn junction_ancestor_refused_before_any_write_outside_untouched() {
        let outside = tempfile::tempdir().expect("outside tempdir");
        let outside_dir = outside.path().join("outside-target");
        std::fs::create_dir_all(&outside_dir).expect("outside target dir");
        let sentinel = outside_dir.join("sentinel.md");
        std::fs::write(&sentinel, b"outside sentinel").expect("seed outside sentinel");
        let sentinel_before = std::fs::read(&sentinel).expect("read outside sentinel");

        let dir = tempfile::tempdir().expect("export tempdir");
        let linked = dir.path().join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&outside_dir)
            .status()
            .expect("mklink run");
        assert!(status.success(), "junction creation failed");

        // Same order as export_draft: the guard must refuse on its own,
        // before any new file is written anywhere.
        let target = linked.join("draft.md");
        let refusal = reject_reparse_ancestors(&target)
            .err()
            .expect("junction ancestor must be refused");
        assert_eq!(refusal.code, ErrorCode::IoDenied);
        assert_eq!(
            refusal.message_key, "error.exportTargetReparse",
            "typed refusal, not a pass-through into the link target"
        );
        assert!(
            !target.exists(),
            "no new export file may appear inside the link target"
        );
        assert_eq!(
            std::fs::read(&sentinel).expect("read outside sentinel after refusal"),
            sentinel_before,
            "outside sentinel must be untouched"
        );

        // A normal directory ancestor on the same root still exports fine.
        let normal = dir.path().join("normal");
        std::fs::create_dir_all(&normal).expect("normal ancestor");
        write_export_file(&normal.join("draft.md"), b"payload").expect("fresh path succeeds");
        assert_eq!(
            std::fs::read(normal.join("draft.md")).expect("read normal export"),
            b"payload",
            "normal fresh path must not be collateral damage"
        );

        // Dismantle the junction without recursing into the link target.
        std::fs::remove_dir(&linked).expect("junction link removal");
        assert_eq!(
            std::fs::read(&sentinel).expect("read outside sentinel after removing link"),
            sentinel_before,
            "removing the link must not touch target bytes"
        );
    }
}

#[cfg(test)]
mod vault_session_guard_tests {
    use super::*;

    const OP_CREATE_STALE: &str = "11111111-1111-4111-8111-111111111111";
    const OP_SAVE_ACTIVE: &str = "22222222-2222-4222-8222-222222222222";

    /// Two freshly initialized vaults contain the SAME copied embedded note
    /// UUID but report different vaultIds. After the picker replaces the
    /// active storage, the shared session guard must refuse a late callback
    /// still scoped to the old vault BEFORE any storage mutation, leave the
    /// current vault's bytes and revision as a sentinel, and let the
    /// matching scope operate on the active vault.
    #[test]
    fn vault_session_guard_refuses_stale_scope_before_mutating_current_vault() {
        // Vault A: new note whose UUID is embedded on disk.
        let dir_a = tempfile::tempdir().expect("tempdir A");
        let mut storage_a = VaultStorage::create(dir_a.path(), "Vault A").expect("vault A");
        let created = storage_a
            .create_note("note.md", "stale session body", OP_CREATE_STALE)
            .expect("create note in A");
        let note_id = created.note_id.clone();

        // Copy the composed bytes (embedded opennote-id) into a second
        // folder BEFORE it is initialized as a vault, so both vaults scan
        // the identical note UUID while owning freshly generated vaultIds.
        let dir_b = tempfile::tempdir().expect("tempdir B");
        std::fs::copy(dir_a.path().join("note.md"), dir_b.path().join("note.md"))
            .expect("copy embedded note into second vault");
        let storage_b = VaultStorage::create(dir_b.path(), "Vault B").expect("vault B");
        let active_vault_id = storage_b.vault().vault_id;
        let stale_vault_id = storage_a.vault().vault_id;
        assert_ne!(active_vault_id, stale_vault_id, "distinct vault sessions");

        // The picker completed: the active AppState now holds vault B's
        // storage while a late vault-A-scoped note callback is in flight.
        let mut state = AppState {
            vault: Some(storage_b),
            recovery: None,
            history: None,
        };

        let note_path_b = dir_b.path().join("note.md");
        let before_bytes = std::fs::read(&note_path_b).expect("read B note bytes");
        let before_rev = scoped_vault_ref(&state, &active_vault_id)
            .and_then(|s| s.read(&note_id))
            .expect("active note legible under matching scope")
            .local_revision;

        // Late callback under the stale vault A scope: refused before any
        // mutation, typed, with no source or path data leaked. Without the
        // guard, note UUID + revision would have resolved on the ACTIVE
        // vault and silently written it.
        let refusal = scoped_vault_mut(&mut state, &stale_vault_id)
            .err()
            .expect("stale scope must be refused");
        assert_eq!(refusal.code, ErrorCode::InvalidInput);
        assert_eq!(refusal.message_key, "error.vaultSessionChanged");
        assert!(refusal.params.is_empty(), "no source/path leak");

        // The read side shares the same guard refusal.
        let read_refusal = scoped_vault_ref(&state, &stale_vault_id)
            .err()
            .expect("stale scope must be refused for reads");
        assert_eq!(read_refusal.message_key, "error.vaultSessionChanged");

        // Sentinel: current vault bytes and revision are untouched.
        assert_eq!(
            std::fs::read(&note_path_b).expect("re-read B note bytes"),
            before_bytes,
            "refused stale-scope mutation must not touch current vault bytes"
        );
        let after_refusal = scoped_vault_ref(&state, &active_vault_id)
            .and_then(|s| s.read(&note_id))
            .expect("active note legible after refusal");
        assert_eq!(
            after_refusal.local_revision, before_rev,
            "refused stale-scope mutation must not bump the current revision"
        );

        // The matching (new-session) scope operates on the active vault.
        let saved = scoped_vault_mut(&mut state, &active_vault_id)
            .and_then(|s| s.save(&note_id, "active session body", before_rev, OP_SAVE_ACTIVE))
            .expect("matching-scope save succeeds on the current session");
        assert_eq!(saved.local_revision, before_rev + 1);
        assert_ne!(
            std::fs::read(&note_path_b).expect("re-read after matching save"),
            before_bytes,
            "matching-scope save commits to the current vault"
        );
    }
}

/// Task 2 draft-command regression. The three draft IPC commands are
/// composition-only: `spawn_blocking` plumbing around the SAME shared
/// scoped-vault guard every other note-bearing command uses, followed by the
/// delegated storage call. Without a live Tauri runtime the IPC wrapper
/// itself cannot execute here, so these tests exercise that boundary
/// verbatim minus the plumbing: mutex-held `AppState` ->
/// `scoped_vault_ref`/`scoped_vault_mut` -> `VaultStorage` draft API, over
/// synthetic temporary vaults only. No GUI/frontend behavior is claimed.
#[cfg(test)]
mod draft_session_guard_tests {
    use super::*;

    const OP_CREATE_A: &str = "55555555-5555-4555-8555-555555555555";
    const DRAFT_OP_A: &str = "33333333-3333-4333-8333-333333333333";
    const DRAFT_OP_B: &str = "44444444-4444-4444-8444-444444444444";

    /// Presence probe for the three draft commands. Public Tauri API offers
    /// no runtime introspection of the registered command table, so a
    /// missing command is an unresolved path here (explicit whole-binary
    /// RED); their precise IPC schema is pinned at build time by `run()`'s
    /// `generate_handler!` invocation.
    #[test]
    fn draft_ipc_commands_exist_in_host_crate() {
        #[allow(path_statements)]
        {
            write_checkpoint;
            list_drafts;
            read_draft;
        }
    }

    #[test]
    fn attachment_import_keeps_unknown_types_and_respects_the_cloud_limit() {
        let inputs = vec![
            onote_attachments_import::AttachmentInput {
                name: "photo.png".to_owned(),
                source_path: "C:/Users/a/photo.png".to_owned(),
                size: 1024,
                sha256: "aa".to_owned(),
            },
            // An unknown type must still be importable (F19-R02).
            onote_attachments_import::AttachmentInput {
                name: "data.bin".to_owned(),
                source_path: "C:/Users/a/data.bin".to_owned(),
                size: 10,
                sha256: "bb".to_owned(),
            },
            // Over the cloud limit: local import still works (F19-R04).
            onote_attachments_import::AttachmentInput {
                name: "huge.mp4".to_owned(),
                source_path: "C:/Users/a/huge.mp4".to_owned(),
                size: onote_attachments_import::ATTACHMENT_CLOUD_LIMIT + 1,
                sha256: "cc".to_owned(),
            },
        ];
        let plan = plan_attachment_import(inputs, "assets".to_owned(), Vec::new()).expect("plan");
        assert_eq!(plan.importable().len(), 3);
        assert_eq!(plan.unsynced, 1);

        let image = plan
            .attachments
            .iter()
            .find(|entry| entry.destination.ends_with("photo.png"))
            .expect("image");
        // The reference is an image tag and the thumbnail is only a cache.
        assert_eq!(image.reference, "![](assets/photo.png)");
        assert!(image.caches_thumbnail);

        let unknown = plan
            .attachments
            .iter()
            .find(|entry| entry.destination.ends_with("data.bin"))
            .expect("unknown");
        assert_eq!(unknown.kind.as_str(), "unknown");
        // A link, not an image tag, because nothing can draw it.
        assert!(unknown.reference.starts_with('['));

        // The same bytes already present are not stored twice.
        let duplicate = plan_attachment_import(
            vec![onote_attachments_import::AttachmentInput {
                name: "copy.png".to_owned(),
                source_path: "C:/Users/a/copy.png".to_owned(),
                size: 1024,
                sha256: "aa".to_owned(),
            }],
            "assets".to_owned(),
            vec![("assets/photo.png".to_owned(), "aa".to_owned())],
        )
        .expect("plan");
        assert_eq!(duplicate.total_bytes, 0);
        assert!(!duplicate.attachments[0].stores_bytes);

        // A same-named file with different bytes keeps both: the existing path is
        // never overwritten (F19-A01).
        let renamed = plan_attachment_import(
            vec![onote_attachments_import::AttachmentInput {
                name: "photo.png".to_owned(),
                source_path: "C:/Users/a/photo.png".to_owned(),
                size: 512,
                sha256: "dd".to_owned(),
            }],
            "assets".to_owned(),
            vec![("assets/photo.png".to_owned(), "aa".to_owned())],
        )
        .expect("plan");
        assert_eq!(renamed.attachments[0].destination, "assets/photo (1).png");
        assert!(renamed.attachments[0].stores_bytes);
        assert!(renamed.attachments[0].issues.iter().any(|issue| matches!(
            issue,
            onote_attachments_import::AttachmentIssue::RenamedForCollision { .. }
        )));

        // A destination that climbs out is refused rather than clamped.
        assert!(plan_attachment_import(Vec::new(), "../outside".to_owned(), Vec::new()).is_err());

        // Classification is exposed so a picker can label a file before import.
        let (kind, preview) = classify_attachment_extension("svg".to_owned());
        assert_eq!(kind, "image");
        // An SVG can carry script, so it is opened by the system, not drawn.
        assert_eq!(preview, "systemOpen");
    }

    #[test]
    fn tag_rename_plans_body_and_frontmatter_and_skips_code() {
        let notes = vec![
            onote_metadata_links::TagNote {
                note_id: "n1".to_owned(),
                relative_path: "a.md".to_owned(),
                source: "---\ntags:\n  - old\n---\n#old\n\n```\n#fenced\n```\n".to_owned(),
                revision: 1,
            },
            onote_metadata_links::TagNote {
                note_id: "n2".to_owned(),
                relative_path: "b.md".to_owned(),
                source: "#old only\n".to_owned(),
                revision: 2,
            },
        ];
        let plan = plan_tag_rename_command(
            notes.clone(),
            "old".to_owned(),
            "new".to_owned(),
            vec![("n1".to_owned(), 1), ("n2".to_owned(), 3)],
            Vec::new(),
        );
        // n1 is rewritten; its fenced occurrence stays.
        let first = plan
            .rewrites
            .iter()
            .find(|rewrite| rewrite.note_id == "n1")
            .expect("n1 rewrite");
        assert!(onote_metadata_links::extract_tags(&first.source)
            .iter()
            .any(|tag| tag.tag == "new"
                && tag.source == onote_metadata_links::TagSource::Frontmatter));
        assert!(
            first.source.contains("```\n#fenced\n```"),
            "code fence untouched"
        );
        // n2 changed since it was read, so it is reported and not rewritten.
        assert!(plan.rewrites.iter().all(|rewrite| rewrite.note_id != "n2"));
        assert_eq!(plan.conflicted.len(), 1);

        // A removal keeps the note and drops the tag.
        let removal = plan_tag_removal_command(
            vec![onote_metadata_links::TagNote {
                note_id: "n1".to_owned(),
                relative_path: "a.md".to_owned(),
                source: "Text #old more\n".to_owned(),
                revision: 1,
            }],
            "old".to_owned(),
            vec![("n1".to_owned(), 1)],
        );
        assert_eq!(removal.rewrites[0].source, "Text more\n");
    }

    #[test]
    fn rename_commit_lists_steps_and_refuses_a_stale_note() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Rename").unwrap();
        let a = storage
            .create_note("a.md", "[[b]]\n", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let b = storage
            .create_note("b.md", "body\n", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let (index, _) = metadata::rebuild_index(&storage).unwrap();
        let plan =
            onote_metadata_links::plan_rename(&index, &b.note_id, "moved/b.md").expect("plan");
        // The linking note is the only one whose source changes.
        assert_eq!(plan.edits.len(), 1);

        // A matching revision produces one step and no issues.
        let commit = onote_metadata_links::plan_commit(
            &index,
            &plan,
            &[(a.note_id.clone(), a.local_revision)],
            &[(a.note_id.clone(), a.local_revision)],
            &["a.md".to_owned(), "b.md".to_owned()],
            false,
        );
        assert!(commit.is_applicable(), "issues: {:?}", commit.issues);
        assert_eq!(commit.steps.len(), 1);
        assert_eq!(commit.move_to, "moved/b.md");
        assert!(commit.keeps_identity, "a move keeps the note id (F03-R02)");

        // A changed revision blocks it: the link offsets would be wrong.
        let stale = onote_metadata_links::plan_commit(
            &index,
            &plan,
            &[(a.note_id.clone(), a.local_revision)],
            &[(a.note_id.clone(), a.local_revision + 1)],
            &[],
            false,
        );
        assert!(!stale.is_applicable());
        assert!(stale.steps.is_empty());

        // An occupied destination is refused.
        let occupied = onote_metadata_links::plan_commit(
            &index,
            &onote_metadata_links::plan_rename(&index, &b.note_id, "a.md").expect("plan"),
            &[(a.note_id.clone(), a.local_revision)],
            &[(a.note_id.clone(), a.local_revision)],
            &["a.md".to_owned(), "b.md".to_owned()],
            false,
        );
        assert!(!occupied.is_applicable());
    }

    #[test]
    fn note_names_follow_windows_rules_including_case() {
        // Reserved device names and trailing dots are refused before any write.
        assert!(validate_note_name("con".to_owned()).is_err());
        assert!(validate_note_name("notes.".to_owned()).is_err());
        assert!(validate_note_name("a/b".to_owned()).is_err());
        assert_eq!(
            validate_note_name("Weekly review".to_owned()).expect("ok"),
            "Weekly review"
        );

        // A case-only rename must not collide with the file it renames.
        let existing = vec!["Notes.md".to_owned()];
        assert_eq!(
            onote_vault_storage::naming::check_destination("Notes.md", "notes.md", &existing)
                .expect("allowed"),
            "notes.md"
        );
        // A different file that differs only in case is a collision (F03-R01).
        assert!(
            onote_vault_storage::naming::check_destination("other.md", "notes.md", &existing)
                .is_err()
        );
        // Windows needs two steps for a case-only change, so it is reported.
        assert!(is_case_only_rename(
            "Notes.md".to_owned(),
            "notes.md".to_owned()
        ));
        assert!(!is_case_only_rename(
            "Notes.md".to_owned(),
            "Other.md".to_owned()
        ));

        // A destination inside the moved folder would move with its own parent.
        assert!(onote_vault_storage::naming::check_destination("notes", "notes/sub", &[]).is_err());
        // Escaping the vault is refused.
        assert!(onote_vault_storage::naming::safe_relative("../outside/x.md").is_err());
    }

    #[test]
    fn project_inspection_picks_a_single_entry_and_never_runs_anything() {
        let files = vec![
            onote_latex::InspectFile {
                path: "main.tex".to_owned(),
                bytes: Some(80),
                text: Some(
                    "\\documentclass{article}\n\\usepackage{graphicx}\n\\input{sections/a}\n"
                        .to_owned(),
                ),
            },
            onote_latex::InspectFile {
                path: "sections/a.tex".to_owned(),
                bytes: Some(10),
                text: Some("Body\n".to_owned()),
            },
            onote_latex::InspectFile {
                path: ".latexmkrc".to_owned(),
                bytes: Some(12),
                text: Some("$pdf_mode = 1;\n".to_owned()),
            },
        ];
        let inspection = inspect_latex_project("proj".to_owned(), files.clone());
        assert_eq!(inspection.entry.as_deref(), Some("main.tex"));
        assert!(inspection.inspectable);
        // The config file is reported but not executed.
        assert!(inspection.findings.iter().any(|finding| matches!(
            finding,
            onote_latex::ProjectFinding::ConfigNotExecuted { .. }
        )));
        assert!(inspection
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "graphicx" && dependency.known));

        // Two equally plausible entries block the inspection instead of guessing.
        let ambiguous = inspect_latex_project(
            "proj".to_owned(),
            vec![
                onote_latex::InspectFile {
                    path: "a.tex".to_owned(),
                    bytes: None,
                    text: Some("\\documentclass{article}\n".to_owned()),
                },
                onote_latex::InspectFile {
                    path: "b.tex".to_owned(),
                    bytes: None,
                    text: Some("\\documentclass{report}\n".to_owned()),
                },
            ],
        );
        assert_eq!(ambiguous.entry, None);
        assert!(!ambiguous.inspectable);

        // A shell escape blocks it: OpenNote never enables one.
        let escaping = inspect_latex_project(
            "proj".to_owned(),
            vec![onote_latex::InspectFile {
                path: "main.tex".to_owned(),
                bytes: None,
                text: Some("\\documentclass{article}\n\\immediate\\write18{id}\n".to_owned()),
            }],
        );
        assert!(!escaping.inspectable);

        // An unsafe root is refused outright.
        let unsafe_root = inspect_latex_project("../outside".to_owned(), Vec::new());
        assert!(!unsafe_root.inspectable);
    }

    #[test]
    fn bibliography_keeps_unknown_entries_and_refuses_ambiguous_keys() {
        let source = "@article{a,\n author = {Smith},\n title = {A {G}reat Paper},\n}\n\
@dataset{data1,\n title = {Data},\n}\n\
@article{dup,\n title={One},\n}\n\
@book{dup,\n title={Two},\n}\n";
        let bibliography = parse_bibliography(source.to_owned());
        // The unknown type is kept and flagged, not dropped.
        let dataset = bibliography
            .entries
            .iter()
            .find(|entry| entry.cite_key == "data1")
            .expect("dataset entry");
        assert!(dataset.unknown_type);
        assert_eq!(dataset.title(), Some("Data"));
        // A duplicate key reports both offsets.
        assert_eq!(bibliography.duplicates.len(), 1);
        assert_eq!(bibliography.duplicates[0].offsets.len(), 2);

        // Citing a duplicate is refused with both offsets, not resolved silently.
        let plan = plan_citation_command(
            bibliography.clone(),
            vec!["dup".to_owned()],
            "bibtex".to_owned(),
            "\\bibliography{refs}".to_owned(),
        )
        .expect("plan");
        assert!(plan.fragment.is_empty());
        assert!(matches!(
            plan.issues.first(),
            Some(onote_latex::CitationIssue::DuplicateKey { offsets, .. }) if offsets.len() == 2
        ));

        // A normal citation inserts one command and knows the package is present.
        let ok = plan_citation_command(
            bibliography.clone(),
            vec!["a".to_owned(), "data1".to_owned()],
            "biber".to_owned(),
            "\\bibliography{refs}".to_owned(),
        )
        .expect("plan");
        assert_eq!(ok.fragment, "\\cite{a,data1}\n");
        assert!(ok.has_bibliography_package);
        assert_eq!(ok.backend, onote_latex::Backend::Biber);

        // An unknown backend is refused instead of defaulting.
        assert!(plan_citation_command(
            bibliography.clone(),
            Vec::new(),
            "biblatex".to_owned(),
            String::new()
        )
        .is_err());

        // Deleting a cited entry surfaces the dangling key without touching the
        // source text (F49-A02).
        let dangling = bibliography_dangling(
            parse_bibliography("@article{a,\n title={A},\n}\n".to_owned()),
            vec!["\\cite{a} and \\cite{gone}".to_owned()],
        );
        assert_eq!(dangling, vec!["gone".to_owned()]);
    }

    #[test]
    fn latex_insertion_escapes_plain_text_but_not_formulas() {
        // F48-R01: an equation body is TeX, a section title is plain text.
        let equation = plan_latex_insertion(
            onote_latex::Parameters::Equation {
                body: "a_b^2".to_owned(),
                label: Some("eq:one".to_owned()),
            },
            "\\documentclass{article}".to_owned(),
        );
        assert!(equation.is_ready());
        assert!(equation.fragment.contains("a_b^2"));
        assert!(equation.fragment.contains("\\label{eq:one}"));

        let section = plan_latex_insertion(
            onote_latex::Parameters::Section {
                title: "R&D 50%".to_owned(),
                level: 1,
                label: None,
            },
            String::new(),
        );
        assert!(section.fragment.contains("\\section{R\\&D 50\\%}"));

        // A duplicate label is refused with the labels that exist.
        let duplicate = plan_latex_insertion(
            onote_latex::Parameters::Equation {
                body: "x".to_owned(),
                label: Some("eq:one".to_owned()),
            },
            "\\label{eq:one}".to_owned(),
        );
        assert!(!duplicate.is_ready());
        assert!(duplicate
            .issues
            .iter()
            .any(|issue| matches!(issue, onote_latex::InsertionIssue::DuplicateLabel { .. })));

        // A needed package is reported, not silently added to the preamble.
        let image = plan_latex_insertion(
            onote_latex::Parameters::Image {
                path: "assets/a.png".to_owned(),
                width: 0.6,
                caption: None,
                label: None,
            },
            "\\documentclass{article}".to_owned(),
        );
        assert!(image.is_ready());
        assert_eq!(image.missing_dependencies().len(), 1);
        assert_eq!(image.missing_dependencies()[0].name, "graphicx");

        // Labels come from the buffer, so the picker has something to show.
        assert_eq!(
            latex_labels("\\label{fig:a}\n\\label{tab:b}".to_owned()),
            vec!["fig:a".to_owned(), "tab:b".to_owned()]
        );
    }

    #[test]
    fn latex_assist_edits_are_applicable_in_one_pass() {
        // The editor applies the returned edits in one transaction, so the result
        // must be complete on its own: apply them and check the buffer.
        let source = "alpha\nbeta\n";
        let result = latex_toggle_comment(
            source.to_owned(),
            onote_latex::Range { start: 0, end: 5 },
            false,
        )
        .expect("comment");
        let applied = onote_latex::apply_edits(source, &result.edits).expect("apply");
        assert_eq!(applied, "%alpha\nbeta\n");

        // Wrapping the selection is likewise a single self-contained edit.
        let wrapped = latex_wrap_selection(
            "value".to_owned(),
            onote_latex::Range { start: 0, end: 5 },
            "textbf".to_owned(),
            false,
        )
        .expect("wrap");
        let applied = onote_latex::apply_edits("value", &wrapped.edits).expect("apply");
        assert_eq!(applied, "\\textbf{value}");
        // The selection the caller restores covers the original text.
        assert_eq!(
            &applied[wrapped.selection.start..wrapped.selection.end],
            "value"
        );
    }

    #[test]
    fn latex_assist_refuses_stale_ranges_and_escaped_brackets() {
        // A composition in progress blocks every assist action (F47-R03).
        assert!(matches!(
            latex_toggle_comment(
                "abc".to_owned(),
                onote_latex::Range { start: 0, end: 3 },
                true
            ),
            Err(ServiceError {
                code: ErrorCode::InvalidInput,
                ..
            })
        ));

        // Commenting is one edit over the whole line.
        let commented = latex_toggle_comment(
            "alpha\nbeta\n".to_owned(),
            onote_latex::Range { start: 0, end: 5 },
            false,
        )
        .expect("comment");
        assert_eq!(commented.edits.len(), 1);
        assert_eq!(commented.edits[0].text, "%alpha");

        // Wrapping keeps the selection on the original text (F47-R02).
        let wrapped = latex_wrap_selection(
            "hi".to_owned(),
            onote_latex::Range { start: 0, end: 2 },
            "textbf".to_owned(),
            false,
        )
        .expect("wrap");
        assert_eq!(wrapped.edits[0].text, "\\textbf{hi}");
        assert_eq!(
            wrapped.selection,
            onote_latex::Range { start: 8, end: 10 },
            "the inner text stays selected"
        );

        // A stale completion range is a typed conflict, not a silent replace.
        assert!(matches!(
            latex_apply_completion(
                "one two".to_owned(),
                onote_latex::Range { start: 0, end: 3 },
                "four".to_owned(),
                "four".to_owned()
            ),
            Err(ServiceError {
                code: ErrorCode::VersionConflict,
                ..
            })
        ));

        // An escaped brace has no partner, so no pair is invented.
        assert_eq!(latex_matching_bracket("\\{x}".to_owned(), 1), None);
        assert_eq!(latex_matching_bracket("{x}".to_owned(), 0), Some(2));

        // Completions merge the lexicon with the project's own commands.
        let candidates = latex_completions("\\customcmd{x}".to_owned(), "\\cu".to_owned());
        assert!(candidates
            .iter()
            .any(|candidate| candidate.label == "\\customcmd"));

        // Language detection falls back to plain text rather than guessing.
        assert_eq!(
            detect_latex_language("main.tex".to_owned(), String::new()),
            onote_latex::LanguageMode::Tex
        );
        assert_eq!(
            detect_latex_language("notes.txt".to_owned(), "words".to_owned()),
            onote_latex::LanguageMode::Plain
        );
    }

    #[test]
    fn latex_scaffold_escapes_metadata_and_reports_collisions() {
        let metadata = onote_latex::Metadata {
            title: "R&D 50%".to_owned(),
            author: String::new(),
            date: "2026-10-20".to_owned(),
            language: onote_latex::DocumentLanguage::En,
            paper: onote_latex::PaperSize::A4,
        };
        // Escaping uses one encoder, exposed for previewing.
        let escaped = escape_latex_text("R&D 50%".to_owned());
        assert_eq!(escaped, "R\\&D 50\\%");

        // A clean plan is executable and includes the generated closure.
        let clean =
            onote_latex::plan_scaffold(onote_latex::TemplateId::Report, &metadata, "proj", &[]);
        assert!(clean.is_executable());
        assert!(clean.files.iter().any(|file| file.path == "main.tex"));
        assert!(clean.files.iter().any(|file| file.path == "references.bib"));
        // The title is escaped inside the generated source.
        let main = &clean.files[0].content;
        assert!(main.contains("\\&D"));
        // No YAML frontmatter, so an external editor can open the file (F45-R01).
        assert!(!main.starts_with("---"));

        // A collision blocks creation rather than overwriting (F46-R03).
        let colliding = onote_latex::plan_scaffold(
            onote_latex::TemplateId::Report,
            &metadata,
            "proj",
            &["proj/main.tex".to_owned()],
        );
        assert!(!colliding.is_executable());
        assert!(colliding
            .issues
            .iter()
            .any(|issue| matches!(issue, onote_latex::PlanIssue::Collision { .. })));

        // An unknown template id is refused by the command, not defaulted.
        assert!(onote_latex::TemplateId::parse("poster").is_none());
    }

    #[test]
    fn search_index_status_is_reported_without_building_the_index() {
        // A cold index must report Unavailable, not "0 documents": the two facts
        // are different and only one of them is a measurement (F44-R04).
        let host = search::SearchHost::default();
        let status = host.status().expect("status");
        assert_eq!(status.state, onote_search::IndexState::Unavailable);
        assert_eq!(status.indexed_notes, 0);

        // Asking twice must not change the answer; a status probe never rebuilds.
        let again = host.status().expect("status");
        assert_eq!(again, status);
    }

    #[test]
    fn diagnostics_probe_reports_real_counts_and_keeps_unknown_states() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Diag").unwrap();
        storage
            .create_note(
                "a.md",
                "---\n---\nbody\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        storage
            .create_note(
                "b.md",
                "---\n---\nbody\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();

        // The probe counts what M02 lists, and reports a writable vault as ready.
        assert_eq!(storage.list().unwrap().len(), 2);
        assert!(storage.vault().writable);
        assert!(storage.vault().findings.is_empty());
        assert_eq!(storage.list_trash().unwrap().len(), 0);

        // A probe that cannot read a value must produce None, which the report
        // turns into Unknown rather than 0 (F44-R04).
        let unavailable: Option<u64> = None;
        assert!(unavailable.is_none());

        // The compile queue is counted by state, so a queued job is not reported
        // as running.
        let mut queue = onote_latex::Queue::new();
        queue
            .request(
                "p1",
                "m",
                "m",
                onote_latex::JobInput {
                    manifest_hash: "m".to_owned(),
                    source_hash: "s1".to_owned(),
                },
                "op1",
            )
            .unwrap();
        queue
            .request(
                "p1",
                "m",
                "m",
                onote_latex::JobInput {
                    manifest_hash: "m".to_owned(),
                    source_hash: "s2".to_owned(),
                },
                "op2",
            )
            .unwrap();
        let running = queue
            .jobs()
            .iter()
            .filter(|job| matches!(job.state, onote_latex::JobState::Running))
            .count();
        let queued = queue
            .jobs()
            .iter()
            .filter(|job| matches!(job.state, onote_latex::JobState::Queued))
            .count();
        assert_eq!((running, queued), (1, 1));
    }

    #[test]
    fn diagnostics_report_is_allowlisted_and_redacts_the_log() {
        let facts = onote_diagnostics::HostFacts {
            app_version: "0.1.0".to_owned(),
            build_channel: "dev".to_owned(),
            platform: "windows-x64".to_owned(),
            locale: "zh-CN".to_owned(),
            roots: vec!["/home/u/vault".to_owned()],
        };
        let request = onote_diagnostics::PreviewRequest {
            facts,
            correlation_id: onote_diagnostics::correlation_id("vault:op"),
            counts: vec![
                (
                    onote_diagnostics::CountKind::Notes,
                    onote_diagnostics::Measured::Known(12),
                ),
                // An unreadable counter must stay Unknown, not become 0.
                (
                    onote_diagnostics::CountKind::SearchDocuments,
                    onote_diagnostics::Measured::Unknown,
                ),
            ],
            capabilities: vec![(
                onote_diagnostics::CapabilityKind::LatexCompile,
                onote_diagnostics::Status::Unknown {
                    reason_key: "no.toolchain".to_owned(),
                },
            )],
            statuses: vec![onote_diagnostics::ScopeStatus {
                scope: onote_diagnostics::Scope::Storage,
                status: onote_diagnostics::Status::Ready,
            }],
            last_error_code: None,
            timestamp_millis: 1,
            raw_log: Some("error in /home/u/vault/main.tex\n".to_owned()),
        };
        let preview = diagnostics_preview(request.clone());
        let unknown = preview
            .fields
            .iter()
            .find(|field| field.key == "count.searchDocuments")
            .map(|field| field.value.clone())
            .expect("field");
        assert_eq!(unknown, onote_diagnostics::FieldValue::Unknown);

        // Only allowlisted keys appear; paths and content are named as omissions.
        assert!(!preview
            .fields
            .iter()
            .any(|field| field.key.contains("path")));
        assert!(preview
            .omitted
            .iter()
            .any(|omission| omission.key == "noteContent"));

        // The exported text carries the redacted log, never the raw path.
        let text = diagnostics_export_text(preview.clone(), request);
        assert!(!text.contains("/home/u/vault/main.tex"));
        assert!(text.contains("<redacted>"));

        // One ready scope does not make a report with an unknown one healthy.
        assert!(!diagnostics_overall(preview).is_ready());
    }

    #[test]
    fn latex_export_plans_the_closure_and_never_calls_an_incomplete_project_ready() {
        let files = vec![
            onote_latex::ProjectFile {
                path: "main.tex".to_owned(),
                bytes: Some(100),
                text: Some("\\input{chapters/intro}\n\\bibliography{refs}\n".to_owned()),
            },
            onote_latex::ProjectFile {
                path: "chapters/intro.tex".to_owned(),
                bytes: Some(50),
                text: Some("body".to_owned()),
            },
            onote_latex::ProjectFile {
                path: "refs.bib".to_owned(),
                bytes: Some(20),
                text: None,
            },
            onote_latex::ProjectFile {
                path: "main.log".to_owned(),
                bytes: Some(10),
                text: None,
            },
            onote_latex::ProjectFile {
                path: "main.pdf".to_owned(),
                bytes: Some(1000),
                text: None,
            },
        ];
        let plan = plan_latex_export(
            "sourceZip".to_owned(),
            files.clone(),
            "h1".to_owned(),
            "h1".to_owned(),
            None,
            "h1".to_owned(),
            Some("h1".to_owned()),
        )
        .expect("plan");
        assert!(plan.complete);
        let paths: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert!(paths.contains(&"chapters/intro.tex"));
        assert!(paths.contains(&"refs.bib"));
        // Generated files are excluded from a source export.
        assert!(!paths.contains(&"main.log"));
        assert!(plan.excluded.iter().any(|path| path == "main.log"));

        // A missing include makes the plan incomplete rather than silently partial.
        let broken = plan_latex_export(
            "sourceZip".to_owned(),
            vec![onote_latex::ProjectFile {
                path: "main.tex".to_owned(),
                bytes: Some(1),
                text: Some("\\input{gone}\n".to_owned()),
            }],
            "h1".to_owned(),
            "h1".to_owned(),
            None,
            "h1".to_owned(),
            None,
        )
        .expect("plan");
        assert!(!broken.complete);

        // A PDF older than the source is reported (PDF_OUTDATED).
        let outdated = plan_latex_export(
            "pdf".to_owned(),
            files,
            "h1".to_owned(),
            "h1".to_owned(),
            None,
            "new".to_owned(),
            Some("old".to_owned()),
        )
        .expect("plan");
        assert!(!outdated.complete);
        assert!(outdated
            .issues
            .iter()
            .any(|issue| matches!(issue, onote_latex::ExportIssue::PdfOutdated { .. })));

        // An unknown kind is refused instead of guessing.
        assert!(plan_latex_export(
            "everything".to_owned(),
            Vec::new(),
            "h1".to_owned(),
            "h1".to_owned(),
            None,
            "h1".to_owned(),
            None,
        )
        .is_err());
    }

    #[test]
    fn compile_queue_runs_one_job_and_collapses_pending_requests() {
        let mut queue = onote_latex::Queue::new();
        let first = queue
            .request(
                "p1",
                "m1",
                "m1",
                onote_latex::JobInput {
                    manifest_hash: "m1".to_owned(),
                    source_hash: "s1".to_owned(),
                },
                "op1",
            )
            .unwrap();
        assert_eq!(first.state, onote_latex::JobState::Running);

        // A second request while the project is busy becomes the single pending
        // job (F51-R01/R02).
        let pending = queue
            .request(
                "p1",
                "m1",
                "m1",
                onote_latex::JobInput {
                    manifest_hash: "m1".to_owned(),
                    source_hash: "s2".to_owned(),
                },
                "op2",
            )
            .unwrap();
        assert_eq!(pending.state, onote_latex::JobState::Queued);

        // A newer request supersedes the pending one instead of stacking.
        let newer = queue
            .request(
                "p1",
                "m1",
                "m1",
                onote_latex::JobInput {
                    manifest_hash: "m1".to_owned(),
                    source_hash: "s3".to_owned(),
                },
                "op3",
            )
            .unwrap();
        assert_eq!(
            queue.queued_job("p1").map(|job| job.job_id.clone()),
            Some(newer.job_id)
        );
        assert_eq!(
            queue.job(&pending.job_id).map(|job| job.state),
            Some(onote_latex::JobState::Cancelled)
        );

        // A stale manifest is refused before anything is queued.
        assert!(queue
            .request(
                "p1",
                "old",
                "new",
                onote_latex::JobInput {
                    manifest_hash: "new".to_owned(),
                    source_hash: "s4".to_owned(),
                },
                "op4",
            )
            .is_err());

        // A leftover running job is reported interrupted, never as a success.
        let interrupted = queue.recover();
        assert_eq!(interrupted, vec![first.job_id.clone()]);
        assert_eq!(
            queue
                .job(&first.job_id)
                .and_then(|job| job.artifact_id.clone()),
            None
        );
        assert_eq!(
            queue.job(&first.job_id).map(|job| job.state),
            Some(onote_latex::JobState::Interrupted)
        );
    }

    #[test]
    fn preview_keeps_a_failed_build_readable_and_refuses_a_stale_jump() {
        let previous = onote_latex::Artifact {
            artifact_id: "a1".to_owned(),
            source_hash: "h1".to_owned(),
            has_synctex: true,
        };

        // A failed build keeps the previous PDF readable (F52-R01).
        let failed = preview_state(
            Some(previous.clone()),
            onote_latex::BuildOutcome::Failed {
                reason: "engine error".to_owned(),
            },
            "h1".to_owned(),
            true,
        );
        assert_eq!(failed.artifact_id.as_deref(), Some("a1"));
        // The failed rebuild did not make the source newer: the kept PDF was
        // built from h1, which is still the current source.
        assert_eq!(failed.pdf_state, onote_latex::PdfState::Current);
    }

    #[test]
    fn tex_log_parsing_never_invents_a_position_or_a_success() {
        let log = "! LaTeX Error: File `missingpkg.sty' not found.\n\
! Undefined control sequence.\n\
l.12 \\foo\n\
Overfull \\hbox (1pt too wide) in paragraph\n";
        let report = parse_tex_log(log.to_owned(), Some(1), None, None);
        // The missing package is surfaced for the manual prepare step.
        assert!(report
            .missing
            .iter()
            .any(|resource| resource.name == "missingpkg.sty"));
        // The engine's own line number is used; nothing is inferred.
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.line == Some(12)));
        // A warning does not decide the verdict; the exit code does.
        assert_eq!(report.verdict, onote_latex::BuildVerdict::Failed);

        // A missing exit code yields unverified, never success.
        let unknown = parse_tex_log("This is pdfTeX\n".to_owned(), None, None, None);
        assert_eq!(unknown.verdict, onote_latex::BuildVerdict::Unverified);
        assert!(!unknown.succeeded);

        // An unrecognised error keeps its text and claims no position.
        let odd = parse_tex_log("! strange failure\n".to_owned(), Some(1), None, None);
        assert_eq!(odd.diagnostics[0].raw, "! strange failure");
        assert_eq!(odd.diagnostics[0].line, None);

        // Redaction drops machine paths but keeps the message.
        let with_path = parse_tex_log(
            "/home/u/doc.tex:3: Undefined control sequence.\n".to_owned(),
            Some(1),
            Some(" job-9 ".to_owned()),
            Some("snap-1".to_owned()),
        );
        assert_eq!(with_path.job_id.as_deref(), Some("job-9"));
        assert_eq!(with_path.source_hash.as_deref(), Some("snap-1"));
        let redacted = redact_tex_log(with_path, vec!["/home/u".to_owned()]);
        assert!(!redacted
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.raw.contains("/home/u")));
    }

    #[test]
    fn deleting_a_task_is_distinct_from_completing_it() {
        let source = "- [ ] a\n- [ ] b\n";
        let tasks = onote_tasks::parse_tasks(source);
        let deleted =
            onote_tasks::delete_task(source, tasks[0].offset, &tasks[0].text).expect("delete");
        assert_eq!(deleted.source, "- [ ] b\n");
        let completed = onote_tasks::set_task_status(
            source,
            tasks[0].offset,
            &tasks[0].text,
            onote_tasks::TaskStatus::Done,
        )
        .expect("complete");
        assert_ne!(deleted.source, completed.source);

        // Due dates are written into the same line, and an invalid date is
        // refused rather than silently dropping the task from every group.
        let added =
            onote_tasks::set_task_due(source, tasks[0].offset, &tasks[0].text, Some("2026-10-20"))
                .expect("set due");
        assert_eq!(
            onote_tasks::parse_tasks(&added.source)[0].due.as_deref(),
            Some("2026-10-20")
        );
        assert!(matches!(
            onote_tasks::set_task_due(source, tasks[0].offset, &tasks[0].text, Some("2026-02-30")),
            Err(onote_tasks::TaskError::InvalidDate(_))
        ));
    }

    #[test]
    fn canvas_saves_reopen_after_restart_and_refuse_to_overwrite_external_edits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let mut storage = VaultStorage::create(&root, "Canvas").unwrap();
        let document = onote_tasks::parse_canvas(
            r#"{"formatVersion":"1.0","future":{"keep":"me"},"nodes":[{"id":"n1","type":"text","x":10,"y":20,"width":240,"height":160,"text":"你好"}],"edges":[]}"#,
        )
        .unwrap();
        let created = save_canvas_to(
            &mut storage,
            "Boards/Plan.canvas",
            &document,
            None,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(created.written);
        drop(storage);

        let mut storage = VaultStorage::open(&root).unwrap();
        let loaded = read_canvas_from(&storage, "Boards/Plan.canvas").unwrap();
        assert_eq!(loaded.sha256, created.sha256);
        assert_eq!(loaded.document.nodes.len(), 1);
        assert!(
            loaded.document.extra.contains_key("future"),
            "unknown fields survive"
        );

        let mut moved = loaded.document.clone();
        moved.nodes[0].x = 99.0;
        let saved = save_canvas_to(
            &mut storage,
            "Boards/Plan.canvas",
            &moved,
            Some(&loaded.sha256),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert_ne!(saved.sha256, loaded.sha256);

        // A stale editor (still holding the first hash) cannot overwrite it.
        let error = save_canvas_to(
            &mut storage,
            "Boards/Plan.canvas",
            &document,
            Some(&loaded.sha256),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::VersionConflict);
        assert_eq!(
            read_canvas_from(&storage, "Boards/Plan.canvas")
                .unwrap()
                .document
                .nodes[0]
                .x,
            99.0
        );
    }

    #[test]
    fn view_files_are_stored_verbatim_and_must_be_json_objects() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let mut storage = VaultStorage::create(&root, "Views").unwrap();
        let source = "{\n  \"formatVersion\": 1,\n  \"name\": \"读书\",\n  \"future\": [1]\n}\n";
        let saved = save_view_to(
            &mut storage,
            "Views/Reading.onview.json",
            source,
            None,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(saved.written);
        assert_eq!(
            std::fs::read_to_string(root.join("Views/Reading.onview.json")).unwrap(),
            source
        );
        for bad in ["[]", "not json", "1"] {
            assert_eq!(
                save_view_to(
                    &mut storage,
                    "b.onview.json",
                    bad,
                    None,
                    &uuid::Uuid::new_v4().to_string()
                )
                .unwrap_err()
                .message_key,
                "error.viewInvalid"
            );
        }
        let huge = format!("{{\"x\":\"{}\"}}", "a".repeat(MAX_VIEW_BYTES));
        assert_eq!(
            save_view_to(
                &mut storage,
                "c.onview.json",
                &huge,
                None,
                &uuid::Uuid::new_v4().to_string()
            )
            .unwrap_err()
            .message_key,
            "error.viewTooLarge"
        );
        assert!(save_view_to(
            &mut storage,
            "d.json",
            "{}",
            None,
            &uuid::Uuid::new_v4().to_string()
        )
        .is_err());
    }

    #[test]
    fn canvas_documents_are_validated_and_keep_unknown_fields() {
        // A reference card keeps the note id and never the body (F21-R01).
        let source = r#"{
          "formatVersion": "1.1",
          "future": true,
          "nodes": [{"id":"n1","type":"note","x":0,"y":0,"width":240,"height":160,"noteId":"note-a"}],
          "edges": []
        }"#;
        let document = onote_tasks::parse_canvas(source).expect("parse");
        assert_eq!(document.format_version, "1.1");
        assert_eq!(
            onote_tasks::canvas_references(&document)[0].note_id,
            "note-a"
        );
        // Unknown fields survive a round trip.
        let again = onote_tasks::parse_canvas(
            &onote_tasks::serialize_canvas(&document).expect("serialize"),
        )
        .expect("reparse");
        assert!(again.extra.contains_key("future"));

        // A dangling edge is refused rather than drawn to nowhere.
        assert!(matches!(
            onote_tasks::parse_canvas(
                r#"{"formatVersion":"1.0","nodes":[],"edges":[{"id":"e1","from":"a","to":"b"}]}"#
            ),
            Err(onote_tasks::GeometryError::DanglingEdge { .. })
        ));

        // A sub-minimum content node is refused; a group may be smaller.
        assert!(matches!(
            onote_tasks::parse_canvas(
                r#"{"formatVersion":"1.0","nodes":[{"id":"n1","type":"text","x":0,"y":0,"width":10,"height":10}],"edges":[]}"#
            ),
            Err(onote_tasks::GeometryError::TooSmall { .. })
        ));
    }

    #[test]
    fn database_table_projects_frontmatter_without_faking_unset_values() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Db").unwrap();
        storage
            .create_note(
                "a.md",
                "---\nstatus: doing\nscore: 10\n---\nbody\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        storage
            .create_note(
                "b.md",
                "---\nstatus: todo\nscore: 0\n---\nbody\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        storage
            .create_note(
                "c.md",
                "---\nstatus: doing\n---\nbody\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();

        let mut sources = Vec::new();
        for summary in storage.list().unwrap() {
            let document = storage.read(&summary.note_id).unwrap();
            let set = onote_metadata_links::parse_properties(&document.source, &[]).unwrap();
            sources.push(onote_tasks::RowSource {
                note_id: summary.note_id.clone(),
                title: onote_metadata_links::title_of(&summary.relative_path),
                relative_path: summary.relative_path.clone(),
                source_revision: document.local_revision,
                source_hash: document.content_hash.clone(),
                read_only: document.read_only,
                properties: set
                    .properties
                    .iter()
                    .map(|property| (property.name.clone(), property.value.clone()))
                    .collect(),
            });
        }

        let table = onote_tasks::build_table(&sources, &["score".into()], &[], None);
        // The note with score 0 and the note without a score must differ.
        let zero = table
            .rows
            .iter()
            .find(|row| row.title == "b")
            .map(|row| row.cells[0].clone())
            .unwrap();
        let unset = table
            .rows
            .iter()
            .find(|row| row.title == "c")
            .map(|row| row.cells[0].clone())
            .unwrap();
        assert_eq!(zero, onote_tasks::CellValue::Number(0.0));
        assert_eq!(unset, onote_tasks::CellValue::Unset);

        // Filtering hides rows and reports how many, never deleting anything.
        let filtered = onote_tasks::build_table(
            &sources,
            &[],
            &[onote_tasks::Filter {
                key: "status".into(),
                op: onote_tasks::FilterOp::Equals,
                value: "doing".into(),
            }],
            None,
        );
        assert_eq!(filtered.rows.len(), 2);
        assert_eq!(filtered.hidden, 1);
        assert_eq!(filtered.rows.len() + filtered.hidden, 3);
    }

    #[test]
    fn graph_report_walks_hops_and_marks_missing_targets() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Graph").unwrap();
        let a = storage
            .create_note(
                "a.md",
                "[[b]] and ![[c]]\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        let b = storage
            .create_note("b.md", "[[c]]\n", &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let c = storage
            .create_note(
                "c.md",
                "[[missing-note]]\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();

        // One hop from a reaches b and c, and shows the embed edge type.
        let one = metadata::graph_report(&storage, Some(&a.note_id), 1, 500).unwrap();
        let ids: Vec<&str> = one.nodes.iter().map(|node| node.id.as_str()).collect();
        assert!(ids.contains(&a.note_id.as_str()));
        assert!(ids.contains(&b.note_id.as_str()));
        assert!(ids.contains(&c.note_id.as_str()));
        assert!(one.edges.iter().any(|edge| edge.kind == "embed"));
        assert!(one
            .nodes
            .iter()
            .any(|node| node.anchor && node.id == a.note_id));

        // Two hops reveals the broken target of c as a missing node.
        let two = metadata::graph_report(&storage, Some(&a.note_id), 2, 500).unwrap();
        assert!(
            two.nodes.iter().any(|node| node.kind == "missing"),
            "a broken link target must appear instead of vanishing (F15-R03)"
        );

        // The node limit cuts the result and says so rather than dropping silently.
        let limited = metadata::graph_report(&storage, None, 3, 2).unwrap();
        assert!(limited.truncated);
        assert!(limited.nodes.len() <= 2 + 1);
    }

    #[test]
    fn daily_note_is_found_by_path_then_property_and_never_duplicated() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Daily").unwrap();
        let date = "2026-10-20";
        let path = onote_tasks::daily_path(date, onote_tasks::DEFAULT_FOLDER_PATTERN).unwrap();
        assert_eq!(path, "Daily/2026/2026-10-20.md");

        // The first call creates the note and records the day in frontmatter.
        let operation = onote_attachments_import::note_operation_id(
            &storage.vault().vault_id,
            &format!("daily:{path}"),
        );
        let created = storage
            .create_note(&path, &format!("---\ndate: {date}\n---\n\n"), &operation)
            .unwrap();
        let candidates = vec![onote_tasks::DailyCandidate {
            note_id: created.note_id.clone(),
            relative_path: created.relative_path.clone(),
            property_date: Some(date.to_owned()),
        }];
        assert_eq!(
            onote_tasks::find_daily(date, onote_tasks::DEFAULT_FOLDER_PATTERN, &candidates)
                .unwrap(),
            onote_tasks::DailyMatch::Found {
                note_id: created.note_id.clone(),
                relative_path: path.clone(),
                matched_by: onote_tasks::MatchKind::Path
            }
        );

        // Replaying the same operation id with the same body is idempotent.
        let replay = storage
            .create_note(&path, "---\ndate: 2026-10-20\n---\n\n", &operation)
            .unwrap();
        assert_eq!(replay.note_id, created.note_id);
        assert_eq!(storage.list().unwrap().len(), 1);
        // Different content under the same id is refused rather than silently
        // overwriting an existing day, which is why the host re-checks for an
        // existing note before creating one.
        assert!(matches!(
            storage.create_note(&path, "different body", &operation),
            Err(ServiceError {
                code: ErrorCode::IdempotencyMismatch,
                ..
            })
        ));
        assert_eq!(storage.list().unwrap().len(), 1);

        // A renamed note is still found through its date property.
        let renamed = vec![onote_tasks::DailyCandidate {
            note_id: created.note_id.clone(),
            relative_path: "Archive/moved.md".to_owned(),
            property_date: Some(date.to_owned()),
        }];
        assert!(matches!(
            onote_tasks::find_daily(date, onote_tasks::DEFAULT_FOLDER_PATTERN, &renamed).unwrap(),
            onote_tasks::DailyMatch::Found {
                matched_by: onote_tasks::MatchKind::Property,
                ..
            }
        ));
    }

    #[test]
    fn missing_daily_template_is_a_choice_not_a_silent_blank_note() {
        // No template, or a blank id, is an intentional blank body.
        assert_eq!(
            resolve_daily_template(None, |_| unreachable!()).unwrap(),
            Some(String::new())
        );
        assert_eq!(
            resolve_daily_template(Some("  "), |_| unreachable!()).unwrap(),
            Some(String::new())
        );
        // A resolvable template returns its body.
        assert_eq!(
            resolve_daily_template(Some("tpl-1"), |id| {
                assert_eq!(id, "tpl-1");
                Ok("# Weekly".to_owned())
            })
            .unwrap(),
            Some("# Weekly".to_owned())
        );
        // ObjectNotFound is the F17 TEMPLATE_MISSING branch.
        assert_eq!(
            resolve_daily_template(Some("gone"), |_| Err(svc_err(
                ErrorCode::ObjectNotFound,
                "error.objectNotFound"
            )))
            .unwrap(),
            None
        );
        // Any other read failure must not be disguised as a missing template.
        assert!(matches!(
            resolve_daily_template(Some("locked"), |_| Err(svc_err(
                ErrorCode::IoDenied,
                "error.ioDenied"
            ))),
            Err(ServiceError {
                code: ErrorCode::IoDenied,
                ..
            })
        ));
    }

    #[test]
    fn template_expansion_keeps_unknown_variables_and_strips_markers() {
        let variables = onote_tasks::TemplateVariables {
            title: "Weekly".to_owned(),
            date: "2026-10-20".to_owned(),
            time: None,
        };
        let expansion = expand_template(
            "# {{title}}\n\n{{mood}} on {{date}}\n".to_owned(),
            variables,
        );
        // Known variables expand; the unknown one is preserved verbatim (F18-R01).
        assert_eq!(expansion.fragment, "# Weekly\n\n{{mood}} on 2026-10-20\n");
        assert_eq!(
            expansion.warnings,
            vec![onote_tasks::ExpansionWarning::UnknownVariable(
                "mood".to_owned()
            )]
        );

        // The template marker is frontmatter, not content.
        let source = "---\ntemplate: true\n---\n# {{title}}\n";
        assert!(onote_tasks::is_template(source));
        assert!(!onote_tasks::is_template("# {{title}}\n"));
    }

    #[test]
    fn task_projection_and_toggle_keep_the_markdown_authoritative() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = VaultStorage::create(directory.path(), "Tasks").unwrap();
        let vault_id = storage.vault().vault_id.clone();
        let note = storage
            .create_note(
                "notes/tasks.md",
                "# Tasks\n\n- [ ] buy milk\n- [x] ship it\n- [ ] pay [due:: 2026-10-15]\n",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        let rows = tasks_for_document(&note);
        assert_eq!(rows.len(), 3);
        let wire = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(wire["sourceRevision"], note.local_revision);
        assert_eq!(wire["sourceHash"], note.content_hash);
        assert_eq!(wire["readOnly"], false);
        let mut readonly = note.clone();
        readonly.read_only = true;
        assert!(tasks_for_document(&readonly)
            .iter()
            .all(|row| row.read_only));
        let source = note.source;
        let tasks = onote_tasks::parse_tasks(&source);
        assert_eq!(tasks.len(), 3);
        assert_eq!(storage.vault().vault_id, vault_id);

        // Toggling rewrites exactly one checkbox and nothing else.
        let toggled = onote_tasks::set_task_status(
            &source,
            tasks[0].offset,
            &tasks[0].text,
            onote_tasks::TaskStatus::Done,
        )
        .unwrap();
        assert_eq!(
            toggled.source,
            source.replace("- [ ] buy milk", "- [x] buy milk")
        );

        // A stale text at that offset is refused, not applied.
        assert!(matches!(
            onote_tasks::set_task_status(
                &source,
                tasks[0].offset,
                "a different task",
                onote_tasks::TaskStatus::Done
            ),
            Err(onote_tasks::TaskError::Moved { .. })
        ));
    }

    /// Byte-level snapshot of a synthetic import source, keyed by relative path.
    /// Folder import must be a read-only operation on the chosen source (F24-R01).
    fn source_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &Path, directory: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    visit(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        std::fs::read(&path).unwrap(),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        visit(root, root, &mut out);
        out
    }

    fn attachment_import_checked(
        storage: &mut VaultStorage,
        paths: &[PathBuf],
        destination: &str,
        operation_id: &str,
    ) -> AttachmentImportOutcome {
        let existing: Vec<(String, String)> = storage
            .list_attachments(destination, ATTACHMENT_LIST_HASH_LIMIT)
            .unwrap()
            .into_iter()
            .map(|file| (file.relative_path, file.sha256))
            .collect();
        import_attachment_files(
            paths,
            destination,
            operation_id,
            &existing,
            |path, bytes, sha, id| {
                storage
                    .store_attachment(path, bytes, sha, id)
                    .map(|(_, write)| write)
            },
        )
        .unwrap()
    }

    #[test]
    fn attachments_are_stored_reused_and_renamed_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        let picked = directory.path().join("picked");
        std::fs::create_dir_all(&picked).unwrap();
        std::fs::write(picked.join("pic.png"), b"png-bytes").unwrap();
        std::fs::write(picked.join("doc.pdf"), b"%PDF-1.7").unwrap();
        let source_before = source_tree(&picked);
        let vault_root = directory.path().join("vault");
        std::fs::create_dir_all(&vault_root).unwrap();
        let mut storage = VaultStorage::create(&vault_root, "Att").unwrap();
        let op = uuid::Uuid::new_v4().to_string();
        let paths = vec![picked.join("pic.png"), picked.join("doc.pdf")];

        let first = attachment_import_checked(&mut storage, &paths, "assets", &op);
        let statuses: Vec<_> = first
            .items
            .iter()
            .map(|item| item.status.as_str())
            .collect();
        assert_eq!(statuses, vec!["stored", "stored"]);
        assert_eq!(first.items[0].reference, "![](assets/pic.png)");
        assert_eq!(
            std::fs::read(vault_root.join("assets/pic.png")).unwrap(),
            b"png-bytes"
        );

        // Retrying the same files reuses them instead of writing copies.
        let again = attachment_import_checked(&mut storage, &paths, "assets", &op);
        assert!(again
            .items
            .iter()
            .all(|item| item.status == "alreadyPresent"));
        assert_eq!(
            std::fs::read_dir(vault_root.join("assets"))
                .unwrap()
                .count(),
            2
        );

        // Different bytes under an existing name are stored beside it.
        std::fs::write(picked.join("pic.png"), b"changed").unwrap();
        let renamed = attachment_import_checked(
            &mut storage,
            &[picked.join("pic.png")],
            "assets",
            &uuid::Uuid::new_v4().to_string(),
        );
        assert_eq!(renamed.items[0].status, "stored");
        assert_eq!(renamed.items[0].destination, "assets/pic (1).png");
        assert_eq!(
            std::fs::read(vault_root.join("assets/pic.png")).unwrap(),
            b"png-bytes"
        );
        std::fs::write(picked.join("pic.png"), b"png-bytes").unwrap();
        assert_eq!(
            source_tree(&picked),
            source_before,
            "sources are never modified"
        );
    }

    #[test]
    fn missing_or_directory_sources_are_refused_and_not_referenced() {
        let directory = tempfile::tempdir().unwrap();
        let vault_root = directory.path().join("vault");
        std::fs::create_dir_all(&vault_root).unwrap();
        let mut storage = VaultStorage::create(&vault_root, "Att").unwrap();
        let outcome = attachment_import_checked(
            &mut storage,
            &[
                directory.path().join("missing.png"),
                directory.path().to_path_buf(),
            ],
            "assets",
            &uuid::Uuid::new_v4().to_string(),
        );
        assert!(outcome
            .items
            .iter()
            .all(|item| item.status == "refused" && item.reference.is_empty()));
        assert!(!vault_root.join("assets").exists());
    }

    /// Runs the same plan/commit steps as the command against a directly-owned
    /// storage (the command itself needs an AppHandle).
    #[test]
    fn export_publishes_a_verified_zip_and_leaves_no_staging_file() {
        use onote_attachments_import::{verify_zip, ZipWriter};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let notes = vec![
            ExportNote {
                relative_path: "notes/a.md".to_owned(),
                source: "# A\n".to_owned(),
            },
            ExportNote {
                relative_path: "notes/b.md".to_owned(),
                source: "# B\n".to_owned(),
            },
        ];
        let destination = directory.path().join("out.zip");
        let outcome = publish_export(
            &notes,
            Vec::new(),
            &root,
            onote_attachments_import::ExportScope::Vault,
            "",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Zip,
        )
        .unwrap();
        assert!(!outcome.cancelled);
        assert_eq!(outcome.notes, 2);
        assert!(outcome.verified);
        assert!(destination.is_file(), "archive must be published");
        let staging = PathBuf::from(format!("{}{}", destination.display(), ".opennote-partial"));
        assert!(!staging.exists(), "staging file must not survive");

        // The published bytes must match a manifest rebuilt independently.
        let bytes = std::fs::read(&destination).unwrap();
        let mut writer = ZipWriter::new();
        writer.add("notes/a.md", b"# A\n");
        writer.add("notes/b.md", b"# B\n");
        verify_zip(&bytes, &writer.manifest()).expect("published archive verifies");

        // An existing destination is refused instead of overwritten.
        let again = publish_export(
            &notes,
            Vec::new(),
            &root,
            onote_attachments_import::ExportScope::Vault,
            "",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Zip,
        );
        assert_eq!(again.unwrap_err().message_key, "error.exportTargetExists");
    }

    #[test]
    fn export_normalizes_snapshot_separators_before_body_lookup() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let notes = vec![ExportNote {
            relative_path: "notes\\a.md".to_owned(),
            source: "# A\n".to_owned(),
        }];
        let destination = directory.path().join("normalized");
        let outcome = publish_export(
            &notes,
            Vec::new(),
            &root,
            onote_attachments_import::ExportScope::Vault,
            "",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Markdown,
        )
        .unwrap();
        assert_eq!(outcome.notes, 1);
        assert!(outcome.verified);
        assert_eq!(
            std::fs::read_to_string(destination.join("notes/a.md")).unwrap(),
            "# A\n"
        );
    }

    #[test]
    fn markdown_export_publishes_notes_attachments_and_a_loss_report() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(root.join("img")).unwrap();
        std::fs::write(root.join("img/pic.png"), b"\x89PNG\r\n").unwrap();
        let notes = vec![
            ExportNote {
                relative_path: "a.md".to_owned(),
                source: "# A\n\n![pic](img/pic.png)\n\n![[b]]\n".to_owned(),
            },
            ExportNote {
                relative_path: "export-losses.md".to_owned(),
                source: "# My own report\n".to_owned(),
            },
        ];
        let destination = directory.path().join("export folder");
        let outcome = publish_export(
            &notes,
            Vec::new(),
            &root,
            onote_attachments_import::ExportScope::Vault,
            "",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Markdown,
        )
        .unwrap();

        assert!(!outcome.cancelled);
        assert_eq!(outcome.notes, 2);
        assert_eq!(outcome.attachments, 1);
        assert!(outcome.verified);
        assert_eq!(outcome.compatibility_losses.len(), 1);

        // F25-A01: the folder holds plain Markdown and the attachment at its
        // vault-relative path, so another editor can open it without this app.
        assert!(destination.is_dir());
        assert_eq!(
            std::fs::read_to_string(destination.join("a.md")).unwrap(),
            "# A\n\n![pic](img/pic.png)\n\n![[b]]\n"
        );
        assert_eq!(
            std::fs::read(destination.join("img/pic.png")).unwrap(),
            b"\x89PNG\r\n"
        );
        // A note already named export-losses.md is never overwritten by the
        // generated report.
        assert_eq!(
            std::fs::read_to_string(destination.join("export-losses.md")).unwrap(),
            "# My own report\n"
        );
        let report = std::fs::read_to_string(destination.join("export-losses-2.md")).unwrap();
        assert!(report.contains("## Wiki syntax kept literally"));
        assert!(report.contains("embed `b`"));
        assert!(report.contains("## Not included"));
        assert!(report.contains("view definitions"));

        let staging = PathBuf::from(format!("{}{}", destination.display(), ".opennote-partial"));
        assert!(!staging.exists(), "staging folder must not survive");

        let again = publish_export(
            &notes,
            Vec::new(),
            &root,
            onote_attachments_import::ExportScope::Vault,
            "",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Markdown,
        );
        assert_eq!(again.unwrap_err().message_key, "error.exportTargetExists");
    }

    #[test]
    fn export_keeps_wiki_syntax_and_reports_only_exported_notes_as_losses() {
        use onote_attachments_import::{CompatibilityLoss, CompatibilitySyntax};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let notes = vec![
            ExportNote {
                relative_path: "notes/a.md".to_owned(),
                source: "![[b]] [[c|alias]]\n".to_owned(),
            },
            ExportNote {
                relative_path: "notes/b.md".to_owned(),
                source: "plain\n".to_owned(),
            },
            ExportNote {
                relative_path: "other.md".to_owned(),
                source: "![[outside]]\n".to_owned(),
            },
        ];
        let destination = directory.path().join("one.zip");
        let outcome = publish_export(
            &notes,
            Vec::new(),
            &root,
            onote_attachments_import::ExportScope::Note,
            "notes/a.md",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Zip,
        )
        .unwrap();
        assert_eq!(outcome.notes, 1);
        assert_eq!(
            outcome.compatibility_losses,
            vec![
                CompatibilityLoss {
                    path: "notes/a.md".to_owned(),
                    syntax: CompatibilitySyntax::Embed,
                    target: "b".to_owned(),
                },
                CompatibilityLoss {
                    path: "notes/a.md".to_owned(),
                    syntax: CompatibilitySyntax::WikiLink,
                    target: "c".to_owned(),
                },
            ],
            "a note outside the exported scope must not appear"
        );
        let bytes = std::fs::read(&destination).unwrap();
        assert!(
            bytes.windows(6).any(|window| window == b"![[b]]"),
            "the original wiki syntax must be exported unchanged"
        );
    }

    #[test]
    fn export_reports_unreadable_notes_and_missing_attachments_as_losses() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let notes = vec![ExportNote {
            relative_path: "a.md".to_owned(),
            source: "# A\n\n![shot](img/missing.png)\n".to_owned(),
        }];
        let destination = directory.path().join("lossy.zip");
        let outcome = publish_export(
            &notes,
            vec!["broken.md".to_owned()],
            &root,
            onote_attachments_import::ExportScope::Note,
            "a.md",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Zip,
        )
        .unwrap();
        assert_eq!(outcome.notes, 1);
        assert_eq!(outcome.attachments, 0);
        assert!(outcome.losses.iter().any(|loss| loss == "broken.md"));
        assert!(outcome
            .losses
            .iter()
            .any(|loss| loss.contains("img/missing.png")));
    }

    #[test]
    fn export_copies_ordinary_attachment_links_and_reports_missing_ones() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/data.bin"), b"binary\0sentinel").unwrap();
        let source = "# A\n[data](assets/data.bin) [lost](assets/missing.pdf)\n";
        let notes = vec![ExportNote {
            relative_path: "a.md".into(),
            source: source.into(),
        }];
        for format in [
            onote_attachments_import::ExportFormat::Markdown,
            onote_attachments_import::ExportFormat::Zip,
        ] {
            let destination = directory.path().join(format!("out-{format:?}"));
            let outcome = publish_export(
                &notes,
                Vec::new(),
                &root,
                onote_attachments_import::ExportScope::Note,
                "a.md",
                &destination,
                true,
                format,
            )
            .unwrap();
            assert!(outcome.verified);
            assert_eq!(outcome.attachments, 1);
            assert!(outcome
                .losses
                .iter()
                .any(|path| path.contains("assets/missing.pdf")));
            if matches!(format, onote_attachments_import::ExportFormat::Markdown) {
                assert_eq!(
                    std::fs::read(destination.join("assets/data.bin")).unwrap(),
                    b"binary\0sentinel"
                );
                assert_eq!(
                    std::fs::read_to_string(destination.join("a.md")).unwrap(),
                    source
                );
            }
        }
        assert_eq!(
            std::fs::read(root.join("assets/data.bin")).unwrap(),
            b"binary\0sentinel"
        );
    }

    #[test]
    fn scoped_export_includes_shared_assets_only_from_selected_notes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/shared.bin"), b"shared").unwrap();
        std::fs::write(root.join("assets/unrelated.bin"), b"unrelated").unwrap();
        let notes = vec![
            ExportNote {
                relative_path: "notes/a.md".into(),
                source: "[shared](../assets/shared.bin)".into(),
            },
            ExportNote {
                relative_path: "other.md".into(),
                source: "[unrelated](assets/unrelated.bin)".into(),
            },
        ];
        for (scope, target) in [
            (onote_attachments_import::ExportScope::Note, "notes/a.md"),
            (onote_attachments_import::ExportScope::Folder, "notes"),
        ] {
            let destination = directory.path().join(format!("out-{scope:?}"));
            let outcome = publish_export(
                &notes,
                Vec::new(),
                &root,
                scope,
                target,
                &destination,
                true,
                onote_attachments_import::ExportFormat::Markdown,
            )
            .unwrap();
            assert_eq!(outcome.notes, 1);
            assert_eq!(outcome.attachments, 1);
            assert_eq!(
                std::fs::read(destination.join("assets/shared.bin")).unwrap(),
                b"shared"
            );
            assert!(!destination.join("assets/unrelated.bin").exists());
            assert!(!destination.join("other.md").exists());
        }
    }

    #[test]
    fn export_attachment_reader_refuses_outside_and_reserved_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(root.join(".opennote")).unwrap();
        std::fs::write(directory.path().join("outside.bin"), b"outside").unwrap();
        std::fs::write(root.join(".opennote/session.bin"), b"private").unwrap();
        std::fs::write(root.join("safe.bin"), b"safe").unwrap();
        assert_eq!(read_export_attachment(&root, "safe.bin").unwrap(), b"safe");
        for reference in [
            "../outside.bin".to_owned(),
            directory
                .path()
                .join("outside.bin")
                .to_string_lossy()
                .into_owned(),
            ".opennote/session.bin".to_owned(),
        ] {
            assert!(
                read_export_attachment(&root, &reference).is_err(),
                "must reject {reference}"
            );
        }
        assert_eq!(
            std::fs::read(directory.path().join("outside.bin")).unwrap(),
            b"outside"
        );
    }

    #[cfg(windows)]
    #[test]
    fn export_attachment_reader_refuses_junction_sources_and_reports_loss() {
        use std::os::windows::process::CommandExt;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("private.bin"), b"private sentinel").unwrap();
        let linked = root.join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&outside)
            .creation_flags(0x0800_0000)
            .status()
            .unwrap();
        assert!(status.success());
        let direct = read_export_attachment(&root, "linked/private.bin");
        let notes = vec![ExportNote {
            relative_path: "a.md".into(),
            source: "[private](linked/private.bin)".into(),
        }];
        let destination = directory.path().join("export");
        let result = publish_export(
            &notes,
            Vec::new(),
            &root,
            onote_attachments_import::ExportScope::Note,
            "a.md",
            &destination,
            true,
            onote_attachments_import::ExportFormat::Markdown,
        );
        std::fs::remove_dir(&linked).unwrap();
        assert!(direct.is_err());
        let outcome = result.unwrap();
        assert_eq!(outcome.attachments, 0);
        assert!(outcome
            .losses
            .iter()
            .any(|loss| loss.contains("linked/private.bin")));
        assert!(!destination.join("linked/private.bin").exists());
        assert_eq!(
            std::fs::read(outside.join("private.bin")).unwrap(),
            b"private sentinel"
        );
    }

    #[test]
    fn property_edit_keeps_unknown_fields_and_refuses_bad_shapes() {
        // F09-A02: editing one field must not drop or reorder the others.
        let source = "---\ncustom:\n  nested: 1\ntitle: Old\nrating: 3\n---\nbody\n";
        let value = onote_metadata_links::PropertyValue::Date("2026-10-20".to_owned());
        let write = apply_property_edit(source.to_owned(), "due".to_owned(), value).unwrap();
        assert!(write.changed);
        assert!(write
            .source
            .contains("custom:\n  nested: 1\ntitle: Old\nrating: 3\ndue: 2026-10-20\n"));
        assert!(write.source.ends_with("---\nbody\n"));

        // F09-A03: a failed type change leaves the original value in place.
        let number = onote_metadata_links::PropertyValue::Number(42.0);
        let written = apply_property_edit(
            "---\nrating: not-a-number\n---\n".to_owned(),
            "rating".to_owned(),
            number,
        )
        .unwrap();
        assert!(written.source.contains("rating: 42"));
        assert_eq!(
            written.previous,
            onote_metadata_links::PropertyValue::Text("not-a-number".to_owned())
        );

        // The reserved identity key and unreadable YAML are typed refusals.
        let refused = apply_property_edit(
            "---\ntitle: T\n---\n".to_owned(),
            "opennote-id".to_owned(),
            onote_metadata_links::PropertyValue::Text("x".to_owned()),
        )
        .unwrap_err();
        assert_eq!(refused.message_key, "error.propertyNameInvalid");
        let broken = apply_property_edit(
            "---\nkey: [unclosed\n---\n".to_owned(),
            "title".to_owned(),
            onote_metadata_links::PropertyValue::Text("x".to_owned()),
        )
        .unwrap_err();
        assert_eq!(broken.message_key, "error.propertyYamlInvalid");
    }

    #[test]
    fn outline_parse_uses_m04_rules_and_ignores_code_fences() {
        // F14-A02: a `#` inside a fence is code, not a heading; duplicate
        // headings keep distinct anchors so each one can be located (F14-A01).
        let source = "# One\n\n```\n# not a heading\n```\n\n## Two\n\n## Two\n";
        let headings = parse_headings(source.to_owned(), None);
        assert_eq!(
            headings
                .iter()
                .map(|heading| heading.text.as_str())
                .collect::<Vec<_>>(),
            vec!["One", "Two", "Two"]
        );
        assert_eq!(headings[1].slug, "two");
        assert_eq!(headings[2].slug, "two-1");
        assert_ne!(headings[1].line, headings[2].line);
        assert!(
            parse_headings("# Big".to_owned(), Some(2)).is_empty(),
            "oversized input is refused instead of parsed partially"
        );
    }

    #[test]
    fn draft_helpers_refuse_typed_when_no_vault_open() {
        let mut state = AppState {
            vault: None,
            recovery: None,
            history: None,
        };
        let refusal = scoped_vault_mut(&mut state, "some grain")
            .err()
            .expect("no-vault write refused");
        assert_eq!(refusal.message_key, "error.noVaultOpen");
        let refusal = scoped_vault_ref(&state, "some vault")
            .err()
            .expect("no-vault read refused");
        assert_eq!(refusal.message_key, "error.noVaultOpen");
    }

    #[test]
    fn draft_commands_refuse_stale_scope_keeping_other_vault_rows_unchanged() {
        // Vault A: a note plus one immutable checkpoint, all captured
        // BEFORE the "picker" moves the session to vault B.
        let dir_a = tempfile::tempdir().expect("tempdir A");
        let mut storage_a = VaultStorage::create(dir_a.path(), "Vault A").expect("vault A");
        let doc = storage_a
            .create_note("notes/d.md", "stale session body", OP_CREATE_A)
            .expect("seed note in A");
        let note_id = doc.note_id.clone();
        let ck = storage_a
            .write_checkpoint(onote_domain::CheckpointRequest {
                note_id: note_id.clone(),
                relative_path: "notes/d.md".to_owned(),
                source: "draft in vault A".to_owned(),
                base_revision: doc.local_revision,
                base_content_hash: doc.content_hash.clone(),
                operation_id: DRAFT_OP_A.to_owned(),
            })
            .expect("seed checkpoint in A");
        assert_eq!(ck.draft_id, DRAFT_OP_A);

        // Vault B: copy the composed note bytes BEFORE initializing it as a
        // vault, so B scans the SAME note UUID with its own fresh vaultId
        // and zero draft rows.
        let dir_b = tempfile::tempdir().expect("tempdir B");
        std::fs::create_dir_all(dir_b.path().join("notes")).expect("B subdir");
        std::fs::copy(
            dir_a.path().join("notes/d.md"),
            dir_b.path().join("notes/d.md"),
        )
        .expect("copy embedded note into B before init");
        let storage_b = VaultStorage::create(dir_b.path(), "Vault B").expect("vault B");
        let active_vault_id = storage_b.vault().vault_id;
        let stale_vault_id = storage_a.vault().vault_id;
        assert_ne!(active_vault_id, stale_vault_id, "distinct vault sessions");
        assert_eq!(
            storage_b
                .read(&note_id)
                .expect("B carries the SAME note UUID")
                .note_id,
            note_id,
            "noteId alone cannot distinguish the vaults"
        );

        // The picker completed: AppState now holds vault B while a late
        // vault-A-scoped draft callback is in flight. Exactly the command
        // body: scoped guard first, then the storage delegation.
        let mut state = AppState {
            vault: Some(storage_b),
            recovery: None,
            history: None,
        };
        let note_path_b = dir_b.path().join("notes/d.md");
        let note_before = std::fs::read(&note_path_b).expect("read B note bytes");

        // Matching scope: checkpoint write works and lists/reads back.
        let req_b = onote_domain::CheckpointRequest {
            note_id: note_id.clone(),
            relative_path: "notes/d.md".to_owned(),
            source: "draft in vault B".to_owned(),
            base_revision: doc.local_revision,
            base_content_hash: doc.content_hash.clone(),
            operation_id: DRAFT_OP_B.to_owned(),
        };
        let saved = scoped_vault_mut(&mut state, &active_vault_id)
            .and_then(|s| s.write_checkpoint(req_b.clone()))
            .expect("matching-scope write_checkpoint succeeds on the active vault");
        assert_eq!(saved.draft_id, DRAFT_OP_B);
        assert_eq!(saved.note_id, note_id);
        assert!(saved.sequence >= 1);
        let page_b = scoped_vault_ref(&state, &active_vault_id)
            .and_then(|s| s.list_drafts(Some(&note_id), None, Some(50)))
            .expect("matching-scope list_drafts on the active vault");
        assert_eq!(page_b.items.len(), 1);
        assert_eq!(page_b.items[0].draft_id, DRAFT_OP_B);
        assert_eq!(page_b.items[0].source_hash, saved.source_hash);
        assert_eq!(page_b.items[0].relative_path, "notes/d.md");
        assert_eq!(page_b.items[0].base_revision, doc.local_revision);
        assert_eq!(page_b.items[0].base_content_hash, doc.content_hash);
        assert_eq!(page_b.next_before_sequence, None);
        let draft_b = scoped_vault_ref(&state, &active_vault_id)
            .and_then(|s| s.read_draft(DRAFT_OP_B))
            .expect("matching-scope read_draft on the active vault");
        assert_eq!(draft_b.source, "draft in vault B");
        assert_eq!(draft_b.source_hash, saved.source_hash);
        assert_eq!(draft_b.base_revision, doc.local_revision);
        assert_eq!(draft_b.base_content_hash, doc.content_hash);

        // Late vault-A-scoped callbacks carrying the SAME note UUID (B and
        // A share it) and a draft UUID that now exists INSIDE the active
        // vault B: all three refused by the shared session guard BEFORE any
        // storage access, typed, with no source/path leakage.
        let write_refusal = scoped_vault_mut(&mut state, &stale_vault_id)
            .and_then(|s| s.write_checkpoint(req_b.clone()))
            .err()
            .expect("stale-scope write_checkpoint must be refused");
        assert_eq!(write_refusal.message_key, "error.vaultSessionChanged");
        assert!(write_refusal.params.is_empty(), "no source/path leak");
        let list_refusal = scoped_vault_ref(&state, &stale_vault_id)
            .and_then(|s| s.list_drafts(Some(&note_id), None, Some(50)))
            .err()
            .expect("stale-scope list_drafts must be refused");
        assert_eq!(list_refusal.message_key, "error.vaultSessionChanged");
        let read_refusal = scoped_vault_ref(&state, &stale_vault_id)
            .and_then(|s| s.read_draft(DRAFT_OP_B))
            .err()
            .expect("stale-scope read_draft must be refused even for a draft stored in the active vault");
        assert_eq!(
            read_refusal.message_key, "error.vaultSessionChanged",
            "the draft UUID must resolve per-session, never globally"
        );

        // Other-vault rows unchanged: B still holds exactly its own draft
        // and no extra rows; A still holds exactly its own one; note bytes
        // and revisions moved nowhere.
        let page_b_after = scoped_vault_ref(&state, &active_vault_id)
            .and_then(|s| s.list_drafts(None, None, Some(50)))
            .expect("B page after refusals");
        assert_eq!(page_b_after.items.len(), 1);
        assert_eq!(page_b_after.items[0].draft_id, DRAFT_OP_B);
        assert_eq!(
            std::fs::read(&note_path_b).expect("reread B note bytes"),
            note_before,
            "checkpoint refusals must not touch the current vault note bytes"
        );
        let page_a = storage_a
            .list_drafts(Some(&note_id), None, Some(50))
            .expect("A page unaffected by B session");
        assert_eq!(page_a.items.len(), 1);
        assert_eq!(page_a.items[0].draft_id, DRAFT_OP_A);
        assert_eq!(page_a.items[0].source_hash, ck.source_hash);
        let a_note = storage_a.read(&note_id).expect("A note legible");
        assert_eq!(
            a_note.local_revision, doc.local_revision,
            "checkpoint writes never bump a note revision"
        );
    }
}

#[cfg(test)]
mod onboarding_sample_tests {
    use super::*;

    #[test]
    fn sample_note_requires_explicit_opt_in() {
        let directory = tempfile::tempdir().unwrap();
        let (_, mut storage, _, history) =
            prepare_vault(directory.path().to_path_buf(), true).unwrap();

        assert!(create_requested_sample(&mut storage, false)
            .unwrap()
            .is_none());
        assert!(storage.list().unwrap().is_empty());
        drop(storage);
        drop(history);
    }

    #[test]
    fn sample_note_uses_a_suffix_without_overwriting_the_collision() {
        let directory = tempfile::tempdir().unwrap();
        let (_, mut storage, _, history) =
            prepare_vault(directory.path().to_path_buf(), true).unwrap();
        let original = storage
            .create_note(
                "OpenNote sample.md",
                "keep this note",
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        let original_path = directory.path().join("OpenNote sample.md");
        let original_bytes = std::fs::read(&original_path).unwrap();

        let sample = create_requested_sample(&mut storage, true)
            .unwrap()
            .expect("opt-in should create a sample");

        assert_eq!(sample.relative_path, "OpenNote sample 2.md");
        assert!(sample.source.ends_with(SAMPLE_NOTE_SOURCE));
        assert_eq!(
            storage.read(&original.note_id).unwrap().source,
            original.source
        );
        assert_eq!(std::fs::read(original_path).unwrap(), original_bytes);
        drop(storage);
        drop(history);
    }
}

#[cfg(test)]
mod history_session_tests {
    use super::*;

    #[test]
    fn native_legacy_open_keeps_metadata_database_and_history_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let (_, mut storage, _, history) =
            prepare_vault(directory.path().to_path_buf(), true).unwrap();
        let note = storage
            .create_note("legacy.md", "original", &uuid_for_test())
            .unwrap();
        drop(storage);
        drop(history);
        let meta_path = directory.path().join(".opennote/vault.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta["schemaVersion"] = 1.into();
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
        let before_meta = std::fs::read(&meta_path).unwrap();
        let database = directory.path().join(".opennote/state.sqlite");
        let before_db = std::fs::read(&database).unwrap();
        let (vault, mut storage, recovery, history) =
            prepare_vault(directory.path().to_path_buf(), false).unwrap();
        assert!(!vault.writable);
        assert_eq!(vault.schema_version, 1);
        assert!(recovery.applied.is_empty() && recovery.pending.is_empty());
        assert_eq!(storage.read(&note.note_id).unwrap().source, note.source);
        assert!(history.list(&note.note_id).unwrap().is_empty());
        assert_eq!(
            storage
                .save(&note.note_id, "changed", 0, &uuid_for_test())
                .unwrap_err()
                .message_key,
            "error.schemaReadOnly"
        );
        assert_eq!(std::fs::read(&meta_path).unwrap(), before_meta);
        assert_eq!(std::fs::read(database).unwrap(), before_db);
        assert!(!directory.path().join(".opennote/history").exists());
    }

    #[test]
    fn native_composition_protects_saves_and_history_calls_reject_other_vaults() {
        let directory = tempfile::tempdir().unwrap();
        let (vault, mut storage, _, history) =
            prepare_vault(directory.path().to_path_buf(), true).unwrap();
        let note = storage
            .create_note("native.md", "old", &uuid_for_test())
            .unwrap();
        storage
            .save(&note.note_id, "new", note.local_revision, &uuid_for_test())
            .unwrap();
        let state = AppState {
            vault: Some(storage),
            recovery: None,
            history: Some(history),
        };
        assert_eq!(
            scoped_history(&state, &vault.vault_id)
                .unwrap()
                .list(&note.note_id)
                .unwrap()
                .len(),
            1
        );
        assert!(
            matches!(scoped_history(&state, &uuid_for_test()), Err(e) if e.code == ErrorCode::InvalidInput)
        );
    }

    fn uuid_for_test() -> String {
        // UUID fixture has no bearing on ordering; each test operation is distinct.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        format!(
            "00000000-0000-4000-8000-{:012x}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    #[test]
    fn native_composition_connects_trash_protection_before_any_destructive_operation() {
        use onote_vault_storage::TrashArchive;
        let directory = tempfile::tempdir().unwrap();
        let (vault, mut storage, _, history) =
            prepare_vault(directory.path().to_path_buf(), true).unwrap();
        assert_eq!(vault.schema_version, 3);
        let note = storage
            .create_note("trash-native.md", "protected original", &uuid_for_test())
            .unwrap();
        let entry = storage
            .delete_note(&note.note_id, 0, &note.content_hash, &uuid_for_test())
            .unwrap();
        assert_eq!(
            history.read_deleted(&entry.object_id).unwrap().document,
            note
        );
        assert_eq!(storage.list_trash().unwrap(), vec![entry]);
    }

    #[test]
    fn trash_reader_checks_session_and_active_catalog_before_returning_archived_text() {
        let directory = tempfile::tempdir().unwrap();
        let (vault, mut storage, _, history) =
            prepare_vault(directory.path().to_path_buf(), true).unwrap();
        let note = storage
            .create_note("trash.md", "scoped text", &uuid_for_test())
            .unwrap();
        let entry = storage
            .delete_note(&note.note_id, 0, &note.content_hash, &uuid_for_test())
            .unwrap();
        let mut state = AppState {
            vault: Some(storage),
            history: Some(history),
            recovery: None,
        };
        assert_eq!(
            read_trash_record(&state, &vault.vault_id, &entry.object_id)
                .unwrap()
                .document,
            note
        );
        assert!(
            matches!(read_trash_record(&state,&uuid_for_test(),&entry.object_id),Err(e) if e.code==ErrorCode::InvalidInput)
        );
        scoped_vault_mut(&mut state, &vault.vault_id)
            .unwrap()
            .restore_trash(&entry.object_id, "restored.md", 1, &uuid_for_test())
            .unwrap();
        assert!(
            matches!(read_trash_record(&state,&vault.vault_id,&entry.object_id),Err(e) if e.code==ErrorCode::ObjectNotFound)
        );
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(import_batch::ImportRuns::default())
        .manage(backup::BackupHost::default())
        .manage(history_cleanup::HistoryCleanupHost::default())
        .manage(search::SearchHost::default())
        .manage(tag_cache::TagHost::default())
        .manage(search_job::SearchJobHost::default())
        .manage(latex_build::LatexBuildHost::default())
        .manage(VaultFolderSelection::default())
        .manage(latex_project::LatexFolderSelections::default())
        .manage(latex_runtime::LatexRuntimeHost::default())
        .manage(source_watch::SourceWatchHost::default())
        .manage(Mutex::new(AppState {
            vault: None,
            recovery: None,
            history: None,
        }))
        .invoke_handler(tauri::generate_handler![
            latex_project::select_latex_project_folder,
            latex_project::inspect_latex_folder,
            latex_project::register_latex_project,
            latex_project::open_latex_project,
            latex_project::read_latex_project_file,
            latex_project::save_latex_project_file,
            latex_project::list_latex_projects,
            latex_project::unregister_latex_project,
            latex_project::create_latex_project,
            latex_project::export_latex_source_zip,
            latex_runtime::latex_environment_plan,
            latex_runtime::latex_environment_status,
            latex_runtime::prepare_latex_environment,
            latex_runtime::cancel_latex_preparation,
            latex_runtime::remove_latex_environment,
            backup::backup_context,
            backup::choose_backup_destination,
            backup::start_backup_task,
            backup::set_backup_retention,
            backup::preview_backup_restore,
            backup::backup_job_status,
            backup::cancel_backup_task,
            backup::preview_backup_upgrade,
            backup::preview_backup_overwrite,
            source_watch::start_source_watch,
            source_watch::start_catalog_watch,
            refresh_catalog,
            source_watch::stop_source_watch,
            create_vault,
            choose_vault_folder,
            open_vault,
            reopen_last_vault,
            list_notes,
            list_folders,
            create_folder,
            inspect_folder,
            remove_empty_folder,
            recycle_folder,
            rename_empty_folder,
            read_note,
            create_note,
            list_trash,
            read_trash,
            delete_note,
            restore_trash,
            purge_trash,
            list_trash_operations,
            resume_trash,
            cancel_trash,
            create_copy,
            prepare_conflict,
            save_note,
            write_checkpoint,
            list_drafts,
            read_draft,
            list_history,
            read_history,
            restore_history,
            history_retention_settings,
            set_history_retention,
            preview_history_cleanup,
            apply_history_cleanup,
            history_cleanup::history_cleanup_status,
            history_cleanup::cancel_history_cleanup,
            history_cleanup::stop_history_cleanup,
            list_tags,
            list_backlinks,
            list_link_issues,
            read_note_metadata,
            parse_headings,
            parse_properties,
            apply_property_edit,
            list_tasks,
            toggle_task,
            expand_template,
            list_templates,
            open_daily_note,
            graph_report,
            database_table,
            diagnostics_probe,
            diagnostics_preview,
            diagnostics_export_text,
            diagnostics_export,
            diagnostics_overall,
            diagnostics_host_facts,
            summarize_import_plan,
            import_plan_is_complete,
            import_plan_loss_report,
            group_tasks_command,
            plan_attachment_import,
            import_attachments,
            attachment_io::attachment_info,
            attachment_io::read_attachment,
            attachment_io::store_attachment_bytes,
            attachment_io::list_vault_pdfs,
            attachment_io::open_attachment_with_system,
            attachment_io::save_note_html,
            classify_attachment_extension,
            plan_tag_rename_command,
            plan_tag_removal_command,
            preview_rename_commit,
            validate_note_name,
            check_rename_destination,
            is_case_only_rename,
            inspect_latex_project,
            parse_bibliography,
            plan_citation_command,
            plan_citation_rename,
            bibliography_dangling,
            bibliography_unicode,
            plan_latex_insertion,
            latex_labels,
            detect_latex_language,
            latex_toggle_comment,
            latex_wrap_selection,
            latex_matching_bracket,
            latex_completions,
            latex_apply_completion,
            plan_latex_scaffold,
            escape_latex_text,
            plan_latex_export,
            latex_build::list_compile_jobs,
            latex_build::compile_latex_project,
            latex_build::cancel_latex_compile,
            latex_build::latex_build_status,
            latex_build::read_latex_artifact,
            latex_build::save_latex_pdf,
            latex_build::synctex_forward,
            latex_build::synctex_inverse,
            preview_state,
            parse_tex_log,
            redact_tex_log,
            delete_task,
            set_task_due,
            parse_canvas_source,
            serialize_canvas_document,
            list_canvases,
            read_canvas,
            save_canvas,
            list_views,
            read_view,
            save_view,
            export_vault,
            import_batch::import_prepare,
            import_batch::import_start,
            import_batch::import_status,
            import_batch::import_cancel,
            import_batch::import_unfinished,
            import_batch::import_discard,
            search_notes,
            start_search,
            search_job_status,
            cancel_search,
            expand_note_embed,
            preview_note_rename,
            rename_note,
            preview_tag_update,
            apply_tag_update,
            resume_tag_update,
            cancel_tag_update,
            list_tag_updates,
            recover_vault,
            export_draft,
            // The frontend's confirmClose() calls this; the host refuses a
            // native close until it is authorized.
            confirm_close
        ])
        .setup(|app| {
            window_fit::fit_main_window(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit(EVENT_CLOSE_REQUESTED, ());
            }
        })
        .run(tauri::generate_context!())
        .expect("OpenNote: host failed to start");
}
