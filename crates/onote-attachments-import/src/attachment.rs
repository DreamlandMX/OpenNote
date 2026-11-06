//! M07 single-file attachment import planning (F19).
//!
//! An attachment is imported as **bytes plus a hash**, and the originals are never
//! re-encoded: a thumbnail is a rebuildable cache, never a replacement (F19-R01).
//! That distinction drives everything here — the plan records what will be stored,
//! what will be cached separately, and what the reader will see for a type the app
//! cannot render (F19-R02).
//!
//! Three further rules:
//!
//! * a file whose type is unknown is still importable and is opened by the system,
//!   never executed (F19-R02);
//! * the cloud size limit never blocks a local import; it marks the file as
//!   unsynced with the reason (F19-R04);
//! * a path that could escape the vault is refused before anything is written
//!   (F19-R06).
//!
//! Nothing here reads or writes a file.

use serde::{Deserialize, Serialize};

/// Largest attachment this build imports in one piece.
pub const MAX_ATTACHMENT_BYTES: u64 = 512 * 1024 * 1024;

/// The cloud limit, which marks a file unsynced but never blocks it locally.
pub const CLOUD_LIMIT_BYTES: u64 = 100 * 1024 * 1024;

/// How a file is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttachmentKind {
    /// Rendered inline in the editor.
    Image,
    /// Shown by an embedded viewer.
    Media,
    /// Handed to the system's default application (F19-R02).
    Document,
    /// No preview; the reader gets a download or open action.
    Binary,
    /// Recognised but not something this build previews.
    Unknown,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Media => "media",
            Self::Document => "document",
            Self::Binary => "binary",
            Self::Unknown => "unknown",
        }
    }
}

/// What the reader is offered for a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preview {
    /// Drawn by the editor.
    Inline,
    /// Opened by the system handler; the app itself never runs it.
    SystemOpen,
    /// Download only.
    Download,
}

impl Preview {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::SystemOpen => "systemOpen",
            Self::Download => "download",
        }
    }
}

/// The kind and preview for an extension.
///
/// An unrecognised extension is `Unknown` with `Download`: the file is kept and the
/// user can still open it, which is the point of F19-R02. Treating it as an error
/// would lose the file.
pub fn classify(extension: &str) -> (AttachmentKind, Preview) {
    let lowered = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match lowered.as_str() {
        // Raster formats every supported platform can decode.
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "avif" | "ico" => {
            (AttachmentKind::Image, Preview::Inline)
        }
        // SVG is an image but may carry script, so it is shown by the system
        // handler rather than drawn inline.
        "svg" => (AttachmentKind::Image, Preview::SystemOpen),
        "mp3" | "m4a" | "wav" | "ogg" | "flac" | "mp4" | "webm" | "mov" => {
            (AttachmentKind::Media, Preview::Inline)
        }
        "pdf" => (AttachmentKind::Document, Preview::SystemOpen),
        "txt" | "csv" | "json" | "yaml" | "yml" => (AttachmentKind::Document, Preview::Download),
        "zip" | "7z" | "tar" | "gz" => (AttachmentKind::Binary, Preview::Download),
        _ => (AttachmentKind::Unknown, Preview::Download),
    }
}

/// Whether an extension is an image the editor can draw.
pub fn is_inline_image(extension: &str) -> bool {
    let (kind, preview) = classify(extension);
    kind == AttachmentKind::Image && preview == Preview::Inline
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttachmentIssue {
    /// Empty name, or a name that is only separators.
    EmptyName,
    /// `..`, an absolute path, a drive letter or a NUL byte (F19-R06).
    UnsafePath { path: String },
    /// Bigger than this build imports in one piece.
    TooLarge { size: u64, limit: u64 },
    /// Zero bytes: importable, but worth reporting rather than storing silently.
    Empty { path: String },
    /// A file with this hash already exists, so nothing needs storing.
    DuplicateContent { existing: String },
    /// A different file already occupies the requested name; both are kept and
    /// the incoming one is stored under `to` (F19-A01, §4).
    RenamedForCollision { from: String, to: String },
}

/// A planned attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedAttachment {
    /// Vault-relative destination with `/` separators.
    pub destination: String,
    pub kind: AttachmentKind,
    pub preview: Preview,
    pub size: u64,
    /// Lower-case hex SHA-256 of the bytes, so duplicates can be found without
    /// reading the file again (M07 keeps the original and its hash).
    pub sha256: String,
    /// True when the bytes must be written; false when an identical file exists.
    pub stores_bytes: bool,
    /// True when a thumbnail would be generated. The thumbnail is a cache and is
    /// regenerated on demand, never written in place of the original (F19-R01).
    pub caches_thumbnail: bool,
    /// True when the file exceeds the cloud limit. Local use still works; the file
    /// is marked unsynced with the reason (F19-R04).
    pub exceeds_cloud_limit: bool,
    /// The markdown snippet that references the attachment.
    pub reference: String,
    pub issues: Vec<AttachmentIssue>,
}

impl PlannedAttachment {
    /// True when the attachment can be imported as planned.
    ///
    /// A bad name blocks the import just as a bad path does: a name containing a
    /// separator would place the file somewhere the user did not choose, and an
    /// empty name has no destination at all. The `Empty` and `DuplicateContent`
    /// issues are informational and do not block.
    pub fn is_importable(&self) -> bool {
        !self.issues.iter().any(|issue| {
            matches!(
                issue,
                AttachmentIssue::UnsafePath { .. }
                    | AttachmentIssue::TooLarge { .. }
                    | AttachmentIssue::EmptyName
            )
        })
    }

    /// True when this import needs the bytes written to disk.
    pub fn writes(&self) -> bool {
        self.stores_bytes && self.is_importable()
    }
}

/// The whole plan for a set of files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentPlan {
    pub attachments: Vec<PlannedAttachment>,
    /// Bytes that would be written, after duplicates are excluded.
    pub total_bytes: u64,
    /// Files that would be marked unsynced (F19-R04).
    pub unsynced: usize,
}

impl AttachmentPlan {
    /// Attachments that can be imported.
    pub fn importable(&self) -> Vec<&PlannedAttachment> {
        self.attachments
            .iter()
            .filter(|attachment| attachment.is_importable())
            .collect()
    }

    /// Attachments refused for a reason that would lose the file.
    pub fn refused(&self) -> Vec<&PlannedAttachment> {
        self.attachments
            .iter()
            .filter(|attachment| !attachment.is_importable())
            .collect()
    }
}

/// One file the caller wants to import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInput {
    /// The file name as the user chose it, without a directory.
    pub name: String,
    /// The full path the file came from, used only for the unsafe-path check.
    pub source_path: String,
    pub size: u64,
    /// Lower-case hex SHA-256 of the bytes, computed by the caller.
    pub sha256: String,
}

/// Validates a destination directory.
///
/// The folder is vault-relative; an absolute path or a `..` segment is refused
/// rather than clamped, because clamping would silently place files somewhere the
/// user did not ask for.
pub fn validate_destination(destination: &str) -> Result<String, AttachmentIssue> {
    let normalized = destination.trim().replace('\\', "/");
    let normalized = normalized.trim_matches('/');
    if normalized.is_empty() {
        // The vault root is a legal destination for an attachment folder.
        return Ok(String::new());
    }
    if normalized.chars().nth(1) == Some(':') || normalized.starts_with('/') {
        return Err(AttachmentIssue::UnsafePath {
            path: destination.to_owned(),
        });
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                return Err(AttachmentIssue::UnsafePath {
                    path: destination.to_owned(),
                })
            }
            other => segments.push(other),
        }
    }
    Ok(segments.join("/"))
}

/// Whether a source path tries to traverse or carries a NUL byte.
///
/// A source path is absolute, so a leading `/` or a drive letter is normal here;
/// what matters is that it does not climb with `..` and does not smuggle a NUL.
fn source_is_unsafe(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.contains('\0') || normalized.split('/').any(|segment| segment == "..")
}

/// Whether a name is usable as a file name.
fn name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\0')
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
}

/// The extension of a name, without the dot.
pub fn extension_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, suffix)) if !stem.is_empty() => suffix.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// The markdown an inserted attachment uses.
///
/// An inline image becomes `![](path)`; anything else becomes a link, because a
/// preview the app cannot draw would render as a broken image.
pub fn reference_for(destination: &str, kind: AttachmentKind, preview: Preview) -> String {
    if kind == AttachmentKind::Image && preview == Preview::Inline {
        format!("![]({destination})")
    } else {
        format!("[{destination}]({destination})")
    }
}

/// The media type the app is willing to show, decided from the file's first bytes.
///
/// The extension only breaks one tie: SVG is text, so it is recognised when the
/// name says `.svg` and the content opens with an `<svg` element. Anything else,
/// including a `.png` holding other bytes, returns `None` and is not displayed.
pub fn detect_media_type(path: &str, head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if head.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if head.starts_with(b"BM") && head.len() >= 14 {
        return Some("image/bmp");
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" && matches!(&head[8..12], b"avif" | b"avis") {
        return Some("image/avif");
    }
    if head.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if path.to_ascii_lowercase().ends_with(".svg") && looks_like_svg(head) {
        return Some("image/svg+xml");
    }
    None
}

/// True when the text opens with an `<svg` element, after a BOM, an XML
/// declaration, comments or a doctype.
fn looks_like_svg(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        if rest.starts_with("<svg") {
            return true;
        }
        let closing = if rest.starts_with("<?") {
            "?>"
        } else if rest.starts_with("<!--") {
            "-->"
        } else if rest.starts_with("<!") {
            ">"
        } else {
            return false;
        };
        match rest.find(closing) {
            Some(at) => rest = rest[at + closing.len()..].trim_start(),
            None => return false,
        }
    }
}

/// A case-insensitive vault path, because the vault lives on Windows (F03-R01).
fn normalized(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Appends a numeric suffix before the extension, matching the ZIP importer so
/// the same conflict reads the same way in both flows.
fn suffixed_attachment(destination: &str, index: u32) -> String {
    let (folder, name) = match destination.rsplit_once('/') {
        Some((folder, name)) => (folder, name),
        None => ("", destination),
    };
    let renamed = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem} ({index}).{extension}"),
        _ => format!("{name} ({index})"),
    };
    if folder.is_empty() {
        renamed
    } else {
        format!("{folder}/{renamed}")
    }
}

/// Plans importing the given files into a folder.
///
/// `existing` maps vault-relative paths to their SHA-256, so a re-import of the
/// same bytes is recognised without writing again. A name occupied by different
/// bytes is renamed rather than overwritten (F19-A01).
pub fn plan_import(
    inputs: &[AttachmentInput],
    destination: &str,
    existing: &[(String, String)],
) -> Result<AttachmentPlan, AttachmentIssue> {
    let folder = validate_destination(destination)?;
    let mut attachments: Vec<PlannedAttachment> = Vec::new();
    let mut occupied: Vec<(String, String, String)> = existing
        .iter()
        .map(|(path, sha)| (normalized(path), path.clone(), sha.to_ascii_lowercase()))
        .collect();
    let mut total_bytes = 0u64;
    let mut unsynced = 0usize;

    for input in inputs {
        let mut issues: Vec<AttachmentIssue> = Vec::new();
        if !name_is_safe(&input.name) {
            issues.push(AttachmentIssue::EmptyName);
        }
        // The source is an absolute OS path by definition, so it is NOT validated
        // as a vault-relative destination. Only the parts that matter for safety
        // are checked: it must not traverse upward and must not contain a NUL.
        if source_is_unsafe(&input.source_path) {
            issues.push(AttachmentIssue::UnsafePath {
                path: input.source_path.clone(),
            });
        }
        if input.size > MAX_ATTACHMENT_BYTES {
            issues.push(AttachmentIssue::TooLarge {
                size: input.size,
                limit: MAX_ATTACHMENT_BYTES,
            });
        }
        if input.size == 0 {
            issues.push(AttachmentIssue::Empty {
                path: input.name.clone(),
            });
        }

        let name = input.name.replace('\\', "/");
        let name = name.trim_start_matches('/');
        let requested_path = if folder.is_empty() {
            name.to_owned()
        } else {
            format!("{folder}/{name}")
        };
        let extension = extension_of(&name);
        let (kind, preview) = classify(&extension);

        let blocked = issues.iter().any(|issue| {
            matches!(
                issue,
                AttachmentIssue::UnsafePath { .. }
                    | AttachmentIssue::TooLarge { .. }
                    | AttachmentIssue::EmptyName
            )
        });
        let mut destination_path = requested_path.clone();
        let mut stores_bytes = false;
        if !blocked {
            let sha256 = input.sha256.to_ascii_lowercase();
            // Identical bytes are reused wherever they already live, so a second
            // import does not create a copy under a new name.
            let existing_sha_path = occupied
                .iter()
                .find(|(_, _, known)| *known == sha256)
                .map(|(_, path, _)| path.clone());
            if let Some(existing_path) = existing_sha_path {
                issues.push(AttachmentIssue::DuplicateContent {
                    existing: existing_path.clone(),
                });
                destination_path = existing_path;
            } else if occupied
                .iter()
                .any(|(path, _, _)| *path == normalized(&requested_path))
            {
                // Different bytes under the same name: keep both. The suffix is
                // shown in the plan so the inserted reference names the renamed file.
                let mut index = 1u32;
                let renamed = loop {
                    let candidate = suffixed_attachment(&requested_path, index);
                    if !occupied
                        .iter()
                        .any(|(path, _, _)| *path == normalized(&candidate))
                    {
                        break candidate;
                    }
                    index += 1;
                };
                issues.push(AttachmentIssue::RenamedForCollision {
                    from: requested_path.clone(),
                    to: renamed.clone(),
                });
                destination_path = renamed;
                stores_bytes = true;
            } else {
                stores_bytes = true;
            }
            if stores_bytes {
                occupied.push((
                    normalized(&destination_path),
                    destination_path.clone(),
                    sha256,
                ));
            }
        }

        let exceeds_cloud_limit = input.size > CLOUD_LIMIT_BYTES;
        if exceeds_cloud_limit {
            unsynced += 1;
        }

        let attachment = PlannedAttachment {
            reference: reference_for(&destination_path, kind, preview),
            destination: destination_path,
            kind,
            preview,
            size: input.size,
            sha256: input.sha256.to_ascii_lowercase(),
            stores_bytes,
            // A thumbnail is only useful for an inline image, and it is a cache.
            caches_thumbnail: is_inline_image(&extension) && stores_bytes,
            exceeds_cloud_limit,
            issues,
        };
        // Only new bytes count toward the write total.
        if attachment.stores_bytes && attachment.is_importable() {
            total_bytes = total_bytes.saturating_add(attachment.size);
        }
        attachments.push(attachment);
    }

    Ok(AttachmentPlan {
        attachments,
        total_bytes,
        unsynced,
    })
}

/// The message key for an issue.
pub fn issue_key(issue: &AttachmentIssue) -> &'static str {
    match issue {
        AttachmentIssue::EmptyName => "error.attachmentEmptyName",
        AttachmentIssue::UnsafePath { .. } => "error.attachmentUnsafePath",
        AttachmentIssue::TooLarge { .. } => "error.attachmentTooLarge",
        AttachmentIssue::Empty { .. } => "error.attachmentEmpty",
        AttachmentIssue::DuplicateContent { .. } => "attachment.duplicateContent",
        AttachmentIssue::RenamedForCollision { .. } => "attachment.issue.renamed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, size: u64, sha: &str) -> AttachmentInput {
        AttachmentInput {
            name: name.to_owned(),
            // A real picker hands over an absolute path, which is legal for a
            // source; only traversal and NUL are refused.
            source_path: format!("C:/Users/a/{name}"),
            size,
            sha256: sha.to_owned(),
        }
    }

    #[test]
    fn the_media_type_comes_from_the_bytes_not_the_name() {
        assert_eq!(
            detect_media_type("a.txt", b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(
            detect_media_type("a.png", b"\xff\xd8\xff\xe0"),
            Some("image/jpeg")
        );
        assert_eq!(detect_media_type("a", b"GIF89a"), Some("image/gif"));
        assert_eq!(
            detect_media_type("a", b"RIFF1234WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            detect_media_type("a", b"\0\0\0\x1cftypavif0000"),
            Some("image/avif")
        );
        assert_eq!(detect_media_type("a", b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(
            detect_media_type("evil.png", b"<script>alert(1)</script>"),
            None
        );
        assert_eq!(detect_media_type("evil.png", b"MZ\x90\0"), None);
    }

    #[test]
    fn svg_needs_both_its_name_and_an_svg_root() {
        let svg =
            b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<!-- c -->\n<!DOCTYPE svg>\n<svg xmlns=\"x\">";
        assert_eq!(detect_media_type("a.SVG", svg), Some("image/svg+xml"));
        assert_eq!(detect_media_type("a.png", svg), None);
        assert_eq!(detect_media_type("a.svg", b"<html><svg>"), None);
    }

    #[test]
    fn an_image_is_referenced_inline_and_gets_a_thumbnail() {
        let plan = plan_import(&[input("photo.png", 1024, "aa")], "assets", &[]).expect("plan");
        let attachment = &plan.attachments[0];
        assert_eq!(attachment.destination, "assets/photo.png");
        assert_eq!(attachment.kind, AttachmentKind::Image);
        assert_eq!(attachment.preview, Preview::Inline);
        assert_eq!(attachment.reference, "![](assets/photo.png)");
        // F19-R01: the thumbnail is a cache, not a replacement.
        assert!(attachment.caches_thumbnail);
        assert!(attachment.stores_bytes);
        assert_eq!(plan.total_bytes, 1024);
    }

    #[test]
    fn an_unknown_type_is_kept_and_opened_by_the_system() {
        // F19-R02: keep the file, never execute it.
        let plan = plan_import(&[input("data.bin", 10, "bb")], "assets", &[]).expect("plan");
        let attachment = &plan.attachments[0];
        assert_eq!(attachment.kind, AttachmentKind::Unknown);
        assert_eq!(attachment.preview, Preview::Download);
        assert!(
            attachment.is_importable(),
            "an unknown type must not be refused"
        );
        assert!(!attachment.caches_thumbnail);
        // A link, not an image tag, because nothing can draw it.
        assert_eq!(attachment.reference, "[assets/data.bin](assets/data.bin)");
    }

    #[test]
    fn svg_is_shown_by_the_system_rather_than_inline() {
        // An SVG can carry script, so it is not drawn by the editor.
        let plan = plan_import(&[input("logo.svg", 100, "cc")], "assets", &[]).expect("plan");
        let attachment = &plan.attachments[0];
        assert_eq!(attachment.kind, AttachmentKind::Image);
        assert_eq!(attachment.preview, Preview::SystemOpen);
        assert!(!attachment.caches_thumbnail);
        assert_eq!(attachment.reference, "[assets/logo.svg](assets/logo.svg)");
    }

    #[test]
    fn a_pdf_is_a_document_opened_by_the_system() {
        let plan = plan_import(&[input("paper.pdf", 2048, "dd")], "docs", &[]).expect("plan");
        assert_eq!(plan.attachments[0].kind, AttachmentKind::Document);
        assert_eq!(plan.attachments[0].preview, Preview::SystemOpen);
    }

    #[test]
    fn the_same_bytes_are_not_stored_twice() {
        let plan = plan_import(
            &[input("copy.png", 512, "AB")],
            "assets",
            &[("assets/original.png".to_owned(), "ab".to_owned())],
        )
        .expect("plan");
        let attachment = &plan.attachments[0];
        assert!(!attachment.stores_bytes);
        assert!(attachment.issues.iter().any(|issue| matches!(
            issue,
            AttachmentIssue::DuplicateContent { existing } if existing == "assets/original.png"
        )));
        // The reference names the file that actually holds these bytes.
        assert_eq!(attachment.destination, "assets/original.png");
        assert_eq!(attachment.reference, "![](assets/original.png)");
        // Nothing new is written, so the total is zero.
        assert_eq!(plan.total_bytes, 0);
    }

    #[test]
    fn a_same_name_with_different_bytes_is_renamed_not_overwritten() {
        // F19-A01: the original attachment is never replaced by a same-named file.
        let plan = plan_import(
            &[input("photo.png", 100, "bb")],
            "assets",
            &[("assets/photo.png".to_owned(), "aa".to_owned())],
        )
        .expect("plan");
        let attachment = &plan.attachments[0];
        assert_eq!(attachment.destination, "assets/photo (1).png");
        assert_eq!(attachment.reference, "![](assets/photo (1).png)");
        assert!(attachment.stores_bytes);
        assert!(attachment.issues.iter().any(|issue| matches!(
            issue,
            AttachmentIssue::RenamedForCollision { from, to }
                if from == "assets/photo.png" && to == "assets/photo (1).png"
        )));
        assert_eq!(plan.total_bytes, 100);
    }

    #[test]
    fn a_case_only_name_collision_is_renamed() {
        // Windows treats these as the same file (F03-R01), so bytes must not be
        // planned onto the existing name.
        let plan = plan_import(
            &[input("photo.png", 100, "bb")],
            "assets",
            &[("Assets/Photo.PNG".to_owned(), "aa".to_owned())],
        )
        .expect("plan");
        assert_eq!(plan.attachments[0].destination, "assets/photo (1).png");
        assert!(plan.attachments[0].stores_bytes);
    }

    #[test]
    fn two_files_with_one_name_in_a_batch_are_both_kept() {
        let plan = plan_import(
            &[input("photo.png", 100, "aa"), input("photo.png", 200, "bb")],
            "assets",
            &[],
        )
        .expect("plan");
        assert_eq!(plan.attachments[0].destination, "assets/photo.png");
        assert_eq!(plan.attachments[1].destination, "assets/photo (1).png");
        assert!(plan
            .attachments
            .iter()
            .all(|attachment| attachment.stores_bytes));
        assert_eq!(plan.total_bytes, 300);
    }

    #[test]
    fn a_suffix_skips_names_that_are_already_taken() {
        let plan = plan_import(
            &[input("photo.png", 100, "bb")],
            "assets",
            &[
                ("assets/photo.png".to_owned(), "aa".to_owned()),
                ("assets/photo (1).png".to_owned(), "cc".to_owned()),
            ],
        )
        .expect("plan");
        assert_eq!(plan.attachments[0].destination, "assets/photo (2).png");
    }

    #[test]
    fn a_same_name_with_the_same_bytes_stays_a_duplicate() {
        let plan = plan_import(
            &[input("photo.png", 100, "ab")],
            "assets",
            &[("assets/photo.png".to_owned(), "AB".to_owned())],
        )
        .expect("plan");
        let attachment = &plan.attachments[0];
        assert!(!attachment.stores_bytes);
        assert_eq!(attachment.destination, "assets/photo.png");
        assert!(!attachment
            .issues
            .iter()
            .any(|issue| matches!(issue, AttachmentIssue::RenamedForCollision { .. })));
    }

    #[test]
    fn a_file_over_the_cloud_limit_is_marked_unsynced_but_still_importable() {
        // F19-R04: the cloud limit never blocks local use.
        let plan = plan_import(
            &[input("huge.mp4", CLOUD_LIMIT_BYTES + 1, "ee")],
            "assets",
            &[],
        )
        .expect("plan");
        let attachment = &plan.attachments[0];
        assert!(attachment.exceeds_cloud_limit);
        assert!(attachment.is_importable());
        assert!(attachment.writes());
        assert_eq!(plan.unsynced, 1);
    }

    #[test]
    fn a_file_over_the_hard_limit_is_refused() {
        let plan = plan_import(
            &[input("huge.bin", MAX_ATTACHMENT_BYTES + 1, "ff")],
            "assets",
            &[],
        )
        .expect("plan");
        assert!(!plan.attachments[0].is_importable());
        assert_eq!(plan.refused().len(), 1);
        assert_eq!(plan.total_bytes, 0);
    }

    #[test]
    fn an_unsafe_destination_is_refused_outright() {
        // F19-R06: a destination that climbs out is refused, never clamped.
        for destination in ["../outside", "a/../../b", "C:/vault"] {
            assert!(
                plan_import(&[], destination, &[]).is_err(),
                "{destination} should be refused"
            );
        }
        // A leading slash is a common way to type "the vault root", so it is
        // normalised rather than refused — clamping and refusing are different
        // things, and only climbing out is unsafe.
        assert!(plan_import(&[], "/etc", &[]).is_ok());
    }

    #[test]
    fn an_unsafe_source_path_is_reported_on_the_attachment() {
        let mut bad = input("ok.png", 10, "11");
        bad.source_path = "../../outside/ok.png".to_owned();
        let plan = plan_import(&[bad], "assets", &[]).expect("plan");
        assert!(!plan.attachments[0].is_importable());
        assert!(plan.attachments[0]
            .issues
            .iter()
            .any(|issue| matches!(issue, AttachmentIssue::UnsafePath { .. })));
    }

    #[test]
    fn a_name_with_a_separator_is_refused() {
        for name in ["a/b.png", "a\\b.png", "", ".."] {
            // The source is a legal absolute path, so only the name is at fault.
            let mut candidate = input(name, 10, "22");
            candidate.source_path = "C:/Users/a/file.png".to_owned();
            let plan = plan_import(&[candidate], "assets", &[]).expect("plan");
            assert!(
                !plan.attachments[0].is_importable(),
                "{name:?} should be refused"
            );
        }
    }

    #[test]
    fn an_empty_file_is_importable_but_reported() {
        let plan = plan_import(&[input("empty.txt", 0, "33")], "assets", &[]).expect("plan");
        let attachment = &plan.attachments[0];
        assert!(attachment.is_importable());
        assert!(attachment
            .issues
            .iter()
            .any(|issue| matches!(issue, AttachmentIssue::Empty { .. })));
    }

    #[test]
    fn the_vault_root_is_a_legal_destination() {
        let plan = plan_import(&[input("a.png", 10, "44")], "", &[]).expect("plan");
        assert_eq!(plan.attachments[0].destination, "a.png");
    }

    #[test]
    fn a_trailing_or_leading_slash_is_normalised() {
        let plan = plan_import(&[input("a.png", 10, "55")], "/assets/", &[]).expect("plan");
        assert_eq!(plan.attachments[0].destination, "assets/a.png");
    }

    #[test]
    fn only_inline_images_cache_a_thumbnail() {
        assert!(is_inline_image("png"));
        assert!(is_inline_image(".PNG"));
        assert!(!is_inline_image("svg"));
        assert!(!is_inline_image("pdf"));
        assert!(!is_inline_image(""));
    }

    #[test]
    fn extensions_are_lower_cased_and_a_dotfile_has_none() {
        assert_eq!(extension_of("A.PNG"), "png");
        assert_eq!(extension_of("notes.tar.gz"), "gz");
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of("noext"), "");
    }

    #[test]
    fn several_files_sum_their_new_bytes() {
        let plan = plan_import(
            &[
                input("a.png", 100, "aa"),
                input("b.pdf", 200, "bb"),
                input("c.txt", 300, "cc"),
            ],
            "assets",
            &[],
        )
        .expect("plan");
        assert_eq!(plan.importable().len(), 3);
        assert_eq!(plan.total_bytes, 600);
    }

    #[test]
    fn kinds_round_trip_to_their_names() {
        assert_eq!(AttachmentKind::Image.as_str(), "image");
        assert_eq!(AttachmentKind::Unknown.as_str(), "unknown");
        assert_eq!(Preview::SystemOpen.as_str(), "systemOpen");
        assert_eq!(Preview::Download.as_str(), "download");
    }

    #[test]
    fn every_issue_maps_to_a_message_key() {
        let issues = [
            AttachmentIssue::EmptyName,
            AttachmentIssue::UnsafePath {
                path: "x".to_owned(),
            },
            AttachmentIssue::TooLarge { size: 1, limit: 1 },
            AttachmentIssue::Empty {
                path: "x".to_owned(),
            },
            AttachmentIssue::DuplicateContent {
                existing: "y".to_owned(),
            },
            AttachmentIssue::RenamedForCollision {
                from: "x".to_owned(),
                to: "x (1)".to_owned(),
            },
        ];
        for issue in issues {
            let key = issue_key(&issue);
            assert!(
                key.starts_with("error.") || key.starts_with("attachment."),
                "{key}"
            );
        }
    }
}
