use onote_domain::ErrorCode;

use crate::errors::{self, SeaRes};

const RESERVED_BASE: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const MAX_NAME_UTF16: usize = 120;

pub fn compare_windows_paths(left: &str, right: &str) -> std::cmp::Ordering {
    #[cfg(windows)]
    {
        let left: Vec<u16> = left.encode_utf16().collect();
        let right: Vec<u16> = right.encode_utf16().collect();
        let (Ok(left_len), Ok(right_len)) = (i32::try_from(left.len()), i32::try_from(right.len()))
        else {
            return std::cmp::Ordering::Equal;
        };
        // Ordinal comparison follows Windows casing, independent of the interface locale.
        let result = unsafe {
            windows_sys::Win32::Globalization::CompareStringOrdinal(
                left.as_ptr(),
                left_len,
                right.as_ptr(),
                right_len,
                1,
            )
        };
        if result == 0 {
            std::cmp::Ordering::Equal
        } else {
            result.cmp(&windows_sys::Win32::Globalization::CSTR_EQUAL)
        }
    }
    // shortcut: other desktop platforms need a filesystem casing adapter before release.
    #[cfg(not(windows))]
    {
        left.to_uppercase().cmp(&right.to_uppercase())
    }
}

pub(crate) fn same_windows_path(left: &str, right: &str) -> bool {
    compare_windows_paths(left, right).is_eq()
}

/// Validate a relative Markdown path for a *new* note. Returns the canonical
/// forward-slash form. No trimming/normalization of real text happens here:
/// structural violations simply reject the operation.
pub fn validate_new_note_path(raw: &str) -> SeaRes<String> {
    let canonical = validate_vault_relative_path(raw)?;
    let last = canonical.rsplit('/').next().unwrap_or("");
    if !last.to_ascii_lowercase().ends_with(".md") {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.notMarkdown",
            &[],
        ));
    }
    Ok(canonical)
}

/// Validate a relative path for a new attachment file. Same structural rules as
/// a note path, but a Markdown name is refused: notes are created only through
/// the note path with an identity, never as raw attachment bytes.
pub fn validate_attachment_path(raw: &str) -> SeaRes<String> {
    let canonical = validate_vault_relative_path(raw)?;
    let last = canonical.rsplit('/').next().unwrap_or("");
    let lower = last.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.attachmentIsNote",
            &[],
        ));
    }
    Ok(canonical)
}

/// Structural checks shared by every vault-relative file path.
pub(crate) fn validate_vault_relative_path(raw: &str) -> SeaRes<String> {
    if raw.is_empty() {
        return Err(errors::err(ErrorCode::InvalidInput, "error.emptyPath", &[]));
    }
    if raw.starts_with('/') || raw.starts_with('\\') || (raw.as_bytes().get(1) == Some(&b':')) {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.absolutePath",
            &[],
        ));
    }
    let normalized = raw.replace('\\', "/");
    let mut canonical = String::with_capacity(normalized.len() + 4);
    for comp in normalized.split('/') {
        if comp.is_empty() {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.emptyPathSegment",
                &[],
            ));
        }
        if comp == "." || comp == ".." {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.pathTraversal",
                &[],
            ));
        }
        if comp.contains(':') {
            return Err(errors::err(ErrorCode::InvalidInput, "error.pathAds", &[]));
        }
        if comp.chars().any(|c| {
            matches!(
                c,
                '"' | '<' | '>' | '|' | '*' | '?' | '\u{0}'..='\u{1f}' | '\u{7f}'
            )
        }) {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.pathIllegalChar",
                &[],
            ));
        }
        if comp.ends_with('.') || comp.ends_with(' ') {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.trailingDotOrSpace",
                &[],
            ));
        }
        if comp.eq_ignore_ascii_case(".opennote") {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.reservedDir",
                &[],
            ));
        }
        if is_reserved_name(comp) {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.reservedName",
                &[],
            ));
        }
        if comp.encode_utf16().count() > MAX_NAME_UTF16 {
            return Err(errors::err(
                ErrorCode::InvalidInput,
                "error.nameTooLong",
                &[],
            ));
        }
        canonical.push_str(comp);
        canonical.push('/');
    }
    canonical.pop();
    Ok(canonical)
}

fn is_reserved_name(comp: &str) -> bool {
    let stem = match comp.split_once('.') {
        Some((stem, _)) => stem,
        None => comp,
    };
    RESERVED_BASE.iter().any(|r| stem.eq_ignore_ascii_case(r))
}
