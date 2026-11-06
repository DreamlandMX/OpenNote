//! F45 LaTeX projects in the host: folder selection, inspection from disk,
//! registration and source read/save.
//!
//! The frontend never names a disk path. It receives a `selectionId` for a
//! folder the user picked inside the vault, then a `projectId` and per-file
//! `fileId`s from the stored record (`onote_latex::ProjectRecord`). Every
//! save goes through the record's journal, so an interrupted save is settled
//! from the bytes on disk the next time the project is opened.

use super::*;
use std::collections::HashMap;

use onote_latex::{ProjectRecord, Recovery, SaveStep, StoreError};
use onote_vault_storage::is_project_source;

/// Text read for inspection; a larger file is inspected by name only.
const MAX_INSPECT_TEXT_BYTES: u64 = 8 * 1024 * 1024;

/// Folders the user picked, by selection id, scoped to the vault they were
/// picked in.
#[derive(Default)]
pub(crate) struct LatexFolderSelections(Mutex<HashMap<String, (String, String)>>);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChosenLatexFolder {
    pub selection_id: String,
    /// Vault-relative folder for display; empty for the vault root.
    pub folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexProjectFile {
    pub file_id: String,
    /// Path relative to the project folder.
    pub path: String,
    pub revision: u64,
    /// Whether a source save may write this file.
    pub editable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexProject {
    pub project_id: String,
    pub folder: String,
    pub entry_file_id: String,
    pub files: Vec<LatexProjectFile>,
    /// How an interrupted save was settled while opening: `nothing`,
    /// `completed`, `abandoned` or `external`.
    pub recovery: &'static str,
    /// The vault or the record format does not allow writing.
    pub read_only: bool,
    /// Identity of the complete input (see [`manifest_hash`]); a compile
    /// request carries the value it was shown.
    pub manifest_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexProjectSource {
    pub project_id: String,
    pub file_id: String,
    pub path: String,
    pub text: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexSaveResult {
    pub file_id: String,
    pub revision: u64,
    /// False when nothing had to be written (a retry, or unchanged bytes).
    pub written: bool,
    /// The project's manifest after this save, so a PDF built before it can
    /// be shown as outdated without re-reading the whole folder.
    pub manifest_hash: String,
}

fn store_err(error: StoreError) -> ServiceError {
    let code = match error {
        StoreError::RevisionConflict { .. } | StoreError::SavePending => ErrorCode::VersionConflict,
        StoreError::UnknownFile => ErrorCode::ObjectNotFound,
        _ => ErrorCode::InvalidInput,
    };
    svc_err(code, error.message_key())
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn project_relative(folder: &str, path: &str) -> String {
    if folder.is_empty() {
        path.to_owned()
    } else {
        path.strip_prefix(&format!("{folder}/"))
            .unwrap_or(path)
            .to_owned()
    }
}

fn vault_relative(folder: &str, path: &str) -> String {
    if folder.is_empty() {
        path.to_owned()
    } else {
        format!("{folder}/{path}")
    }
}

fn listing(storage: &VaultStorage, folder: &str) -> Result<Vec<(String, String)>, ServiceError> {
    Ok(storage
        .list_project_files(folder)?
        .into_iter()
        .map(|file| (file.relative_path, file.sha256))
        .collect())
}

fn to_view(record: &ProjectRecord, recovery: Recovery, read_only: bool) -> LatexProject {
    LatexProject {
        project_id: record.project_id.clone(),
        folder: record.root.clone(),
        entry_file_id: record.entry_file_id.clone(),
        files: record
            .files
            .iter()
            .map(|file| LatexProjectFile {
                file_id: file.file_id.clone(),
                path: project_relative(&record.root, &file.path),
                revision: file.revision,
                editable: !read_only && is_project_source(&file.path),
            })
            .collect(),
        recovery: match recovery {
            Recovery::Nothing => "nothing",
            Recovery::Completed => "completed",
            Recovery::Abandoned => "abandoned",
            Recovery::External => "external",
        },
        read_only,
        manifest_hash: manifest_hash(record),
    }
}

/// Identity of a project's complete input: its entry and every file's
/// project-relative path and content hash. Windows names cannot hold a tab or
/// a newline, so the encoding is unambiguous.
pub(crate) fn manifest_hash(record: &ProjectRecord) -> String {
    let entry = record
        .entry()
        .map(|entry| project_relative(&record.root, &entry.path))
        .unwrap_or_default();
    let mut files: Vec<(String, &str)> = record
        .files
        .iter()
        .map(|file| {
            (
                project_relative(&record.root, &file.path),
                file.sha256.as_str(),
            )
        })
        .collect();
    files.sort();
    let mut text = format!("onote-latex-manifest/1\nentry\t{entry}\n");
    for (path, sha) in files {
        text.push_str(&path);
        text.push('\t');
        text.push_str(sha);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

/// The input of one build, written outside the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectSnapshot {
    pub manifest_hash: String,
    /// Entry relative to the snapshot root, `/`-separated.
    pub entry: String,
    pub files: usize,
    pub bytes: u64,
}

/// F51: copies every file of the project, exactly as the record knows it, into
/// the new directory `input`. The caller's `expected_manifest_hash` must match
/// the manifest after syncing with the disk, and every file must still have
/// the hash the record holds; otherwise nothing is built (VERSION_CONFLICT).
pub(crate) fn capture_snapshot(
    storage: &mut VaultStorage,
    project_id: &str,
    expected_manifest_hash: &str,
    input: &Path,
) -> Result<ProjectSnapshot, ServiceError> {
    let (record, _, _, _) = load(storage, project_id)?;
    let manifest = manifest_hash(&record);
    if manifest != expected_manifest_hash {
        return Err(svc_err(
            ErrorCode::VersionConflict,
            "error.buildManifestStale",
        ));
    }
    let entry = record
        .entry()
        .map(|entry| project_relative(&record.root, &entry.path))
        .ok_or_else(|| svc_err(ErrorCode::ObjectNotFound, "error.projectEntryMissing"))?;
    let write_err = |_| svc_err(ErrorCode::IoDenied, "error.buildSnapshotFailed");
    std::fs::create_dir(input).map_err(write_err)?;
    let mut bytes = 0u64;
    for tracked in &record.files {
        let file = storage.read_project_file(&tracked.path)?;
        if file.sha256 != tracked.sha256 {
            return Err(svc_err(
                ErrorCode::VersionConflict,
                "error.buildManifestStale",
            ));
        }
        let relative = project_relative(&record.root, &tracked.path);
        // The storage read already refused anything outside the vault; this
        // keeps a hand-edited record from placing a file outside `input`.
        if relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(svc_err(
                ErrorCode::InvalidInput,
                "error.projectRecordInvalid",
            ));
        }
        let target = relative
            .split('/')
            .fold(input.to_path_buf(), |path, part| path.join(part));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(write_err)?;
        }
        std::fs::write(&target, &file.bytes).map_err(write_err)?;
        bytes += file.bytes.len() as u64;
    }
    Ok(ProjectSnapshot {
        manifest_hash: manifest,
        entry,
        files: record.files.len(),
        bytes,
    })
}

fn encode(record: &ProjectRecord) -> Result<Vec<u8>, ServiceError> {
    serde_json::to_vec_pretty(record)
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.projectRecordWriteFailed"))
}

/// Maps a picked absolute folder to a vault-relative one, refusing anything
/// outside the vault or inside `.opennote`.
pub(crate) fn folder_inside_vault(root: &Path, picked: &Path) -> Result<String, ServiceError> {
    let outside = || svc_err(ErrorCode::InvalidInput, "error.latexFolderOutsideVault");
    let root = std::fs::canonicalize(root).map_err(|_| outside())?;
    let picked = std::fs::canonicalize(picked).map_err(|_| outside())?;
    let relative = picked.strip_prefix(&root).map_err(|_| outside())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => {
                let part = part.to_str().ok_or_else(outside)?;
                if part.eq_ignore_ascii_case(".opennote") {
                    return Err(outside());
                }
                parts.push(part.to_owned());
            }
            _ => return Err(outside()),
        }
    }
    Ok(parts.join("/"))
}

/// F45: inspects a project folder from disk. Never executes anything.
pub(crate) fn inspect_folder(
    storage: &VaultStorage,
    folder: &str,
) -> Result<onote_latex::ProjectInspection, ServiceError> {
    let mut files = Vec::new();
    for entry in storage.list_project_files(folder)? {
        let text =
            if is_project_source(&entry.relative_path) && entry.size <= MAX_INSPECT_TEXT_BYTES {
                let file = storage.read_project_file(&entry.relative_path)?;
                // Invalid UTF-8 is inspected by name only, never decoded lossily.
                String::from_utf8(file.bytes).ok()
            } else {
                None
            };
        files.push(onote_latex::InspectFile {
            path: project_relative(folder, &entry.relative_path),
            bytes: Some(entry.size),
            text,
        });
    }
    Ok(onote_latex::inspect_project(folder, &files))
}

/// F45: registers the folder as a project with `entry_path` (relative to the
/// folder) as its entry. The operation id becomes the project id, so a retry
/// returns the project it already created.
pub(crate) fn register_folder(
    storage: &mut VaultStorage,
    folder: &str,
    entry_path: &str,
    operation_id: &str,
) -> Result<LatexProject, ServiceError> {
    if let Some(existing) = storage.read_project_record(operation_id)? {
        let record: ProjectRecord = serde_json::from_slice(&existing.bytes)
            .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.projectRecordInvalid"))?;
        let same = record.root == folder
            && record
                .entry()
                .is_some_and(|entry| entry.path == vault_relative(folder, entry_path));
        if !same {
            return Err(svc_err(ErrorCode::InvalidInput, "error.operationReused"));
        }
        return open_project(storage, operation_id);
    }
    // One folder is one project; a second registration would give the same
    // files two identities and two save journals.
    let duplicate = registered(storage)?.into_iter().any(|(_, record)| {
        record.is_some_and(|record| {
            onote_vault_storage::compare_windows_paths(&record.root, folder).is_eq()
        })
    });
    if duplicate {
        return Err(svc_err(
            ErrorCode::PathCollision,
            "error.projectAlreadyRegistered",
        ));
    }
    let files = listing(storage, folder)?;
    let record = ProjectRecord::new(
        operation_id,
        folder,
        &files,
        &vault_relative(folder, entry_path),
        new_id,
    )
    .map_err(store_err)?;
    storage.write_project_record(operation_id, &encode(&record)?, None, operation_id)?;
    Ok(to_view(&record, Recovery::Nothing, false))
}

fn decode(bytes: &[u8]) -> Result<ProjectRecord, ServiceError> {
    serde_json::from_slice(bytes)
        .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.projectRecordInvalid"))
}

/// Every stored record; `None` for one this build cannot read.
fn registered(
    storage: &VaultStorage,
) -> Result<Vec<(String, Option<ProjectRecord>)>, ServiceError> {
    Ok(storage
        .list_project_records()?
        .into_iter()
        .map(|(id, stored)| (id, decode(&stored.bytes).ok()))
        .collect())
}

/// One registered project for the project list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexProjectSummary {
    pub project_id: String,
    pub folder: Option<String>,
    /// Entry path relative to the folder, when it still exists.
    pub entry_path: Option<String>,
    /// False when the record cannot be read by this build; it is listed so it
    /// is not silently hidden, and can still be removed.
    pub readable: bool,
}

pub(crate) fn list_projects(
    storage: &VaultStorage,
) -> Result<Vec<LatexProjectSummary>, ServiceError> {
    Ok(registered(storage)?
        .into_iter()
        .map(|(project_id, record)| match record {
            Some(record) => LatexProjectSummary {
                project_id,
                entry_path: record
                    .entry()
                    .map(|entry| project_relative(&record.root, &entry.path)),
                folder: Some(record.root),
                readable: true,
            },
            None => LatexProjectSummary {
                project_id,
                folder: None,
                entry_path: None,
                readable: false,
            },
        })
        .collect())
}

/// Removes the registration only; the project's files stay where they are.
pub(crate) fn unregister(
    storage: &mut VaultStorage,
    project_id: &str,
    operation_id: &str,
) -> Result<(), ServiceError> {
    let Some(stored) = storage.read_project_record(project_id)? else {
        return Ok(());
    };
    storage.remove_project_record(project_id, &stored.sha256, operation_id)
}

/// Unregisters every project whose folder is `folder` or lies inside it, after
/// that folder was deleted. A project already unregistered is skipped, so a
/// retry is harmless; each removal gets its own operation id.
pub(crate) fn unregister_inside(
    storage: &mut VaultStorage,
    folder: &str,
) -> Result<(), ServiceError> {
    let folder = folder.trim_end_matches('/').to_lowercase();
    let inside: Vec<String> = list_projects(storage)?
        .into_iter()
        .filter(|project| {
            project.folder.as_deref().is_some_and(|root| {
                let root = root.trim_end_matches('/').to_lowercase();
                root == folder || root.starts_with(&format!("{folder}/"))
            })
        })
        .map(|project| project.project_id)
        .collect();
    for project_id in &inside {
        unregister(storage, project_id, &uuid::Uuid::new_v4().to_string())?;
    }
    Ok(())
}

/// Paths already in `destination` that would collide with the plan. A planned
/// file already holding exactly its planned bytes is ours from an interrupted
/// attempt with the same request, so it is not a collision.
fn plan_scaffold_in(
    storage: &VaultStorage,
    template: onote_latex::TemplateId,
    metadata: &onote_latex::Metadata,
    destination: &str,
) -> Result<onote_latex::ScaffoldPlan, ServiceError> {
    let draft = onote_latex::plan_scaffold(template, metadata, destination, &[]);
    if !draft.issues.is_empty() {
        return Ok(draft);
    }
    let present = match storage.list_project_files(&draft.destination) {
        Ok(files) => files,
        Err(error) if error.code == ErrorCode::ObjectNotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let existing: Vec<String> = present
        .into_iter()
        .filter(|file| {
            !draft.files.iter().any(|planned| {
                vault_relative(&draft.destination, &planned.path) == file.relative_path
                    && sha256_hex(planned.content.as_bytes()) == file.sha256
            })
        })
        .map(|file| file.relative_path)
        .collect();
    Ok(onote_latex::plan_scaffold(
        template,
        metadata,
        destination,
        &existing,
    ))
}

/// F46: the plan shown before creating; collisions come from the real folder.
pub(crate) fn preview_scaffold(
    storage: &VaultStorage,
    template: onote_latex::TemplateId,
    metadata: &onote_latex::Metadata,
    destination: &str,
) -> Result<onote_latex::ScaffoldPlan, ServiceError> {
    plan_scaffold_in(storage, template, metadata, destination)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexCreateOutcome {
    pub plan: onote_latex::ScaffoldPlan,
    /// Present only when every file was written and the project registered.
    pub project: Option<LatexProject>,
}

/// F46: writes a template into a new folder and registers it. Nothing is
/// written while the plan has an issue, and no existing file is replaced:
/// every file is created only where nothing exists. A retry with the same
/// operation id finishes an interrupted attempt or returns the project.
pub(crate) fn create_from_template(
    storage: &mut VaultStorage,
    template: onote_latex::TemplateId,
    metadata: &onote_latex::Metadata,
    destination: &str,
    operation_id: &str,
) -> Result<LatexCreateOutcome, ServiceError> {
    if let Some(stored) = storage.read_project_record(operation_id)? {
        let record = decode(&stored.bytes)?;
        let plan = onote_latex::plan_scaffold(template, metadata, destination, &[]);
        if record.root != plan.destination {
            return Err(svc_err(ErrorCode::InvalidInput, "error.operationReused"));
        }
        return Ok(LatexCreateOutcome {
            plan,
            project: Some(open_project(storage, operation_id)?),
        });
    }
    let plan = plan_scaffold_in(storage, template, metadata, destination)?;
    if !plan.is_executable() {
        return Ok(LatexCreateOutcome {
            plan,
            project: None,
        });
    }
    for file in &plan.files {
        let bytes = file.content.as_bytes();
        storage.store_attachment(
            &vault_relative(&plan.destination, &file.path),
            bytes,
            &sha256_hex(bytes),
            operation_id,
        )?;
    }
    let project = register_folder(storage, &plan.destination, &plan.entry_file, operation_id)?;
    Ok(LatexCreateOutcome {
        plan,
        project: Some(project),
    })
}

/// Loads a record, settles an interrupted save and syncs it with the disk,
/// persisting any change when the vault is writable.
fn load(
    storage: &mut VaultStorage,
    project_id: &str,
) -> Result<(ProjectRecord, String, Recovery, bool), ServiceError> {
    let stored = storage
        .read_project_record(project_id)?
        .ok_or_else(|| svc_err(ErrorCode::ObjectNotFound, "error.projectMissing"))?;
    let mut record: ProjectRecord = serde_json::from_slice(&stored.bytes)
        .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.projectRecordInvalid"))?;
    if record.project_id != project_id {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.projectRecordInvalid",
        ));
    }
    let read_only =
        !storage.vault().writable || record.format_version > onote_latex::RECORD_FORMAT_VERSION;
    if read_only {
        return Ok((record, stored.sha256, Recovery::Nothing, true));
    }
    let original = record.clone();
    let disk = match record.pending_path() {
        Some(path) => match storage.read_project_file(path) {
            Ok(file) => Some(file.sha256),
            Err(error) if error.code == ErrorCode::ObjectNotFound => None,
            Err(error) => return Err(error),
        },
        None => None,
    };
    let recovery = record.resolve_pending(disk.as_deref());
    let files = listing(storage, &record.root.clone())?;
    record.sync_files(&files, new_id).map_err(store_err)?;
    let mut sha = stored.sha256;
    if record != original {
        sha = storage.write_project_record(project_id, &encode(&record)?, Some(&sha), &new_id())?;
    }
    Ok((record, sha, recovery, false))
}

pub(crate) fn open_project(
    storage: &mut VaultStorage,
    project_id: &str,
) -> Result<LatexProject, ServiceError> {
    let (record, _, recovery, read_only) = load(storage, project_id)?;
    Ok(to_view(&record, recovery, read_only))
}

pub(crate) fn read_source(
    storage: &mut VaultStorage,
    project_id: &str,
    file_id: &str,
) -> Result<LatexProjectSource, ServiceError> {
    let (record, _, _, _) = load(storage, project_id)?;
    let tracked = record
        .file(file_id)
        .ok_or_else(|| store_err(StoreError::UnknownFile))?;
    if !is_project_source(&tracked.path) {
        return Err(svc_err(ErrorCode::InvalidInput, "error.projectNotSource"));
    }
    let file = storage.read_project_file(&tracked.path)?;
    if file.sha256 != tracked.sha256 {
        // Changed between the sync and this read; the caller reopens.
        return Err(svc_err(
            ErrorCode::VersionConflict,
            "error.projectFileChanged",
        ));
    }
    let text = String::from_utf8(file.bytes)
        .map_err(|_| svc_err(ErrorCode::InvalidInput, "error.projectEncoding"))?;
    Ok(LatexProjectSource {
        project_id: project_id.to_owned(),
        file_id: file_id.to_owned(),
        path: project_relative(&record.root, &tracked.path),
        text,
        revision: tracked.revision,
    })
}

pub(crate) fn save_source(
    storage: &mut VaultStorage,
    project_id: &str,
    file_id: &str,
    text: &str,
    expected_revision: u64,
    operation_id: &str,
) -> Result<LatexSaveResult, ServiceError> {
    let (record, record_sha, _, read_only) = load(storage, project_id)?;
    if read_only {
        return Err(svc_err(ErrorCode::InvalidInput, "error.vaultReadOnly"));
    }
    let bytes = text.as_bytes();
    let step = record
        .begin_save(file_id, expected_revision, &sha256_hex(bytes), operation_id)
        .map_err(store_err)?;
    let (journaled, path, expected_disk) = match step {
        SaveStep::AlreadyDone(receipt) => {
            return Ok(LatexSaveResult {
                file_id: receipt.file_id,
                revision: receipt.revision,
                written: false,
                manifest_hash: manifest_hash(&record),
            })
        }
        SaveStep::Unchanged(file) => {
            return Ok(LatexSaveResult {
                file_id: file.file_id,
                revision: file.revision,
                written: false,
                manifest_hash: manifest_hash(&record),
            })
        }
        SaveStep::Write {
            record,
            path,
            expected_disk_sha256,
        } => (record, path, expected_disk_sha256),
    };
    // 1. Journal first, so a stop after the source write is recoverable.
    let journal_sha = storage.write_project_record(
        project_id,
        &encode(&journaled)?,
        Some(&record_sha),
        &new_id(),
    )?;
    let mut finished = journaled;
    // 2. The source itself, against the hash the record knows.
    match storage.save_project_source(&path, bytes, Some(&expected_disk), operation_id) {
        Ok(_) => {
            let receipt = finished
                .complete_save()
                .ok_or_else(|| svc_err(ErrorCode::IoDenied, "error.projectRecordWriteFailed"))?;
            // 3. If this write fails, the next open completes it from disk.
            storage.write_project_record(
                project_id,
                &encode(&finished)?,
                Some(&journal_sha),
                &new_id(),
            )?;
            Ok(LatexSaveResult {
                file_id: receipt.file_id,
                revision: receipt.revision,
                written: true,
                manifest_hash: manifest_hash(&finished),
            })
        }
        Err(error) => {
            finished.abandon_save();
            // Best effort: if this fails, the next open abandons it from disk.
            let _ = storage.write_project_record(
                project_id,
                &encode(&finished)?,
                Some(&journal_sha),
                &new_id(),
            );
            Err(error)
        }
    }
}

fn selected_folder(
    app: &AppHandle,
    vault_id: &str,
    selection_id: &str,
) -> Result<String, ServiceError> {
    let selections = app.state::<LatexFolderSelections>();
    let selections = selections.0.lock().map_err(|_| poison_err())?;
    selections
        .get(selection_id)
        .filter(|(vault, _)| vault == vault_id)
        .map(|(_, folder)| folder.clone())
        .ok_or_else(|| svc_err(ErrorCode::InvalidInput, "error.latexSelectionInvalid"))
}

/// F45: native folder picker, starting in the vault. Only a folder inside the
/// vault is accepted; the frontend gets a selection id, not a path to submit.
#[tauri::command]
pub(crate) async fn select_latex_project_folder(
    app: AppHandle,
    vault_id: String,
    dialog_title: Option<String>,
) -> Result<Option<ChosenLatexFolder>, ServiceError> {
    let root = with_state(&app, |state| {
        Ok(scoped_vault_ref(state, &vault_id)?
            .root_path()
            .to_path_buf())
    })?;
    let title = localized_or_fallback(dialog_title, "Choose LaTeX project folder");
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = handle
            .dialog()
            .file()
            .set_title(&title)
            .set_directory(&root);
        if let Some(window) = handle.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_pick_folder().and_then(file_path_to_buf) else {
            return Ok(None);
        };
        let folder = folder_inside_vault(&root, &picked)?;
        let selection_id = new_id();
        handle
            .state::<LatexFolderSelections>()
            .0
            .lock()
            .map_err(|_| poison_err())?
            .insert(selection_id.clone(), (vault_id, folder.clone()));
        Ok(Some(ChosenLatexFolder {
            selection_id,
            folder,
        }))
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
pub(crate) fn inspect_latex_folder(
    app: AppHandle,
    vault_id: String,
    selection_id: String,
) -> Result<onote_latex::ProjectInspection, ServiceError> {
    let folder = selected_folder(&app, &vault_id, &selection_id)?;
    with_state(&app, |state| {
        inspect_folder(scoped_vault_ref(state, &vault_id)?, &folder)
    })
}

#[tauri::command]
pub(crate) fn register_latex_project(
    app: AppHandle,
    vault_id: String,
    selection_id: String,
    entry_path: String,
    operation_id: String,
) -> Result<LatexProject, ServiceError> {
    let folder = selected_folder(&app, &vault_id, &selection_id)?;
    with_state(&app, |state| {
        register_folder(
            scoped_vault_mut(state, &vault_id)?,
            &folder,
            &entry_path,
            &operation_id,
        )
    })
}

#[tauri::command]
pub(crate) fn open_latex_project(
    app: AppHandle,
    vault_id: String,
    project_id: String,
) -> Result<LatexProject, ServiceError> {
    with_state(&app, |state| {
        open_project(scoped_vault_mut(state, &vault_id)?, &project_id)
    })
}

#[tauri::command]
pub(crate) fn read_latex_project_file(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    file_id: String,
) -> Result<LatexProjectSource, ServiceError> {
    with_state(&app, |state| {
        read_source(scoped_vault_mut(state, &vault_id)?, &project_id, &file_id)
    })
}

#[tauri::command]
pub(crate) fn save_latex_project_file(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    file_id: String,
    text: String,
    expected_revision: u64,
    operation_id: String,
) -> Result<LatexSaveResult, ServiceError> {
    with_state(&app, |state| {
        save_source(
            scoped_vault_mut(state, &vault_id)?,
            &project_id,
            &file_id,
            &text,
            expected_revision,
            &operation_id,
        )
    })
}

#[tauri::command]
pub(crate) fn list_latex_projects(
    app: AppHandle,
    vault_id: String,
) -> Result<Vec<LatexProjectSummary>, ServiceError> {
    with_state(&app, |state| {
        list_projects(scoped_vault_ref(state, &vault_id)?)
    })
}

#[tauri::command]
pub(crate) fn unregister_latex_project(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    operation_id: String,
) -> Result<(), ServiceError> {
    with_state(&app, |state| {
        unregister(
            scoped_vault_mut(state, &vault_id)?,
            &project_id,
            &operation_id,
        )
    })
}

#[tauri::command]
pub(crate) fn create_latex_project(
    app: AppHandle,
    vault_id: String,
    template_id: String,
    metadata: onote_latex::Metadata,
    destination: String,
    operation_id: String,
) -> Result<LatexCreateOutcome, ServiceError> {
    let Some(template) = onote_latex::TemplateId::parse(&template_id) else {
        return Err(svc_err(ErrorCode::InvalidInput, "error.templateUnknown"));
    };
    with_state(&app, |state| {
        create_from_template(
            scoped_vault_mut(state, &vault_id)?,
            template,
            &metadata,
            &destination,
            &operation_id,
        )
    })
}

/// A source ZIP larger than this is refused rather than built in memory; the
/// stored format also cannot describe an archive past 4 GiB.
const SOURCE_ZIP_LIMIT: u64 = 1024 * 1024 * 1024;

/// F54-A03: every file of the project as the record knows it, by path relative
/// to the project folder, plus a file name for the archive. A file changed on
/// disk since the record synced is refused, so the ZIP is one consistent state.
pub(crate) fn source_files(
    storage: &mut VaultStorage,
    project_id: &str,
) -> Result<(String, Vec<(String, Vec<u8>)>), ServiceError> {
    let (record, _, _, _) = load(storage, project_id)?;
    let mut files = Vec::with_capacity(record.files.len());
    let mut total = 0u64;
    for tracked in &record.files {
        let relative = project_relative(&record.root, &tracked.path);
        if relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(svc_err(
                ErrorCode::InvalidInput,
                "error.projectRecordInvalid",
            ));
        }
        let file = storage.read_project_file(&tracked.path)?;
        if file.sha256 != tracked.sha256 {
            return Err(svc_err(
                ErrorCode::VersionConflict,
                "error.projectFileChanged",
            ));
        }
        total += file.bytes.len() as u64;
        if total > SOURCE_ZIP_LIMIT {
            return Err(svc_err(ErrorCode::InvalidInput, "error.latexZipTooLarge"));
        }
        files.push((relative, file.bytes));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let name = record
        .root
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("latex-project");
    Ok((format!("{name}.zip"), files))
}

/// Packs the files as a stored ZIP with relative names only, and reads the
/// archive back against its manifest before anything is written.
pub(crate) fn source_zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, ServiceError> {
    let mut writer = onote_attachments_import::ZipWriter::new();
    for (path, bytes) in files {
        writer.add(path, bytes);
    }
    let manifest = writer.manifest();
    let bytes = writer.finish();
    onote_attachments_import::verify_zip(&bytes, &manifest)
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.exportVerifyFailed"))?;
    Ok(bytes)
}

/// F54-A03: saves the whole project as a ZIP another TeX tool can open. The
/// archive holds the project folder's files only; build output lives outside
/// the vault and is not part of it.
#[tauri::command]
pub(crate) async fn export_latex_source_zip(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    dialog_title: Option<String>,
) -> Result<bool, ServiceError> {
    let (name, files) = with_state(&app, |state| {
        source_files(scoped_vault_mut(state, &vault_id)?, &project_id)
    })?;
    let title = localized_or_fallback(dialog_title, "Export source ZIP");
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = source_zip(&files)?;
        save_with_dialog(&handle, &bytes, &name, &title, ("ZIP", &["zip"]))
    })
    .await
    .map_err(map_join_err)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, PathBuf, VaultStorage) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        std::fs::create_dir_all(root.join("thesis/chapters")).unwrap();
        std::fs::write(
            root.join("thesis/main.tex"),
            "\\documentclass{article}\n\\begin{document}\\input{chapters/one}\\end{document}\n",
        )
        .unwrap();
        std::fs::write(root.join("thesis/chapters/one.tex"), "One.\n").unwrap();
        std::fs::write(root.join("thesis/fig.png"), [0x89, 0x50]).unwrap();
        let storage = VaultStorage::create(&root, "Tex").unwrap();
        (directory, root, storage)
    }

    fn file_id(project: &LatexProject, path: &str) -> String {
        project
            .files
            .iter()
            .find(|file| file.path == path)
            .unwrap()
            .file_id
            .clone()
    }

    fn record_path(root: &Path, project_id: &str) -> PathBuf {
        root.join(".opennote/latex")
            .join(project_id)
            .join("project.json")
    }

    #[test]
    fn the_source_zip_holds_every_project_file_by_relative_path_only() {
        let (_d, root, mut storage) = vault();
        let op = new_id();
        register_folder(&mut storage, "thesis", "main.tex", &op).unwrap();
        // A file added after registration is part of the folder, so it is exported.
        std::fs::write(root.join("thesis/refs.bib"), "@book{a,title={A}}\n").unwrap();

        let (name, files) = source_files(&mut storage, &op).unwrap();
        assert_eq!(name, "thesis.zip");
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            ["chapters/one.tex", "fig.png", "main.tex", "refs.bib"]
        );
        assert_eq!(
            files.iter().find(|(path, _)| path == "fig.png").unwrap().1,
            [0x89, 0x50]
        );

        let zip = source_zip(&files).unwrap();
        let text = String::from_utf8_lossy(&zip);
        assert!(text.contains("chapters/one.tex"));
        assert!(!text.contains("thesis/"), "no project folder prefix");
        let root_text = root.to_string_lossy().replace('\\', "/");
        assert!(
            !text.replace('\\', "/").contains(&root_text),
            "no absolute path"
        );
        // The OpenNote record never leaks into the archive.
        assert!(!text.contains(".opennote"));
    }

    #[test]
    fn a_vault_root_project_gets_a_neutral_archive_name() {
        let (_d, root, mut storage) = vault();
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\n").unwrap();
        let op = new_id();
        register_folder(&mut storage, "", "main.tex", &op).unwrap();
        let (name, files) = source_files(&mut storage, &op).unwrap();
        assert_eq!(name, "latex-project.zip");
        assert!(files.iter().all(|(path, _)| !path.starts_with(".opennote")));
    }

    #[test]
    fn a_picked_folder_must_be_inside_the_vault() {
        let (directory, root, _storage) = vault();
        assert_eq!(
            folder_inside_vault(&root, &root.join("thesis")).unwrap(),
            "thesis"
        );
        assert_eq!(folder_inside_vault(&root, &root).unwrap(), "");
        assert!(folder_inside_vault(&root, directory.path()).is_err());
        assert!(folder_inside_vault(&root, &root.join(".opennote")).is_err());
    }

    #[test]
    fn inspection_reads_the_disk_and_finds_the_entry() {
        let (_d, _root, storage) = vault();
        let inspection = inspect_folder(&storage, "thesis").unwrap();
        assert_eq!(inspection.entry.as_deref(), Some("main.tex"));
        assert!(inspection.files.contains(&"chapters/one.tex".to_owned()));
        assert!(inspection.files.contains(&"fig.png".to_owned()));
    }

    #[test]
    fn register_open_read_and_save_round_trip_without_touching_the_source_format() {
        let (_d, root, mut storage) = vault();
        let op = new_id();
        let project = register_folder(&mut storage, "thesis", "main.tex", &op).unwrap();
        assert_eq!(project.project_id, op);
        assert!(record_path(&root, &op).is_file());
        // A retried registration returns the same project.
        let again = register_folder(&mut storage, "thesis", "main.tex", &op).unwrap();
        assert_eq!(again.entry_file_id, project.entry_file_id);
        assert!(
            !project
                .files
                .iter()
                .find(|f| f.path == "fig.png")
                .unwrap()
                .editable
        );

        let chapter = file_id(&project, "chapters/one.tex");
        let source = read_source(&mut storage, &op, &chapter).unwrap();
        assert_eq!((source.text.as_str(), source.revision), ("One.\n", 1));

        let save_op = new_id();
        let saved = save_source(&mut storage, &op, &chapter, "Uno.\n", 1, &save_op).unwrap();
        assert_eq!((saved.revision, saved.written), (2, true));
        // `.tex` gets exactly the bytes, no YAML or identity inserted.
        assert_eq!(
            std::fs::read(root.join("thesis/chapters/one.tex")).unwrap(),
            b"Uno.\n"
        );

        // A lost reply retried with the same operation is answered, not re-applied.
        let retry = save_source(&mut storage, &op, &chapter, "Uno.\n", 1, &save_op).unwrap();
        assert_eq!((retry.revision, retry.written), (2, false));
        // A stale buffer is refused.
        let stale = save_source(&mut storage, &op, &chapter, "Old.\n", 1, &new_id()).unwrap_err();
        assert_eq!(stale.code, ErrorCode::VersionConflict);
        assert_eq!(
            std::fs::read(root.join("thesis/chapters/one.tex")).unwrap(),
            b"Uno.\n"
        );
    }

    #[test]
    fn an_external_edit_raises_the_revision_and_is_never_overwritten() {
        let (_d, root, mut storage) = vault();
        let op = new_id();
        let project = register_folder(&mut storage, "thesis", "main.tex", &op).unwrap();
        let chapter = file_id(&project, "chapters/one.tex");
        std::fs::write(root.join("thesis/chapters/one.tex"), "Theirs.\n").unwrap();
        let error = save_source(&mut storage, &op, &chapter, "Mine.\n", 1, &new_id()).unwrap_err();
        assert_eq!(error.code, ErrorCode::VersionConflict);
        assert_eq!(
            std::fs::read(root.join("thesis/chapters/one.tex")).unwrap(),
            b"Theirs.\n"
        );
        let reopened = open_project(&mut storage, &op).unwrap();
        let file = reopened
            .files
            .iter()
            .find(|f| f.file_id == chapter)
            .unwrap();
        assert_eq!(file.revision, 2);
    }

    #[test]
    fn a_save_interrupted_after_the_source_write_completes_on_reopen() {
        let (_d, root, mut storage) = vault();
        let op = new_id();
        let project = register_folder(&mut storage, "thesis", "main.tex", &op).unwrap();
        let chapter = file_id(&project, "chapters/one.tex");
        // Simulate a stop after steps 1 and 2: journaled record + new source.
        let stored = storage.read_project_record(&op).unwrap().unwrap();
        let record: ProjectRecord = serde_json::from_slice(&stored.bytes).unwrap();
        let save_op = new_id();
        let SaveStep::Write {
            record: journaled, ..
        } = record
            .begin_save(&chapter, 1, &sha256_hex(b"Uno.\n"), &save_op)
            .unwrap()
        else {
            panic!("expected a write");
        };
        storage
            .write_project_record(
                &op,
                &encode(&journaled).unwrap(),
                Some(&stored.sha256),
                &new_id(),
            )
            .unwrap();
        std::fs::write(root.join("thesis/chapters/one.tex"), "Uno.\n").unwrap();

        let reopened = open_project(&mut storage, &op).unwrap();
        assert_eq!(reopened.recovery, "completed");
        assert_eq!(
            reopened
                .files
                .iter()
                .find(|f| f.file_id == chapter)
                .unwrap()
                .revision,
            2
        );
        // The original request, retried, is recognised as done.
        let retry = save_source(&mut storage, &op, &chapter, "Uno.\n", 1, &save_op).unwrap();
        assert_eq!((retry.revision, retry.written), (2, false));
    }

    #[test]
    fn binary_files_and_unknown_ids_are_not_sources() {
        let (_d, _root, mut storage) = vault();
        let op = new_id();
        let project = register_folder(&mut storage, "thesis", "main.tex", &op).unwrap();
        let image = file_id(&project, "fig.png");
        assert!(read_source(&mut storage, &op, &image).is_err());
        assert!(save_source(&mut storage, &op, &image, "x", 1, &new_id()).is_err());
        assert_eq!(
            read_source(&mut storage, &op, "missing").unwrap_err().code,
            ErrorCode::ObjectNotFound
        );
        assert!(register_folder(&mut storage, "thesis", "fig.png", &new_id()).is_err());
    }

    #[test]
    fn a_folder_is_registered_once_and_removal_keeps_its_files() {
        let (_d, root, mut storage) = vault();
        let op = new_id();
        register_folder(&mut storage, "thesis", "main.tex", &op).unwrap();
        // Same folder, different spelling of its case, new operation: refused.
        let error = register_folder(&mut storage, "THESIS", "main.tex", &new_id()).unwrap_err();
        assert_eq!(error.code, ErrorCode::PathCollision);
        assert_eq!(error.message_key, "error.projectAlreadyRegistered");

        let listed = list_projects(&storage).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].folder.as_deref(), Some("thesis"));
        assert_eq!(listed[0].entry_path.as_deref(), Some("main.tex"));

        let before = std::fs::read(root.join("thesis/main.tex")).unwrap();
        unregister(&mut storage, &op, &new_id()).unwrap();
        unregister(&mut storage, &op, &new_id()).unwrap();
        assert!(list_projects(&storage).unwrap().is_empty());
        assert_eq!(std::fs::read(root.join("thesis/main.tex")).unwrap(), before);
        // Once removed, the folder can be registered again.
        register_folder(&mut storage, "thesis", "main.tex", &new_id()).unwrap();
    }

    #[test]
    fn an_unreadable_record_is_listed_not_hidden() {
        let (_d, root, storage) = vault();
        let id = new_id();
        let directory = root.join(".opennote/latex").join(&id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("project.json"), b"not json").unwrap();
        let listed = list_projects(&storage).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].readable);
    }

    fn metadata() -> onote_latex::Metadata {
        onote_latex::Metadata {
            title: "Kyoto & Co".to_owned(),
            author: "A".to_owned(),
            date: "2026-10-30".to_owned(),
            language: onote_latex::DocumentLanguage::En,
            paper: onote_latex::PaperSize::A4,
        }
    }

    #[test]
    fn a_template_is_written_registered_and_retried_safely() {
        let (_d, root, mut storage) = vault();
        let op = new_id();
        let template = onote_latex::TemplateId::Report;
        let created =
            create_from_template(&mut storage, template, &metadata(), "papers/kyoto", &op).unwrap();
        let project = created.project.expect("registered");
        assert_eq!(project.folder, "papers/kyoto");
        let main = std::fs::read_to_string(root.join("papers/kyoto/main.tex")).unwrap();
        assert!(main.contains("Kyoto \\& Co"), "metadata is escaped");
        assert!(!main.starts_with("---"), "no YAML is written into a source");
        assert!(root.join("papers/kyoto/references.bib").is_file());
        assert_eq!(
            project.entry_file_id,
            file_id(&project, "main.tex"),
            "the scaffold entry is the project entry"
        );

        // A lost reply retried with the same operation returns the same project.
        let again =
            create_from_template(&mut storage, template, &metadata(), "papers/kyoto", &op).unwrap();
        assert_eq!(again.project.unwrap().project_id, project.project_id);
        assert_eq!(list_projects(&storage).unwrap().len(), 1);
    }

    #[test]
    fn a_template_never_replaces_an_existing_file() {
        let (_d, root, mut storage) = vault();
        std::fs::create_dir_all(root.join("taken")).unwrap();
        std::fs::write(root.join("taken/main.tex"), "mine").unwrap();
        let created = create_from_template(
            &mut storage,
            onote_latex::TemplateId::Paper,
            &metadata(),
            "taken",
            &new_id(),
        )
        .unwrap();
        assert!(created.project.is_none());
        assert!(created
            .plan
            .issues
            .iter()
            .any(|issue| matches!(issue, onote_latex::PlanIssue::Collision { path } if path == "taken/main.tex")));
        assert_eq!(
            std::fs::read_to_string(root.join("taken/main.tex")).unwrap(),
            "mine"
        );
        assert!(
            !root.join("taken/references.bib").exists(),
            "nothing is written"
        );
        // The preview reports the same collision from the real folder.
        let preview = preview_scaffold(
            &storage,
            onote_latex::TemplateId::Paper,
            &metadata(),
            "taken",
        )
        .unwrap();
        assert!(!preview.is_executable());
    }

    #[test]
    fn findings_and_plan_issues_reach_the_ui_tagged_by_kind() {
        // The TS contract reads `kind`; an externally tagged enum would arrive
        // as `{"entryAmbiguous": {...}}` and be rendered as an unknown finding.
        let finding = serde_json::to_value(onote_latex::ProjectFinding::EntryAmbiguous {
            candidates: vec!["a.tex".to_owned()],
        })
        .unwrap();
        assert_eq!(
            finding,
            serde_json::json!({"kind": "entryAmbiguous", "candidates": ["a.tex"]})
        );
        assert_eq!(
            serde_json::to_value(onote_latex::ProjectFinding::NoEntry).unwrap(),
            serde_json::json!({"kind": "noEntry"})
        );
        let issue = serde_json::to_value(onote_latex::PlanIssue::InvalidMetadata {
            error: onote_latex::MetadataError::InvalidDate {
                value: "2026-02-30".to_owned(),
            },
        })
        .unwrap();
        assert_eq!(
            issue,
            serde_json::json!({"kind": "invalidMetadata", "error": {"kind": "invalidDate", "value": "2026-02-30"}})
        );
        assert_eq!(
            serde_json::to_value(onote_latex::PlanIssue::UnknownTemplate {
                template_id: "x".to_owned()
            })
            .unwrap(),
            serde_json::json!({"kind": "unknownTemplate", "templateId": "x"})
        );
    }

    #[test]
    fn an_interrupted_template_write_is_finished_by_a_retry() {
        let (_d, root, mut storage) = vault();
        let op = new_id();
        let template = onote_latex::TemplateId::Assignment;
        // First attempt stopped after writing one planned file.
        let plan = onote_latex::plan_scaffold(template, &metadata(), "hw", &[]);
        let main = plan
            .files
            .iter()
            .find(|file| file.path == "main.tex")
            .unwrap();
        std::fs::create_dir_all(root.join("hw")).unwrap();
        std::fs::write(root.join("hw/main.tex"), &main.content).unwrap();
        let created = create_from_template(&mut storage, template, &metadata(), "hw", &op).unwrap();
        assert!(created.project.is_some(), "{:?}", created.plan.issues);
        assert!(root.join("hw/sections/questions.tex").is_file());
    }

    #[test]
    fn a_snapshot_copies_every_project_file_and_matches_the_shown_manifest() {
        let (directory, _root, mut storage) = vault();
        let project = register_folder(&mut storage, "thesis", "main.tex", &new_id()).unwrap();
        let input = directory.path().join("snapshot");
        let snapshot = capture_snapshot(
            &mut storage,
            &project.project_id,
            &project.manifest_hash,
            &input,
        )
        .unwrap();
        assert_eq!(snapshot.manifest_hash, project.manifest_hash);
        assert_eq!(snapshot.entry, "main.tex");
        assert_eq!(snapshot.files, 3);
        assert_eq!(
            std::fs::read(input.join("chapters/one.tex")).unwrap(),
            b"One.\n"
        );
        assert_eq!(std::fs::read(input.join("fig.png")).unwrap(), [0x89, 0x50]);
    }

    #[test]
    fn a_snapshot_of_a_project_changed_since_it_was_shown_is_refused() {
        let (directory, root, mut storage) = vault();
        let project = register_folder(&mut storage, "thesis", "main.tex", &new_id()).unwrap();
        // Changed outside OpenNote after the workspace read the manifest.
        std::fs::write(root.join("thesis/chapters/one.tex"), "Changed.\n").unwrap();
        let input = directory.path().join("snapshot");
        let error = capture_snapshot(
            &mut storage,
            &project.project_id,
            &project.manifest_hash,
            &input,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::VersionConflict);
        assert_eq!(error.message_key, "error.buildManifestStale");
        assert!(!input.exists());
        // With the manifest after reopening, the new bytes are built.
        let reopened = open_project(&mut storage, &project.project_id).unwrap();
        assert_ne!(reopened.manifest_hash, project.manifest_hash);
        capture_snapshot(
            &mut storage,
            &project.project_id,
            &reopened.manifest_hash,
            &input,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(input.join("chapters/one.tex")).unwrap(),
            b"Changed.\n"
        );
    }

    #[test]
    fn a_save_reports_the_manifest_the_next_build_must_name() {
        let (directory, _root, mut storage) = vault();
        let project = register_folder(&mut storage, "thesis", "main.tex", &new_id()).unwrap();
        let chapter = file_id(&project, "chapters/one.tex");
        let revision = project
            .files
            .iter()
            .find(|file| file.file_id == chapter)
            .unwrap()
            .revision;
        let saved = save_source(
            &mut storage,
            &project.project_id,
            &chapter,
            "Two.\n",
            revision,
            &new_id(),
        )
        .unwrap();
        assert!(saved.written);
        assert_ne!(saved.manifest_hash, project.manifest_hash);
        assert_eq!(
            open_project(&mut storage, &project.project_id)
                .unwrap()
                .manifest_hash,
            saved.manifest_hash
        );
        let input = directory.path().join("snapshot");
        capture_snapshot(
            &mut storage,
            &project.project_id,
            &saved.manifest_hash,
            &input,
        )
        .unwrap();
    }
}
