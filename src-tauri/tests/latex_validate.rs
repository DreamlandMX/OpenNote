//! The PDF structural check runs as `OpenNote.exe --latex-validate-pdf` in
//! the no-network sandbox: it must work there, and it must not be able to
//! read anything but the one directory it was given.
#![cfg(windows)]

use std::path::{Path, PathBuf};

use opennote_desktop::latex_test_support::{remove_profile, sample_pdf, validate_in_sandbox};

struct Profile(String);

impl Drop for Profile {
    fn drop(&mut self) {
        let errors = remove_profile(&self.0);
        if !errors.is_empty() {
            eprintln!("CLEANUP ERRORS {errors:?}");
        }
    }
}

fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_src-tauri"))
}

fn setup(tag: &str) -> (Profile, tempfile::TempDir, PathBuf, PathBuf) {
    let profile = Profile(format!(
        "opennote.sandboxtest.validate.{tag}.{}",
        std::process::id()
    ));
    let base = tempfile::tempdir().unwrap();
    let builds = base.path().join("latex-builds");
    let check = base.path().join("check");
    std::fs::create_dir_all(&check).unwrap();
    (profile, base, builds, check)
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::write(path, bytes).unwrap();
    path.to_owned()
}

#[test]
fn a_real_pdf_is_counted_by_the_sandboxed_check() {
    let (profile, _base, builds, check) = setup("ok");
    let pdf = write(&check.join("document.pdf"), &sample_pdf(4));
    assert_eq!(
        validate_in_sandbox(&program(), &builds, &profile.0, &check, &pdf),
        Ok(4)
    );
}

#[test]
fn a_broken_pdf_is_refused_by_the_sandboxed_check() {
    let (profile, _base, builds, check) = setup("bad");
    let mut garbage = b"%PDF-1.7\n1 0 obj << /Kids [".to_vec();
    garbage.extend_from_slice(b"\n%%EOF\n");
    let pdf = write(&check.join("document.pdf"), &garbage);
    assert_eq!(
        validate_in_sandbox(&program(), &builds, &profile.0, &check, &pdf),
        Err(r#"InvalidPdf("unparsable")"#.to_owned())
    );
}

#[test]
fn the_check_cannot_read_a_pdf_outside_its_directory() {
    let (profile, base, builds, check) = setup("outside");
    let elsewhere = base.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let pdf = write(&elsewhere.join("document.pdf"), &sample_pdf(1));
    assert_eq!(
        validate_in_sandbox(&program(), &builds, &profile.0, &check, &pdf),
        Err(r#"InvalidPdf("unreadable")"#.to_owned())
    );
}
