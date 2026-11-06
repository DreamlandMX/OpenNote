//! M06 daily-note resolution (F17).
//!
//! A daily note is an ordinary note whose path is derived from a calendar day.
//! Three rules shape this module:
//!
//! * the day is a calendar date supplied by the caller, never a clock reading, so
//!   a device in another zone cannot land on a different file (F17 §4);
//! * an existing note wins over creating one — the same date twice must return
//!   the same note, not a second file (F17-R01);
//! * a renamed note is still found through its `date` property, and two notes
//!   claiming one day are reported as ambiguous instead of being merged
//!   (F17-R03, F17-R04).
//!
//! Nothing here touches the filesystem; the host supplies the candidate list and
//! performs the create.

use serde::{Deserialize, Serialize};

/// Default folder pattern from F17 §4: `Daily/YYYY`, file `YYYY-MM-DD.md`.
pub const DEFAULT_FOLDER_PATTERN: &str = "Daily/YYYY";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DailyError {
    /// The supplied day is not a real `YYYY-MM-DD` date.
    InvalidDate(String),
    /// A pattern token that is not `YYYY`, `MM` or `DD`.
    InvalidPattern(String),
    /// The pattern would place the file outside the vault.
    UnsafePattern(String),
}

/// One note that might be a daily note for the requested day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyCandidate {
    pub note_id: String,
    pub relative_path: String,
    /// Frontmatter `date` value, when the note has one.
    pub property_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchKind {
    /// The path is exactly what the pattern produces.
    Path,
    /// The note was renamed; its `date` property still names the day.
    Property,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DailyMatch {
    Found {
        note_id: String,
        relative_path: String,
        matched_by: MatchKind,
    },
    /// More than one note claims this day; the caller must let the user choose.
    Ambiguous {
        candidates: Vec<DailyCandidate>,
    },
    Missing,
}

fn parts(date: &str) -> Option<(u32, u32, u32)> {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return None;
    }
    let year: u32 = date[0..4].parse().ok()?;
    let month: u32 = date[5..7].parse().ok()?;
    let day: u32 = date[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let limit = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    (day <= limit).then_some((year, month, day))
}

/// Replaces `YYYY`, `MM` and `DD` in `pattern`.
///
/// Unknown tokens are refused rather than left in a path, because a literal
/// `{{day}}` in a file name would silently create one note that every later day
/// also tries to use.
pub fn expand_pattern(pattern: &str, date: &str) -> Result<String, DailyError> {
    let Some((year, month, day)) = parts(date) else {
        return Err(DailyError::InvalidDate(date.to_owned()));
    };
    let raw = pattern.trim().replace('\\', "/");
    if raw.is_empty() {
        return Ok(String::new());
    }
    if raw.starts_with('/') || raw.split('/').any(|segment| segment == "..") {
        return Err(DailyError::UnsafePattern(pattern.to_owned()));
    }
    // A brace in a pattern is an intended placeholder this build does not know;
    // refusing it is better than creating one file every day tries to reuse.
    if raw.contains('{') || raw.contains('}') {
        return Err(DailyError::InvalidPattern(pattern.to_owned()));
    }
    // Only the doubled tokens are placeholders. Single `Y`/`M`/`D` letters are
    // ordinary text, so a folder named "Daily" or "May" stays usable.
    let mut output = String::with_capacity(raw.len());
    let mut rest = raw.as_str();
    while let Some(index) = rest
        .find("YYYY")
        .or_else(|| rest.find("MM"))
        .or_else(|| rest.find("DD"))
    {
        output.push_str(&rest[..index]);
        let tail = &rest[index..];
        if tail.starts_with("YYYY") {
            output.push_str(&format!("{year:04}"));
            rest = &tail[4..];
        } else if tail.starts_with("MM") {
            output.push_str(&format!("{month:02}"));
            rest = &tail[2..];
        } else {
            output.push_str(&format!("{day:02}"));
            rest = &tail[2..];
        }
    }
    output.push_str(rest);
    let segments: Vec<&str> = output
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    if segments.is_empty() {
        return Ok(String::new());
    }
    Ok(segments.join("/"))
}

/// Vault-relative path of the daily note for `date`.
pub fn daily_path(date: &str, folder_pattern: &str) -> Result<String, DailyError> {
    if parts(date).is_none() {
        return Err(DailyError::InvalidDate(date.to_owned()));
    }
    let folder = expand_pattern(folder_pattern, date)?;
    Ok(if folder.is_empty() {
        format!("{date}.md")
    } else {
        format!("{folder}/{date}.md")
    })
}

/// Finds the note for `date`: an exact path match wins, then a `date` property.
///
/// Two matches of any kind are ambiguous, and a path match plus a different
/// property match is ambiguous too: merging them would attach one day's writing
/// to another day's note (F17-R04).
pub fn find_daily(
    date: &str,
    folder_pattern: &str,
    candidates: &[DailyCandidate],
) -> Result<DailyMatch, DailyError> {
    let expected = daily_path(date, folder_pattern)?;
    let mut path_matches: Vec<&DailyCandidate> = Vec::new();
    let mut property_matches: Vec<&DailyCandidate> = Vec::new();
    for candidate in candidates {
        if candidate.relative_path.replace('\\', "/") == expected {
            path_matches.push(candidate);
        } else if candidate.property_date.as_deref() == Some(date) {
            property_matches.push(candidate);
        }
    }
    let total = path_matches.len() + property_matches.len();
    if total == 0 {
        return Ok(DailyMatch::Missing);
    }
    if total > 1 {
        let mut all: Vec<DailyCandidate> = path_matches
            .iter()
            .chain(property_matches.iter())
            .map(|candidate| (*candidate).clone())
            .collect();
        all.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        return Ok(DailyMatch::Ambiguous { candidates: all });
    }
    let (found, kind) = match path_matches.first() {
        Some(candidate) => (*candidate, MatchKind::Path),
        None => (property_matches[0], MatchKind::Property),
    };
    Ok(DailyMatch::Found {
        note_id: found.note_id.clone(),
        relative_path: found.relative_path.clone(),
        matched_by: kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, path: &str, property: Option<&str>) -> DailyCandidate {
        DailyCandidate {
            note_id: id.to_owned(),
            relative_path: path.to_owned(),
            property_date: property.map(|value| value.to_owned()),
        }
    }

    #[test]
    fn the_default_pattern_builds_the_documented_path() {
        assert_eq!(
            daily_path("2026-10-20", DEFAULT_FOLDER_PATTERN).unwrap(),
            "Daily/2026/2026-10-20.md"
        );
    }

    #[test]
    fn month_and_day_tokens_are_expanded_and_zero_padded() {
        assert_eq!(
            daily_path("2026-03-05", "Journal/YYYY/MM/DD").unwrap(),
            "Journal/2026/03/05/2026-03-05.md"
        );
        assert_eq!(
            daily_path("2026-03-05", "YYYY-MM").unwrap(),
            "2026-03/2026-03-05.md"
        );
    }

    #[test]
    fn an_empty_pattern_puts_the_note_at_the_vault_root() {
        assert_eq!(daily_path("2026-10-20", "").unwrap(), "2026-10-20.md");
        assert_eq!(daily_path("2026-10-20", "  ").unwrap(), "2026-10-20.md");
    }

    #[test]
    fn invalid_dates_are_refused_before_any_path_exists() {
        for date in ["2026-02-30", "2026-13-01", "2026-1-1", "", "today"] {
            assert_eq!(
                daily_path(date, DEFAULT_FOLDER_PATTERN),
                Err(DailyError::InvalidDate(date.to_owned())),
                "{date}"
            );
        }
        assert!(daily_path("2024-02-29", DEFAULT_FOLDER_PATTERN).is_ok());
    }

    #[test]
    fn unsafe_or_unknown_patterns_are_refused() {
        assert_eq!(
            daily_path("2026-10-20", "../outside"),
            Err(DailyError::UnsafePattern("../outside".to_owned()))
        );
        assert_eq!(
            daily_path("2026-10-20", "/absolute"),
            Err(DailyError::UnsafePattern("/absolute".to_owned()))
        );
        assert_eq!(
            daily_path("2026-10-20", "Daily/{day}"),
            Err(DailyError::InvalidPattern("Daily/{day}".to_owned()))
        );
        // A single letter is ordinary text, so these folder names keep working.
        assert_eq!(
            daily_path("2026-10-20", "Daily/Y").unwrap(),
            "Daily/Y/2026-10-20.md"
        );
        assert_eq!(
            daily_path("2025-05-01", "May").unwrap(),
            "May/2025-05-01.md"
        );
    }

    #[test]
    fn an_exact_path_match_is_reported_as_path() {
        let candidates = vec![candidate("n1", "Daily/2026/2026-10-20.md", None)];
        assert_eq!(
            find_daily("2026-10-20", DEFAULT_FOLDER_PATTERN, &candidates).unwrap(),
            DailyMatch::Found {
                note_id: "n1".to_owned(),
                relative_path: "Daily/2026/2026-10-20.md".to_owned(),
                matched_by: MatchKind::Path
            }
        );
    }

    #[test]
    fn a_renamed_note_is_found_through_its_date_property() {
        // F17-R03: the file moved, the day did not.
        let candidates = vec![candidate("n1", "Archive/renamed.md", Some("2026-10-20"))];
        assert_eq!(
            find_daily("2026-10-20", DEFAULT_FOLDER_PATTERN, &candidates).unwrap(),
            DailyMatch::Found {
                note_id: "n1".to_owned(),
                relative_path: "Archive/renamed.md".to_owned(),
                matched_by: MatchKind::Property
            }
        );
    }

    #[test]
    fn two_notes_claiming_one_day_are_ambiguous_not_merged() {
        let candidates = vec![
            candidate("n1", "Daily/2026/2026-10-20.md", None),
            candidate("n2", "Copies/2026-10-20.md", Some("2026-10-20")),
        ];
        let result = find_daily("2026-10-20", DEFAULT_FOLDER_PATTERN, &candidates).unwrap();
        match result {
            DailyMatch::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
                // Sorted by path so the chooser is stable.
                assert_eq!(candidates[0].note_id, "n2");
            }
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn no_match_is_missing_and_unrelated_notes_are_ignored() {
        let candidates = vec![
            candidate("n1", "Daily/2026/2026-10-19.md", Some("2026-10-19")),
            candidate("n2", "notes/other.md", None),
        ];
        assert_eq!(
            find_daily("2026-10-20", DEFAULT_FOLDER_PATTERN, &candidates).unwrap(),
            DailyMatch::Missing
        );
    }

    #[test]
    fn windows_separators_in_a_stored_path_still_match() {
        let candidates = vec![candidate("n1", "Daily\\2026\\2026-10-20.md", None)];
        assert!(matches!(
            find_daily("2026-10-20", DEFAULT_FOLDER_PATTERN, &candidates).unwrap(),
            DailyMatch::Found { .. }
        ));
    }
}
