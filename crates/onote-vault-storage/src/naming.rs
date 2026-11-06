//! M02 file-name and path validation for vault operations (F03).
//!
//! The rules here exist because the vault lives on a Windows file system, so the
//! name a user types is not automatically a name that can exist:
//!
//! * an empty name, a path separator, a reserved device name, or a trailing dot or
//!   space is refused before anything is written (§4);
//! * two names that differ only in case are the **same file** on Windows, so a
//!   rename into an existing name is a collision even when the strings differ
//!   (F03-R01);
//! * a destination may not escape the vault root or nest inside the item being
//!   moved, which would make the operation impossible to complete (§4).
//!
//! Nothing here touches the file system.

use serde::{Deserialize, Serialize};

/// Longest accepted single path segment.
pub const MAX_NAME_CHARS: usize = 255;

/// Windows device names that cannot be used as a file stem.
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Why a name or path was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathError {
    EmptyName,
    NameTooLong {
        chars: usize,
        limit: usize,
    },
    /// A separator, or a character Windows forbids in a name.
    IllegalCharacter {
        character: String,
    },
    /// A trailing dot or space: Windows silently strips them, which turns one name
    /// into another.
    TrailingDotOrSpace,
    /// `con`, `nul`, `com1` and similar.
    ReservedName {
        name: String,
    },
    /// The path escapes the vault root.
    EscapesRoot {
        path: String,
    },
    /// The destination is inside the item being moved.
    NestedInsideSelf {
        path: String,
        destination: String,
    },
    /// A file already occupies the destination, ignoring case (F03-R01).
    CaseCollision {
        existing: String,
        requested: String,
    },
}

/// Validates one path segment (a file or folder name).
pub fn validate_name(name: &str) -> Result<String, PathError> {
    // The trailing-dot/space check must run on the *original* string: trimming
    // first would hide exactly the case Windows silently rewrites.
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(PathError::TrailingDotOrSpace);
    }
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(PathError::EmptyName);
    }
    let chars = trimmed.chars().count();
    if chars > MAX_NAME_CHARS {
        return Err(PathError::NameTooLong {
            chars,
            limit: MAX_NAME_CHARS,
        });
    }
    for character in trimmed.chars() {
        // `/` and `\` are separators; the rest are illegal on Windows.
        if matches!(
            character,
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
        ) {
            return Err(PathError::IllegalCharacter {
                character: character.to_string(),
            });
        }
    }
    // The device name check applies to the stem, so `con.md` is still refused.
    let stem = trimmed
        .split_once('.')
        .map_or(trimmed, |(before, _)| before)
        .to_ascii_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(PathError::ReservedName {
            name: trimmed.to_owned(),
        });
    }
    Ok(trimmed.to_owned())
}

/// Normalises a vault-relative path.
///
/// Returns the path with `/` separators and no leading or trailing separator, or
/// an error when it is absolute or escapes the root.
pub fn safe_relative(path: &str) -> Result<String, PathError> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err(PathError::EmptyName);
    }
    if normalized.starts_with('/') || normalized.chars().nth(1) == Some(':') {
        return Err(PathError::EscapesRoot {
            path: path.to_owned(),
        });
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                // A `..` that climbs above the root escapes it; one that returns to
                // the root is simply dropped.
                if segments.pop().is_none() {
                    return Err(PathError::EscapesRoot {
                        path: path.to_owned(),
                    });
                }
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return Err(PathError::EmptyName);
    }
    Ok(segments.join("/"))
}

/// Case-folded comparison key, matching Windows' case-insensitive lookups.
pub fn fold_for_comparison(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Whether a destination is inside the item being moved.
///
/// Moving `a` into `a/b` cannot finish: the destination would move with its own
/// parent. Equal paths are not nesting, so a no-op rename is allowed.
pub fn nests_inside(source: &str, destination: &str) -> bool {
    let source = fold_for_comparison(source);
    let destination = fold_for_comparison(destination);
    destination.len() > source.len()
        && destination.starts_with(&source)
        && destination.as_bytes().get(source.len()) == Some(&b'/')
}

/// Checks a rename or move against the existing paths in the vault.
///
/// The comparison is case-insensitive on purpose: on Windows `Notes.md` and
/// `notes.md` are one file, so accepting the second would overwrite the first
/// without warning (F03-R01).
pub fn check_destination(
    source: &str,
    destination: &str,
    existing: &[String],
) -> Result<String, PathError> {
    let destination = safe_relative(destination)?;
    let source_normalized = source.replace('\\', "/");
    if nests_inside(&source_normalized, &destination) {
        return Err(PathError::NestedInsideSelf {
            path: source.to_owned(),
            destination: destination.clone(),
        });
    }
    let folded_source = fold_for_comparison(&source_normalized);
    let folded_destination = fold_for_comparison(&destination);
    for path in existing {
        let folded = fold_for_comparison(path);
        if folded == folded_destination && folded_destination != folded_source {
            // A different file already owns this name once case is ignored.
            return Err(PathError::CaseCollision {
                existing: path.clone(),
                requested: destination.clone(),
            });
        }
    }
    Ok(destination)
}

/// Whether a rename changes only the letter case of a path.
///
/// Windows needs a two-step rename for this — a direct rename to the same name in
/// a different case is a no-op — so the caller must be told.
pub fn is_case_only_change(source: &str, destination: &str) -> bool {
    let source = source.replace('\\', "/");
    let destination = destination.replace('\\', "/");
    source != destination && fold_for_comparison(&source) == fold_for_comparison(&destination)
}

/// One item's outcome in a batch operation (F03-R04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub path: String,
    /// `done`, `skipped` or `failed`.
    pub status: String,
    /// Why it did not complete, as a message key.
    pub reason_key: Option<String>,
}

/// The outcome of a batch operation.
///
/// A batch is explicitly **not** atomic across files: `completed` and `failed` are
/// both reported so an interruption leaves the caller knowing exactly what
/// happened (F03-R04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    pub items: Vec<ItemResult>,
    pub completed: usize,
    pub failed: usize,
    /// True only when every item completed.
    pub all_succeeded: bool,
    /// True when the batch stopped early, so the caller can resume.
    pub interrupted: bool,
}

impl BatchResult {
    /// Builds a result from per-item outcomes.
    pub fn from_items(items: Vec<ItemResult>, interrupted: bool) -> Self {
        let completed = items.iter().filter(|item| item.status == "done").count();
        let failed = items.iter().filter(|item| item.status == "failed").count();
        Self {
            all_succeeded: !interrupted && failed == 0 && completed == items.len(),
            completed,
            failed,
            items,
            interrupted,
        }
    }

    /// Items that did not complete, so a retry can target only those.
    pub fn remaining(&self) -> Vec<&ItemResult> {
        self.items
            .iter()
            .filter(|item| item.status != "done")
            .collect()
    }
}

/// Plans a batch rename, reporting every item before anything is written.
///
/// `existing` is the current set of vault paths; each destination is checked
/// against it and against the other destinations in the same batch, so two items
/// cannot claim one name.
pub fn plan_batch(pairs: &[(String, String)], existing: &[String]) -> Vec<ItemResult> {
    let mut claimed: Vec<String> = Vec::new();
    let mut results: Vec<ItemResult> = Vec::new();
    for (source, destination) in pairs {
        match check_destination(source, destination, existing) {
            Ok(path) => {
                let folded = fold_for_comparison(&path);
                if claimed.contains(&folded) {
                    results.push(ItemResult {
                        path: source.clone(),
                        status: "failed".to_owned(),
                        reason_key: Some("error.pathCollision".to_owned()),
                    });
                    continue;
                }
                claimed.push(folded);
                results.push(ItemResult {
                    path: source.clone(),
                    status: "done".to_owned(),
                    reason_key: None,
                });
            }
            Err(error) => {
                results.push(ItemResult {
                    path: source.clone(),
                    status: "failed".to_owned(),
                    reason_key: Some(reason_key(&error).to_owned()),
                });
            }
        }
    }
    results
}

/// The message key for a path error.
pub fn reason_key(error: &PathError) -> &'static str {
    match error {
        PathError::EmptyName => "error.nameEmpty",
        PathError::NameTooLong { .. } => "error.nameTooLong",
        PathError::IllegalCharacter { .. } => "error.nameIllegalCharacter",
        PathError::TrailingDotOrSpace => "error.nameTrailingDotOrSpace",
        PathError::ReservedName { .. } => "error.nameReserved",
        PathError::EscapesRoot { .. } => "error.pathEscapesRoot",
        PathError::NestedInsideSelf { .. } => "error.pathNestedInsideSelf",
        PathError::CaseCollision { .. } => "error.pathCollision",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_accepted_unchanged() {
        assert_eq!(
            validate_name("Weekly review"),
            Ok("Weekly review".to_owned())
        );
        // Leading whitespace is trimmed, but a trailing space is refused below
        // because Windows would strip it silently.
        assert_eq!(validate_name("  京都"), Ok("京都".to_owned()));
    }

    #[test]
    fn an_empty_or_whitespace_name_is_refused() {
        assert_eq!(validate_name(""), Err(PathError::EmptyName));
        // Whitespace-only input has a trailing space, which is refused first.
        assert!(matches!(
            validate_name("   "),
            Err(PathError::TrailingDotOrSpace) | Err(PathError::EmptyName)
        ));
    }

    #[test]
    fn separators_and_windows_special_characters_are_refused() {
        for name in [
            "a/b", "a\\b", "a:b", "a*b", "a?", "a\"b", "a<b", "a>b", "a|b",
        ] {
            assert!(
                matches!(validate_name(name), Err(PathError::IllegalCharacter { .. })),
                "{name} should be refused"
            );
        }
    }

    #[test]
    fn a_trailing_dot_or_space_is_refused() {
        // Windows strips these silently, turning one name into another.
        assert_eq!(validate_name("notes."), Err(PathError::TrailingDotOrSpace));
        assert_eq!(validate_name("notes "), Err(PathError::TrailingDotOrSpace));
    }

    #[test]
    fn reserved_device_names_are_refused_even_with_an_extension() {
        assert_eq!(
            validate_name("con"),
            Err(PathError::ReservedName {
                name: "con".to_owned()
            })
        );
        assert!(matches!(
            validate_name("NUL.md"),
            Err(PathError::ReservedName { .. })
        ));
        assert!(matches!(
            validate_name("com1.txt"),
            Err(PathError::ReservedName { .. })
        ));
        // A name that merely starts with a reserved word is fine.
        assert!(validate_name("console").is_ok());
    }

    #[test]
    fn an_overlong_name_is_refused_with_the_limit() {
        let long = "a".repeat(300);
        assert_eq!(
            validate_name(&long),
            Err(PathError::NameTooLong {
                chars: 300,
                limit: MAX_NAME_CHARS
            })
        );
    }

    #[test]
    fn a_relative_path_is_normalised() {
        assert_eq!(safe_relative("a/b/c.md"), Ok("a/b/c.md".to_owned()));
        assert_eq!(safe_relative("a\\b\\c.md"), Ok("a/b/c.md".to_owned()));
        assert_eq!(safe_relative("./a//b.md"), Ok("a/b.md".to_owned()));
        // A `..` that returns to the root is dropped, not an error.
        assert_eq!(safe_relative("a/../b.md"), Ok("b.md".to_owned()));
    }

    #[test]
    fn a_path_escaping_the_root_is_refused() {
        for path in ["../x.md", "a/../../x.md", "/etc/passwd", "C:\\vault\\x.md"] {
            assert!(
                matches!(safe_relative(path), Err(PathError::EscapesRoot { .. })),
                "{path} should be refused"
            );
        }
    }

    #[test]
    fn a_name_differing_only_in_case_is_a_collision_on_windows() {
        // F03-R01: Windows treats these as one file.
        let existing = vec!["Notes.md".to_owned()];
        let error = check_destination("other.md", "notes.md", &existing).expect_err("collision");
        assert_eq!(
            error,
            PathError::CaseCollision {
                existing: "Notes.md".to_owned(),
                requested: "notes.md".to_owned()
            }
        );
    }

    #[test]
    fn renaming_a_note_to_its_own_name_is_allowed() {
        // A case-only rename must not collide with itself.
        let existing = vec!["Notes.md".to_owned()];
        assert_eq!(
            check_destination("Notes.md", "notes.md", &existing),
            Ok("notes.md".to_owned())
        );
        assert!(is_case_only_change("Notes.md", "notes.md"));
    }

    #[test]
    fn a_case_only_change_is_reported_so_the_caller_can_do_it_in_two_steps() {
        assert!(is_case_only_change("a/B.md", "a/b.md"));
        assert!(!is_case_only_change("a/b.md", "a/c.md"));
        assert!(!is_case_only_change("a/b.md", "a/b.md"));
    }

    #[test]
    fn a_destination_inside_the_moved_item_is_refused() {
        let error = check_destination("notes", "notes/sub", &[]).expect_err("nested");
        assert!(matches!(error, PathError::NestedInsideSelf { .. }));
        // A sibling is fine.
        assert!(check_destination("notes", "archive/notes", &[]).is_ok());
    }

    #[test]
    fn nesting_check_ignores_case_and_requires_a_segment_boundary() {
        assert!(nests_inside("Notes", "notes/sub"));
        // `notesX` is a different directory, not a child.
        assert!(!nests_inside("notes", "notesX/sub"));
        assert!(!nests_inside("notes", "notes"));
    }

    #[test]
    fn a_batch_reports_each_item_and_never_claims_atomicity() {
        // F03-R04: per-item results, and an interruption is visible.
        let existing = vec!["taken.md".to_owned()];
        let pairs = vec![
            ("a.md".to_owned(), "b.md".to_owned()),
            ("c.md".to_owned(), "taken.md".to_owned()),
            ("d.md".to_owned(), "e.md".to_owned()),
        ];
        let results = plan_batch(&pairs, &existing);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].status, "done");
        assert_eq!(results[1].status, "failed");
        assert_eq!(
            results[1].reason_key.as_deref(),
            Some("error.pathCollision")
        );
        assert_eq!(results[2].status, "done");

        let batch = BatchResult::from_items(results, false);
        assert_eq!(batch.completed, 2);
        assert_eq!(batch.failed, 1);
        assert!(!batch.all_succeeded);
        assert_eq!(batch.remaining().len(), 1);
    }

    #[test]
    fn two_items_in_one_batch_cannot_claim_the_same_name() {
        let pairs = vec![
            ("a.md".to_owned(), "x.md".to_owned()),
            ("b.md".to_owned(), "X.md".to_owned()),
        ];
        let results = plan_batch(&pairs, &[]);
        assert_eq!(results[0].status, "done");
        // Case-insensitively the same destination, so the second must fail.
        assert_eq!(results[1].status, "failed");
    }

    #[test]
    fn an_interrupted_batch_is_not_reported_as_success() {
        let items = vec![ItemResult {
            path: "a.md".to_owned(),
            status: "done".to_owned(),
            reason_key: None,
        }];
        let batch = BatchResult::from_items(items, true);
        assert_eq!(batch.completed, 1);
        assert!(!batch.all_succeeded);
        assert!(batch.interrupted);
    }

    #[test]
    fn every_error_maps_to_a_message_key() {
        let errors = [
            PathError::EmptyName,
            PathError::NameTooLong { chars: 1, limit: 1 },
            PathError::IllegalCharacter {
                character: "/".to_owned(),
            },
            PathError::TrailingDotOrSpace,
            PathError::ReservedName {
                name: "con".to_owned(),
            },
            PathError::EscapesRoot {
                path: "../x".to_owned(),
            },
            PathError::NestedInsideSelf {
                path: "a".to_owned(),
                destination: "a/b".to_owned(),
            },
            PathError::CaseCollision {
                existing: "A".to_owned(),
                requested: "a".to_owned(),
            },
        ];
        for error in errors {
            let key = reason_key(&error);
            assert!(key.starts_with("error."), "{error:?} -> {key}");
        }
    }
}
