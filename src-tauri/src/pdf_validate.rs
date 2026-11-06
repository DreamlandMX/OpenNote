//! F52 structural check of an engine PDF.
//!
//! The PDF comes from an untrusted project, so it is never parsed in the GUI
//! host. `OpenNote.exe --latex-validate-pdf <file>` runs this check inside the
//! no-network sandbox, with read access to the one candidate file only, and
//! prints a single JSON line. A crash, a hang or anything but a `valid` verdict
//! means the PDF is not published.

use serde::{Deserialize, Serialize};

/// The flag that selects this mode in `main.rs`.
pub const FLAG: &str = "--latex-validate-pdf";
/// Largest PDF a build may publish (plan assumption 2).
pub const MAX_PDF_BYTES: u64 = 128 * 1024 * 1024;
/// Bound on what the reader would have to lay out.
pub const MAX_PAGES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "verdict")]
pub enum Verdict {
    Valid { pages: u32 },
    Invalid { reason: String },
}

fn invalid(reason: &str) -> Verdict {
    Verdict::Invalid {
        reason: reason.to_owned(),
    }
}

/// The check itself: a complete, unencrypted PDF whose page tree resolves to
/// at least one page dictionary.
pub fn check(bytes: &[u8]) -> Verdict {
    if bytes.len() as u64 > MAX_PDF_BYTES {
        return invalid("tooLarge");
    }
    if !bytes.starts_with(b"%PDF-") {
        return invalid("header");
    }
    // A truncated file (the engine was stopped mid-write) has no trailer marker.
    let tail = &bytes[bytes.len().saturating_sub(1024)..];
    if !tail.windows(5).any(|window| window == b"%%EOF") {
        return invalid("truncated");
    }
    let document = match lopdf::Document::load_mem(bytes) {
        Ok(document) => document,
        Err(_) => return invalid("unparsable"),
    };
    if document.is_encrypted() {
        return invalid("encrypted");
    }
    let pages = document.get_pages();
    if pages.is_empty() {
        return invalid("noPages");
    }
    if pages.len() > MAX_PAGES {
        return invalid("tooManyPages");
    }
    for id in pages.values() {
        let Ok(page) = document.get_dictionary(*id) else {
            return invalid("page");
        };
        if !page.has_type(b"Page") {
            return invalid("page");
        }
    }
    Verdict::Valid {
        pages: pages.len() as u32,
    }
}

/// `--latex-validate-pdf <file>`: prints the verdict and returns the exit code.
pub fn validate_main(args: &[String]) -> i32 {
    let verdict = match args {
        [path] => match std::fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_PDF_BYTES => invalid("tooLarge"),
            Ok(_) => match std::fs::read(path) {
                Ok(bytes) => check(&bytes),
                Err(_) => invalid("unreadable"),
            },
            Err(_) => invalid("unreadable"),
        },
        _ => invalid("usage"),
    };
    let line = serde_json::to_string(&verdict).unwrap_or_default();
    println!("{line}");
    match verdict {
        Verdict::Valid { .. } => 0,
        Verdict::Invalid { .. } => 2,
    }
}

/// A minimal, well-formed PDF with `pages` empty pages (test fixture).
#[doc(hidden)]
pub fn sample(pages: usize) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_owned());
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages
    ));
    for _ in 0..pages {
        objects.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_owned());
    }
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(pages: usize) -> Vec<u8> {
        sample(pages)
    }

    #[test]
    fn a_well_formed_pdf_is_valid_with_its_page_count() {
        assert_eq!(check(&pdf(3)), Verdict::Valid { pages: 3 });
    }

    #[test]
    fn broken_or_empty_documents_are_refused() {
        assert_eq!(check(b"hello"), invalid("header"));
        let whole = pdf(2);
        assert_eq!(check(&whole[..whole.len() / 2]), invalid("truncated"));
        assert_eq!(check(&pdf(0)), invalid("noPages"));
        let mut garbage = b"%PDF-1.5\nnot a pdf at all\n".to_vec();
        garbage.extend_from_slice(b"%%EOF\n");
        assert_eq!(check(&garbage), invalid("unparsable"));
    }

    #[test]
    fn the_verdict_is_one_tagged_json_line() {
        let json = serde_json::to_string(&Verdict::Valid { pages: 2 }).unwrap();
        assert_eq!(json, r#"{"verdict":"valid","pages":2}"#);
        let parsed: Verdict =
            serde_json::from_str(r#"{"verdict":"invalid","reason":"x"}"#).unwrap();
        assert_eq!(parsed, invalid("x"));
    }
}
