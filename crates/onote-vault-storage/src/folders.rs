//! Plain folders in the vault, created on request and listed even when they
//! hold no notes, so an empty folder stays visible in the file tree.

use std::fs;
use std::path::Path;

use onote_domain::ErrorCode;

use crate::errors::{self, SeaRes};
use crate::{pathcheck, scan, state};

/// Deeper trees are listed only down to here, as the note scan does.
const MAX_DEPTH: usize = 64;

/// Creates one folder at a vault-relative path, with any missing parents.
/// A name already in use (folder, file or link) is a collision, never reused.
pub(crate) fn create(root: &Path, raw: &str) -> SeaRes<String> {
    let canonical = pathcheck::validate_vault_relative_path(raw)?;
    let target = root.join(&canonical);
    if fs::symlink_metadata(&target).is_ok() {
        return Err(errors::err(
            ErrorCode::PathCollision,
            "error.folderExists",
            &[],
        ));
    }
    if let Some(parent) = target.parent() {
        state::ensure_writable_dir_parents(root, parent)?;
    }
    match fs::create_dir(&target) {
        Ok(()) => Ok(canonical),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(errors::err(
            ErrorCode::PathCollision,
            "error.folderExists",
            &[],
        )),
        Err(error) => Err(errors::io(&error, "error.dirCreate")),
    }
}

/// Every folder under the root, as sorted vault-relative paths. Hidden
/// folders (a leading `.`, which covers `.opennote`) and links are skipped and
/// not entered; an unreadable folder is listed but not entered.
pub(crate) fn list(root: &Path) -> SeaRes<Vec<String>> {
    let mut folders = Vec::new();
    walk(root, "", 0, &mut folders)?;
    folders.sort_by(|a, b| pathcheck::compare_windows_paths(a, b));
    Ok(folders)
}

fn walk(dir: &Path, prefix: &str, depth: usize, folders: &mut Vec<String>) -> SeaRes<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // The root must be readable; below it, a folder we cannot open is still shown.
        Err(error) if depth == 0 => return Err(errors::io(&error, "error.scanDenied")),
        Err(_) => return Ok(()),
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name.starts_with('.') || scan::is_reparse(&path) {
            continue;
        }
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        walk(&path, &relative, depth + 1, folders)?;
        folders.push(relative);
    }
    Ok(())
}

/// Files listed by name when a folder holds more than notes.
const LISTED_FILES: usize = 20;

/// What besides notes a folder holds, so a move or delete that only knows how
/// to carry notes can refuse it instead of leaving files or links behind.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderContents {
    /// Vault-relative paths of non-Markdown files, at most `LISTED_FILES`.
    pub other_files: Vec<String>,
    pub other_count: usize,
    /// A link or junction anywhere inside; never followed.
    pub has_link: bool,
}

impl FolderContents {
    fn notes_only(&self) -> bool {
        self.other_count == 0 && !self.has_link
    }
}

/// The folder at `raw`, which must exist as a real directory.
fn existing_folder(root: &Path, raw: &str) -> SeaRes<(String, std::path::PathBuf)> {
    let canonical = pathcheck::validate_vault_relative_path(raw)?;
    let target = root.join(&canonical);
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() && !scan::is_reparse(&target) => Ok((canonical, target)),
        Ok(_) => Err(errors::err(
            ErrorCode::InvalidInput,
            "error.pathNotDir",
            &[],
        )),
        Err(_) => Err(errors::err(
            ErrorCode::ObjectNotFound,
            "error.folderMissing",
            &[],
        )),
    }
}

pub(crate) fn inspect(root: &Path, raw: &str) -> SeaRes<FolderContents> {
    let (canonical, target) = existing_folder(root, raw)?;
    let mut contents = FolderContents {
        other_files: Vec::new(),
        other_count: 0,
        has_link: false,
    };
    collect(&target, &canonical, 0, &mut contents, false)?;
    Ok(contents)
}

/// Walks everything, hidden entries included. With `files_count`, a Markdown
/// file counts too: a folder about to be removed must hold nothing at all.
fn collect(
    dir: &Path,
    prefix: &str,
    depth: usize,
    contents: &mut FolderContents,
    files_count: bool,
) -> SeaRes<()> {
    if depth > MAX_DEPTH {
        contents.has_link = true;
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|error| errors::io(&error, "error.scanDenied"))?;
    for entry in entries {
        let entry = entry.map_err(|error| errors::io(&error, "error.scanDenied"))?;
        let path = entry.path();
        let relative = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        if scan::is_reparse(&path) {
            contents.has_link = true;
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| errors::io(&error, "error.scanDenied"))?;
        if kind.is_dir() {
            collect(&path, &relative, depth + 1, contents, files_count)?;
        } else if files_count || !relative.to_ascii_lowercase().ends_with(".md") {
            contents.other_count += 1;
            if contents.other_files.len() < LISTED_FILES {
                contents.other_files.push(relative);
            }
        }
    }
    Ok(())
}

/// A folder tree that holds no file and no link at all.
fn require_empty(target: &Path, canonical: &str) -> SeaRes<()> {
    let mut contents = FolderContents {
        other_files: Vec::new(),
        other_count: 0,
        has_link: false,
    };
    collect(target, canonical, 0, &mut contents, true)?;
    if contents.notes_only() {
        Ok(())
    } else {
        Err(errors::err(
            ErrorCode::InvalidInput,
            "error.folderNotEmpty",
            &[],
        ))
    }
}

/// Removes a folder tree that holds only folders, deepest first.
pub(crate) fn remove_empty(root: &Path, raw: &str) -> SeaRes<()> {
    let (canonical, target) = existing_folder(root, raw)?;
    require_empty(&target, &canonical)?;
    remove_tree(&target)
}

fn remove_tree(dir: &Path) -> SeaRes<()> {
    for entry in fs::read_dir(dir).map_err(|error| errors::io(&error, "error.scanDenied"))? {
        let entry = entry.map_err(|error| errors::io(&error, "error.scanDenied"))?;
        remove_tree(&entry.path())?;
    }
    fs::remove_dir(dir).map_err(|error| errors::io(&error, "error.dirRemove"))
}

/// Sends a folder holding files other than notes (a LaTeX project, say) away
/// whole, through `send`: the host passes the system recycle bin, tests a
/// stand-in. Notes must already be in the vault's trash, where they can be
/// restored with their history, and a link anywhere inside is refused because
/// it may lead out of the vault.
pub(crate) fn recycle(
    root: &Path,
    raw: &str,
    send: impl FnOnce(&Path) -> std::io::Result<()>,
) -> SeaRes<()> {
    let (canonical, target) = existing_folder(root, raw)?;
    let mut contents = FolderContents {
        other_files: Vec::new(),
        other_count: 0,
        has_link: false,
    };
    collect(&target, &canonical, 0, &mut contents, false)?;
    if contents.has_link {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.folderHasLink",
            &[],
        ));
    }
    if holds_note(&target, 0)? {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.folderHasNotes",
            &[],
        ));
    }
    send(&target).map_err(|error| errors::io(&error, "error.recycleFailed"))
}

/// True when a Markdown file sits anywhere below `dir`.
fn holds_note(dir: &Path, depth: usize) -> SeaRes<bool> {
    if depth > MAX_DEPTH {
        return Ok(true);
    }
    for entry in fs::read_dir(dir).map_err(|error| errors::io(&error, "error.scanDenied"))? {
        let entry = entry.map_err(|error| errors::io(&error, "error.scanDenied"))?;
        let kind = entry
            .file_type()
            .map_err(|error| errors::io(&error, "error.scanDenied"))?;
        if kind.is_dir() {
            if holds_note(&entry.path(), depth + 1)? {
                return Ok(true);
            }
        } else if entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".md")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Renames or moves a folder tree that holds only folders. A name already in
/// use is a collision; a case-only change of the same folder is allowed.
pub(crate) fn rename_empty(root: &Path, from: &str, to: &str) -> SeaRes<String> {
    let (from, source) = existing_folder(root, from)?;
    let to = pathcheck::validate_vault_relative_path(to)?;
    let lower_from = from.to_lowercase();
    if to.to_lowercase().starts_with(&format!("{lower_from}/")) {
        return Err(errors::err(
            ErrorCode::InvalidInput,
            "error.folderIntoItself",
            &[],
        ));
    }
    require_empty(&source, &from)?;
    let target = root.join(&to);
    let case_only = to != from && pathcheck::same_windows_path(&to, &from);
    if !case_only && fs::symlink_metadata(&target).is_ok() {
        return Err(errors::err(
            ErrorCode::PathCollision,
            "error.folderExists",
            &[],
        ));
    }
    if let Some(parent) = target.parent() {
        state::ensure_writable_dir_parents(root, parent)?;
    }
    fs::rename(&source, &target).map_err(|error| errors::io(&error, "error.dirRename"))?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_nested_folders_and_lists_empty_ones() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(create(root, "journal\\2026").unwrap(), "journal/2026");
        fs::create_dir(root.join("Ideas")).unwrap();
        fs::write(root.join("Ideas/a.md"), "a").unwrap();
        assert_eq!(list(root).unwrap(), ["Ideas", "journal", "journal/2026"]);
    }

    #[test]
    fn refuses_an_existing_name_and_unsafe_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        create(root, "a").unwrap();
        assert_eq!(
            create(root, "A").unwrap_err().code,
            ErrorCode::PathCollision
        );
        fs::write(root.join("note.md"), "").unwrap();
        assert_eq!(
            create(root, "note.md").unwrap_err().code,
            ErrorCode::PathCollision
        );
        for bad in [
            "",
            "../out",
            "a/../b",
            ".opennote",
            "x/.opennote",
            "a:b",
            "con",
            "trail.",
        ] {
            assert_eq!(
                create(root, bad).unwrap_err().code,
                ErrorCode::InvalidInput,
                "{bad}"
            );
        }
        assert!(!root.join("out").exists());
    }

    #[test]
    fn skips_hidden_folders() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".opennote/latex")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir(root.join("shown")).unwrap();
        assert_eq!(list(root).unwrap(), ["shown"]);
    }

    #[test]
    fn inspect_lists_what_is_not_a_note() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("p/sections")).unwrap();
        fs::write(root.join("p/a.md"), "").unwrap();
        fs::write(root.join("p/sections/main.tex"), "").unwrap();
        fs::write(root.join("p/.hidden"), "").unwrap();
        let contents = inspect(root, "p").unwrap();
        assert_eq!(contents.other_count, 2);
        assert!(contents
            .other_files
            .contains(&"p/sections/main.tex".to_owned()));
        assert!(!contents.has_link);
        fs::write(root.join("q.md"), "").unwrap();
        assert_eq!(
            inspect(root, "q.md").unwrap_err().code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            inspect(root, "none").unwrap_err().code,
            ErrorCode::ObjectNotFound
        );
    }

    #[test]
    fn recycles_only_folders_without_notes_or_links() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("proj/sections")).unwrap();
        fs::write(root.join("proj/main.tex"), "x").unwrap();
        fs::write(root.join("proj/sections/body.tex"), "x").unwrap();
        fs::create_dir_all(root.join("mixed/deep")).unwrap();
        fs::write(root.join("mixed/deep/note.MD"), "").unwrap();
        fs::write(root.join("mixed/a.tex"), "").unwrap();

        // A folder with a note inside is left to the note trash first.
        let mut sent = Vec::new();
        assert_eq!(
            recycle(root, "mixed", |path| {
                sent.push(path.to_path_buf());
                Ok(())
            })
            .unwrap_err()
            .message_key,
            "error.folderHasNotes"
        );
        assert!(sent.is_empty());

        // The root, the internal folder and a missing folder are refused.
        for bad in ["", ".opennote", "../out"] {
            assert_eq!(
                recycle(root, bad, |_| Ok(())).unwrap_err().code,
                ErrorCode::InvalidInput,
                "{bad}"
            );
        }
        assert_eq!(
            recycle(root, "none", |_| Ok(())).unwrap_err().code,
            ErrorCode::ObjectNotFound
        );

        // A notes-free project goes whole, by its real path.
        recycle(root, "proj", |path| fs::remove_dir_all(path)).unwrap();
        assert!(!root.join("proj").exists());

        // A failing recycle bin is reported, and nothing is removed.
        fs::create_dir(root.join("keep")).unwrap();
        fs::write(root.join("keep/a.png"), "").unwrap();
        let failed = recycle(root, "keep", |_| {
            Err(std::io::Error::new(std::io::ErrorKind::Other, "no bin"))
        })
        .unwrap_err();
        assert_eq!(failed.message_key, "error.recycleFailed");
        assert!(root.join("keep/a.png").exists());
    }

    #[test]
    fn removes_and_renames_only_trees_without_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        assert_eq!(rename_empty(root, "a", "x/a2").unwrap(), "x/a2");
        assert!(root.join("x/a2/b/c").is_dir());
        assert_eq!(rename_empty(root, "x/a2", "x/A2").unwrap(), "x/A2");
        assert_eq!(
            rename_empty(root, "x", "x/inner").unwrap_err().code,
            ErrorCode::InvalidInput
        );
        fs::create_dir(root.join("taken")).unwrap();
        assert_eq!(
            rename_empty(root, "x", "taken").unwrap_err().code,
            ErrorCode::PathCollision
        );
        fs::write(root.join("x/A2/b/note.md"), "").unwrap();
        assert_eq!(
            remove_empty(root, "x").unwrap_err().code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            rename_empty(root, "x", "y").unwrap_err().code,
            ErrorCode::InvalidInput
        );
        fs::remove_file(root.join("x/A2/b/note.md")).unwrap();
        remove_empty(root, "x").unwrap();
        assert!(!root.join("x").exists());
        assert!(root.join("taken").is_dir());
    }
}
