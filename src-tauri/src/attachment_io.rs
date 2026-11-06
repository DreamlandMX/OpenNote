//! F19/F20: attachment bytes across IPC.
//!
//! The webview never receives a file path or a filesystem scope. It asks for a
//! vault-relative path, the storage layer applies its path rules, and the bytes
//! come back in bounded chunks. The media type is detected from the bytes, so a
//! renamed file cannot pass as an image.

use super::*;
use onote_vault_storage::{AttachmentReader, MAX_ATTACHMENT_CHUNK};

/// What the webview may know about one attachment before reading it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub relative_path: String,
    pub size: u64,
    pub sha256: String,
    /// Detected from the content; `None` when the type is not one the app shows.
    pub media_type: Option<&'static str>,
}

/// Largest pasted or dropped file. Larger files still work through the picker.
const MAX_INLINE_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;

fn reader(app: &AppHandle, vault_id: &str) -> Result<AttachmentReader, ServiceError> {
    with_state(app, |state| {
        Ok(scoped_vault_ref(state, vault_id)?.attachment_reader())
    })
}

#[tauri::command]
pub(crate) async fn attachment_info(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<AttachmentInfo, ServiceError> {
    let reader = reader(&app, &vault_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let probe = reader.probe(&relative_path)?;
        let media_type =
            onote_attachments_import::detect_media_type(&probe.file.relative_path, &probe.head);
        Ok(AttachmentInfo {
            relative_path: probe.file.relative_path,
            size: probe.file.size,
            sha256: probe.file.sha256,
            media_type,
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
pub(crate) async fn read_attachment(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
    offset: u64,
    length: u64,
) -> Result<tauri::ipc::Response, ServiceError> {
    let reader = reader(&app, &vault_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        reader
            .read_chunk(&relative_path, offset, length.min(MAX_ATTACHMENT_CHUNK))
            .map(tauri::ipc::Response::new)
    })
    .await
    .map_err(map_join_err)?
}

/// F20: every PDF in the vault, by path.
#[tauri::command]
pub(crate) async fn list_vault_pdfs(
    app: AppHandle,
    vault_id: String,
) -> Result<Vec<String>, ServiceError> {
    with_state(&app, |state| {
        Ok(scoped_vault_ref(state, &vault_id)?
            .list_documents(".pdf")?
            .into_iter()
            .map(|file| file.relative_path)
            .collect())
    })
}

/// F20: hands a PDF to the system's default app, for one this app cannot read.
/// Only bytes that really are a PDF go out: a file named `.pdf` holding a program
/// is refused, and nothing else in the vault can be launched this way.
#[tauri::command]
pub(crate) async fn open_attachment_with_system(
    app: AppHandle,
    vault_id: String,
    relative_path: String,
) -> Result<(), ServiceError> {
    let (reader, root) = with_state(&app, |state| {
        let storage = scoped_vault_ref(state, &vault_id)?;
        Ok((
            storage.attachment_reader(),
            storage.root_path().to_path_buf(),
        ))
    })?;
    tauri::async_runtime::spawn_blocking(move || {
        let probe = reader.probe(&relative_path)?;
        let is_pdf = probe
            .file
            .relative_path
            .to_ascii_lowercase()
            .ends_with(".pdf")
            && onote_attachments_import::detect_media_type(&probe.file.relative_path, &probe.head)
                == Some("application/pdf");
        if !is_pdf {
            return Err(svc_err(
                ErrorCode::InvalidInput,
                "error.attachmentSourceInvalid",
            ));
        }
        open_with_shell(&root.join(&probe.file.relative_path))
    })
    .await
    .map_err(map_join_err)?
}

#[cfg(windows)]
fn open_with_shell(path: &Path) -> Result<(), ServiceError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |text: &std::ffi::OsStr| -> Vec<u16> { text.encode_wide().chain(Some(0)).collect() };
    let file = wide(path.as_os_str());
    let verb = wide(std::ffi::OsStr::new("open"));
    // SAFETY: both strings are NUL-terminated and outlive the call; the other
    // pointers are documented as optional.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result as isize > 32 {
        Ok(())
    } else {
        Err(svc_err(ErrorCode::IoDenied, "error.attachmentOpenFailed"))
    }
}

#[cfg(not(windows))]
fn open_with_shell(_path: &Path) -> Result<(), ServiceError> {
    Err(svc_err(ErrorCode::IoDenied, "error.attachmentOpenFailed"))
}

/// The outcome of saving an exported document.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDocument {
    pub cancelled: bool,
}

/// Largest exported document: a note with its embedded images.
const MAX_NOTE_DOCUMENT_BYTES: usize = 512 * 1024 * 1024;

/// Writes beside the target first, so a failed write leaves whatever was there.
/// The save dialog has already asked before replacing an existing file.
fn write_document(target: &Path, text: &str) -> Result<(), ServiceError> {
    let mut staging = target.as_os_str().to_owned();
    staging.push(format!(".{}.partial", uuid::Uuid::new_v4()));
    let staging = PathBuf::from(staging);
    let written = std::fs::write(&staging, text).and_then(|()| std::fs::rename(&staging, target));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        return Err(map_io_export(&error));
    }
    Ok(())
}

/// F25: saves one note as a standalone document where the user picks.
#[tauri::command]
pub(crate) async fn save_note_html(
    app: AppHandle,
    vault_id: String,
    suggested_name: String,
    html: String,
    dialog_title: String,
) -> Result<SavedDocument, ServiceError> {
    with_state(&app, |state| scoped_vault_ref(state, &vault_id).map(|_| ()))?;
    if html.len() > MAX_NOTE_DOCUMENT_BYTES {
        return Err(svc_err(ErrorCode::InvalidInput, "error.attachmentTooLarge"));
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = handle
            .dialog()
            .file()
            .set_title(&dialog_title)
            .set_file_name(&suggested_name)
            .add_filter("HTML", &["html"]);
        if let Some(window) = handle.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_save_file() else {
            return Ok(SavedDocument { cancelled: true });
        };
        let target = file_path_to_buf(picked)
            .ok_or_else(|| svc_err(ErrorCode::InvalidInput, "error.exportTargetInvalid"))?;
        write_document(&target, &html)?;
        Ok(SavedDocument { cancelled: false })
    })
    .await
    .map_err(map_join_err)?
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreRequest {
    vault_id: String,
    destination: String,
    name: String,
    operation_id: String,
}

/// Splits `[u32 little-endian JSON length][JSON][file bytes]`, the raw IPC body.
fn split_store_body(body: &[u8]) -> Result<(StoreRequest, &[u8]), ServiceError> {
    let malformed = || svc_err(ErrorCode::InvalidInput, "error.attachmentSourceInvalid");
    let (length, rest) = body.split_first_chunk::<4>().ok_or_else(malformed)?;
    let length = u32::from_le_bytes(*length) as usize;
    if length > rest.len() {
        return Err(malformed());
    }
    let (header, bytes) = rest.split_at(length);
    let request = serde_json::from_slice(header).map_err(|_| malformed())?;
    Ok((request, bytes))
}

/// Plans and writes one file that arrived as bytes.
///
/// `existing` lists the destination folder. The same planner as the picker
/// decides the final name, so a name holding different bytes is never replaced
/// and identical bytes are reused.
fn store_bytes(
    request: &StoreRequest,
    bytes: &[u8],
    existing: &[(String, String)],
    mut store: impl FnMut(
        &str,
        &[u8],
        &str,
        &str,
    ) -> Result<onote_vault_storage::AttachmentWrite, ServiceError>,
) -> Result<AttachmentImportItem, ServiceError> {
    if bytes.len() as u64 > MAX_INLINE_ATTACHMENT_BYTES {
        return Err(svc_err(ErrorCode::InvalidInput, "error.attachmentTooLarge"));
    }
    let input = onote_attachments_import::AttachmentInput {
        name: request.name.clone(),
        source_path: request.name.clone(),
        size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    };
    let plan = onote_attachments_import::plan_attachments(
        std::slice::from_ref(&input),
        &request.destination,
        existing,
    )
    .map_err(|issue| {
        svc_err(
            ErrorCode::InvalidInput,
            onote_attachments_import::attachment_issue_key(&issue),
        )
    })?;
    let planned = plan
        .attachments
        .first()
        .ok_or_else(|| svc_err(ErrorCode::InvalidInput, "error.attachmentSourceInvalid"))?;
    let mut item = AttachmentImportItem {
        name: request.name.clone(),
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
        return Ok(item);
    }
    if !planned.stores_bytes {
        item.status = "alreadyPresent".to_owned();
        return Ok(item);
    }
    let item_id = onote_attachments_import::note_operation_id(
        &request.operation_id,
        &format!("attachment:{}", request.name),
    );
    match store(&planned.destination, bytes, &planned.sha256, &item_id) {
        Ok(onote_vault_storage::AttachmentWrite::Stored) => item.status = "stored".to_owned(),
        Ok(onote_vault_storage::AttachmentWrite::AlreadyPresent) => {
            item.status = "alreadyPresent".to_owned()
        }
        Err(error) => {
            item.status = "failed".to_owned();
            item.error_key = Some(error.message_key);
        }
    }
    Ok(item)
}

/// F19: stores a pasted or dropped file. The body is raw bytes, so a large image
/// is not turned into a JSON number array on its way here.
#[tauri::command]
pub(crate) async fn store_attachment_bytes(
    app: AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<AttachmentImportItem, ServiceError> {
    let tauri::ipc::InvokeBody::Raw(body) = request.body() else {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.attachmentSourceInvalid",
        ));
    };
    let body = body.clone();
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (request, bytes) = split_store_body(&body)?;
        let existing = with_state(&handle, |state| {
            let storage = scoped_vault_ref(state, &request.vault_id)?;
            Ok(storage
                .list_attachments(&request.destination, ATTACHMENT_LIST_HASH_LIMIT)?
                .into_iter()
                .map(|file| (file.relative_path, file.sha256))
                .collect::<Vec<_>>())
        })?;
        store_bytes(&request, bytes, &existing, |path, data, sha, id| {
            with_state(&handle, |state| {
                scoped_vault_mut(state, &request.vault_id)?
                    .store_attachment(path, data, sha, id)
                    .map(|(_, write)| write)
            })
        })
    })
    .await
    .map_err(map_join_err)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use onote_vault_storage::AttachmentWrite;

    fn request(name: &str) -> StoreRequest {
        StoreRequest {
            vault_id: "v".to_owned(),
            destination: "assets".to_owned(),
            name: name.to_owned(),
            operation_id: "op".to_owned(),
        }
    }

    fn body(header: &str, bytes: &[u8]) -> Vec<u8> {
        let mut out = (header.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn a_document_replaces_its_target_and_leaves_nothing_else() {
        let directory = tempfile::tempdir().expect("dir");
        let target = directory.path().join("note.html");
        write_document(&target, "one").expect("first");
        write_document(&target, "two").expect("second");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "two");
        assert_eq!(
            std::fs::read_dir(directory.path()).expect("list").count(),
            1
        );
    }

    #[test]
    fn the_store_body_splits_into_request_and_bytes() {
        let header = r#"{"vaultId":"v","destination":"assets","name":"a.png","operationId":"o"}"#;
        let raw = body(header, b"\x89PNG");
        let (parsed, bytes) = split_store_body(&raw).expect("split");
        assert_eq!(parsed.name, "a.png");
        assert_eq!(parsed.destination, "assets");
        assert_eq!(bytes, b"\x89PNG");
    }

    #[test]
    fn a_malformed_store_body_is_refused() {
        assert!(split_store_body(&[1, 2]).is_err());
        assert!(split_store_body(&body("not json", b"x")).is_err());
        let mut lying = 9_999u32.to_le_bytes().to_vec();
        lying.extend_from_slice(b"{}");
        assert!(split_store_body(&lying).is_err());
    }

    #[test]
    fn a_new_name_is_stored_and_a_taken_name_gets_a_suffix() {
        let mut written = Vec::new();
        let existing = vec![("assets/a.png".to_owned(), sha256_hex(b"other"))];
        let item = store_bytes(&request("a.png"), b"mine", &existing, |path, _, _, _| {
            written.push(path.to_owned());
            Ok(AttachmentWrite::Stored)
        })
        .expect("stored");
        assert_eq!(item.status, "stored");
        assert_eq!(item.destination, "assets/a (1).png");
        assert_eq!(written, vec!["assets/a (1).png".to_owned()]);
    }

    #[test]
    fn identical_bytes_are_reused_without_writing() {
        let existing = vec![("assets/a.png".to_owned(), sha256_hex(b"same"))];
        let item = store_bytes(&request("copy.png"), b"same", &existing, |_, _, _, _| {
            panic!("identical bytes must not be written")
        })
        .expect("reused");
        assert_eq!(item.status, "alreadyPresent");
        assert_eq!(item.destination, "assets/a.png");
    }

    #[test]
    fn a_failed_write_is_reported_and_has_no_usable_status() {
        let item = store_bytes(&request("a.png"), b"x", &[], |_, _, _, _| {
            Err(svc_err(ErrorCode::DiskFull, "error.diskFull"))
        })
        .expect("reported");
        assert_eq!(item.status, "failed");
        assert_eq!(item.error_key.as_deref(), Some("error.diskFull"));
    }

    #[test]
    fn a_pasted_file_over_the_limit_is_refused_before_planning() {
        let huge = vec![0u8; (MAX_INLINE_ATTACHMENT_BYTES + 1) as usize];
        let outcome = store_bytes(&request("big.png"), &huge, &[], |_, _, _, _| {
            panic!("an oversized file must not be written")
        });
        assert_eq!(
            outcome.expect_err("refused").message_key,
            "error.attachmentTooLarge"
        );
    }
}
