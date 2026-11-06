//! M06 task projection: parse Markdown task lines and write one back.
//!
//! Two rules drive this crate:
//!
//! * the Markdown line is the only authority (F16-R01): a task is `- [ ]` or
//!   `- [x]` in the note, and toggling rewrites that exact line;
//! * a reference must be locatable again before writing (F16-R03): a task is
//!   identified by its source offset *and* its text, so a stale index cannot
//!   tick a different line that happens to look the same (F16-A01).
//!
//! Nothing here reads or writes a file; the host supplies the source and saves
//! the returned source through M02.

pub mod canvas;
pub use canvas::{
    broken_nodes, export_disclosure, parse as parse_canvas, references as canvas_references,
    remove_node as remove_canvas_node, serialize as serialize_canvas, validate as validate_canvas,
    BrokenNode, CanvasDocument, CanvasEdge, CanvasNode, CanvasReference, ExportDisclosure,
    GeometryError, FORMAT_VERSION as CANVAS_FORMAT_VERSION, MIN_NODE_HEIGHT, MIN_NODE_WIDTH,
};
pub mod daily;
pub mod database;
pub use database::{
    build_table, calendar_cells, columns_for, group_by_select, CellValue, Column, Filter, FilterOp,
    Group, Row, RowSource, Sort, SortDirection, Table,
};

pub use daily::{
    daily_path, expand_pattern as expand_daily_pattern, find_daily, DailyCandidate, DailyError,
    DailyMatch, MatchKind, DEFAULT_FOLDER_PATTERN,
};
pub mod templates;
pub use templates::{
    expand as expand_template, file_name_for, is_template, Expansion, ExpansionWarning,
    TemplateVariables, CURSOR_MARKER, KNOWN_VARIABLES,
};

use onote_metadata_links::Scanner;
use serde::{Deserialize, Serialize};

/// One parsed task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Byte offset of the line start in the full source.
    pub offset: usize,
    /// Zero-based line index, for ordering and diagnostics.
    pub line: usize,
    pub status: TaskStatus,
    /// Text after the checkbox, trimmed of trailing whitespace.
    pub text: String,
    /// `[due:: YYYY-MM-DD]` when present and well formed.
    pub due: Option<String>,
    /// Nesting depth from leading spaces (two spaces or a tab per level).
    pub depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Open,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskError {
    /// The source no longer contains the task at its recorded offset.
    Moved { expected: String, found: String },
    /// Two tasks share the same rendered text and offset, so the target is
    /// ambiguous and must not be guessed.
    Ambiguous,
    /// No task starts at the recorded offset.
    Missing,
    /// A due date that is not `YYYY-MM-DD`.
    InvalidDate(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggledSource {
    pub source: String,
    pub status: TaskStatus,
    /// Offset of the same line in the new source; unchanged today, but the
    /// caller should use it rather than assume that.
    pub offset: usize,
}

/// A bullet checkbox line: `- [ ]`, `* [x]`, `+ [X]`.
struct Marker {
    indent: usize,
    bullet: char,
    checked: bool,
    /// Byte offset just past the closing `]`.
    content_start: usize,
}

fn marker_of(line: &str) -> Option<Marker> {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
        index += 1;
    }
    let indent = index;
    let bullet = *bytes.get(index)? as char;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    index += 1;
    // A bullet needs one space before the checkbox.
    if bytes.get(index) != Some(&b' ') {
        return None;
    }
    index += 1;
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    index += 1;
    let state = *bytes.get(index)?;
    let checked = match state {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    index += 1;
    if bytes.get(index) != Some(&b']') {
        return None;
    }
    index += 1;
    // A checkbox must be followed by a space or the end of the line, otherwise
    // `- [x]note` is prose rather than a task.
    if index < bytes.len() && bytes[index] != b' ' && bytes[index] != b'\t' {
        return None;
    }
    Some(Marker {
        indent,
        bullet,
        checked,
        content_start: index + 1,
    })
}

fn depth_of(indent: usize) -> usize {
    // Two spaces per level, which is what the editor's list continuation
    // produces. A tab counts as one byte toward the indent, so a tab-indented
    // list stays at depth 0 rather than being guessed into a level.
    indent / 2
}

/// `[due:: 2026-10-15]`, optionally anywhere in the text.
fn due_of(text: &str) -> Option<String> {
    let marker = "[due::";
    let lower = text.to_ascii_lowercase();
    let start = lower.find(marker)?;
    let rest = &text[start + marker.len()..];
    let end = rest.find(']')?;
    let value = rest[..end].trim();
    if is_calendar_date(value) {
        Some(value.to_owned())
    } else {
        None
    }
}

/// A strict `YYYY-MM-DD` check that also rejects impossible days.
pub fn is_calendar_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }
    let year: i32 = value[0..4].parse().unwrap_or(0);
    let month: u32 = value[5..7].parse().unwrap_or(0);
    let day: u32 = value[8..10].parse().unwrap_or(0);
    if !(1..=12).contains(&month) || day == 0 {
        return false;
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
        _ => return false,
    };
    day <= limit
}

/// Every task in the note, in document order.
///
/// Code fences and inline code are masked by the shared scanner, so an example
/// inside a fenced block is never reported as a task (F16 §4's level rule and
/// the same rule F08 uses for tags).
pub fn parse_tasks(source: &str) -> Vec<Task> {
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let body_start = scanner.body_start();
    let mut tasks = Vec::new();
    let mut offset = 0usize;
    for (index, line) in source.split('\n').enumerate() {
        let line_start = offset;
        offset += line.len() + 1;
        if line_start < body_start {
            continue;
        }
        let visible = masked
            .get(line_start..line_start + line.len())
            .unwrap_or(line);
        let Some(marker) = marker_of(visible) else {
            continue;
        };
        // Read the text from the original line so masking never eats content.
        let text = line
            .get(marker.content_start..)
            .unwrap_or("")
            .trim_end()
            .to_owned();
        let _ = marker.bullet;
        tasks.push(Task {
            offset: line_start,
            line: index,
            status: if marker.checked {
                TaskStatus::Done
            } else {
                TaskStatus::Open
            },
            due: due_of(&text),
            text,
            depth: depth_of(marker.indent),
        });
    }
    tasks
}

/// Rewrites the checkbox of the task at `offset` whose text still matches.
///
/// F16-R03: the recorded offset and text must agree with what is on disk now.
/// If the line moved (a note edited elsewhere), this returns `Moved` with both
/// texts so the caller can ask the user, instead of ticking the wrong line.
pub fn set_task_status(
    source: &str,
    offset: usize,
    expected_text: &str,
    status: TaskStatus,
) -> Result<ToggledSource, TaskError> {
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let line_start = source[..offset.min(source.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    if line_start != offset {
        // The offset is not a line start; the caller's reference is stale.
        return Err(TaskError::Missing);
    }
    let line_end = source[offset..]
        .find('\n')
        .map_or(source.len(), |index| offset + index);
    let line = &source[offset..line_end];
    let visible = masked.get(offset..line_end).unwrap_or(line);
    let Some(marker) = marker_of(visible) else {
        return Err(TaskError::Missing);
    };
    let current_text = line
        .get(marker.content_start..)
        .unwrap_or("")
        .trim_end()
        .to_owned();
    if current_text != expected_text.trim_end() {
        return Err(TaskError::Moved {
            expected: expected_text.to_owned(),
            found: current_text,
        });
    }
    let desired = matches!(status, TaskStatus::Done);
    if marker.checked == desired {
        return Ok(ToggledSource {
            source: source.to_owned(),
            status,
            offset,
        });
    }
    // `content_start` points past `]` plus one separating space, so the
    // checkbox byte is three positions back: `[x] ` -> `[`, `x`, `]`, ` `.
    let checkbox = offset + marker.content_start - 3;
    let mut next = source.to_owned();
    let replacement = if desired { "x" } else { " " };
    next.replace_range(checkbox..checkbox + 1, replacement);
    Ok(ToggledSource {
        source: next,
        status,
        offset,
    })
}

/// Result of rewriting one task line in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRewrite {
    pub source: String,
    /// Offset of the line in the new source, or None when the line was removed.
    pub offset: Option<usize>,
}

/// Locates the task line a reference points at, guarding against a stale offset.
fn locate_line(
    source: &str,
    masked: &str,
    offset: usize,
    expected_text: &str,
) -> Result<(usize, usize, Marker), TaskError> {
    let line_start = source[..offset.min(source.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    if line_start != offset {
        return Err(TaskError::Missing);
    }
    let line_end = source[offset..]
        .find('\n')
        .map_or(source.len(), |index| offset + index);
    let line = &source[offset..line_end];
    let visible = masked.get(offset..line_end).unwrap_or(line);
    let Some(marker) = marker_of(visible) else {
        return Err(TaskError::Missing);
    };
    let current_text = line
        .get(marker.content_start..)
        .unwrap_or("")
        .trim_end()
        .to_owned();
    if current_text != expected_text.trim_end() {
        return Err(TaskError::Moved {
            expected: expected_text.to_owned(),
            found: current_text,
        });
    }
    Ok((line_start, line_end, marker))
}

/// Deletes the whole task line (F16-R04).
///
/// Deleting a task is a different action from marking it done: completing keeps
/// the record, deleting removes the line from the note. The caller must never
/// treat them as one operation, so this is a separate function rather than a
/// status value.
pub fn delete_task(
    source: &str,
    offset: usize,
    expected_text: &str,
) -> Result<TaskRewrite, TaskError> {
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let (line_start, line_end, _) = locate_line(source, &masked, offset, expected_text)?;
    // Remove the newline too, so deleting one line does not leave a blank line
    // where the task used to be.
    let end = if line_end < source.len() {
        line_end + 1
    } else if line_start > 0 {
        line_start
    } else {
        line_end
    };
    let remove_from = if line_end >= source.len() && line_start > 0 {
        // The last line had no newline; also drop the preceding one so the file
        // does not end with a stray blank line.
        line_start - 1
    } else {
        line_start
    };
    let mut next = String::with_capacity(source.len());
    next.push_str(&source[..remove_from]);
    next.push_str(&source[end.min(source.len())..]);
    Ok(TaskRewrite {
        source: next,
        offset: None,
    })
}

/// Sets, replaces or clears the `[due:: ...]` marker on one task (F16 §4).
///
/// `date` of `None` removes the marker. An invalid date is refused rather than
/// written, because a malformed date would make the task disappear from every
/// date group without telling the user why.
pub fn set_task_due(
    source: &str,
    offset: usize,
    expected_text: &str,
    date: Option<&str>,
) -> Result<TaskRewrite, TaskError> {
    if let Some(value) = date {
        if !is_calendar_date(value) {
            return Err(TaskError::InvalidDate(value.to_owned()));
        }
    }
    let scanner = Scanner::new(source);
    let masked = scanner.masked_source();
    let (line_start, line_end, marker) = locate_line(source, &masked, offset, expected_text)?;
    let line = &source[line_start..line_end];
    let text_start = line_start + marker.content_start;
    let existing = due_span(line, marker.content_start);

    let (replace_from, replace_to, replacement) = match (existing, date) {
        (Some((start, end)), Some(value)) => (start, end, format!("[due:: {value}]")),
        (Some((start, end)), None) => {
            // Remove the marker and the single space that separated it, so the
            // text does not end with a double space.
            let mut from = start;
            if from > 0 && source.as_bytes().get(from - 1) == Some(&b' ') {
                from -= 1;
            }
            (from, end, String::new())
        }
        (None, Some(value)) => (line_end, line_end, format!(" [due:: {value}]")),
        (None, None) => (line_end, line_end, String::new()),
    };
    let mut next = String::with_capacity(source.len() + 16);
    next.push_str(&source[..line_start + replace_from]);
    next.push_str(&replacement);
    next.push_str(&source[line_start + replace_to..]);
    let _ = text_start;
    Ok(TaskRewrite {
        source: next,
        offset: Some(line_start),
    })
}

/// Byte span of an existing `[due:: ...]` marker within a line, if any.
fn due_span(line: &str, from: usize) -> Option<(usize, usize)> {
    let rest = line.get(from..)?;
    let lower = rest.to_ascii_lowercase();
    let start = lower.find("[due::")?;
    let end = rest[start..].find(']')? + start + 1;
    Some((from + start, from + end))
}

/// The task a reference points at, or why it cannot be used.
pub fn locate_task<'a>(
    tasks: &'a [Task],
    offset: usize,
    expected_text: &str,
) -> Result<&'a Task, TaskError> {
    let matches: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.offset == offset && task.text == expected_text)
        .collect();
    match matches.len() {
        0 => Err(TaskError::Missing),
        1 => Ok(matches[0]),
        // Two tasks with the same offset cannot exist in one parse, so this arm
        // only fires for a caller-supplied list with duplicates.
        _ => Err(TaskError::Ambiguous),
    }
}

/// Groups tasks for S24: due today, upcoming, undated, and done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroup {
    pub key: String,
    pub tasks: Vec<usize>,
}

/// Groups by due date relative to `today`.
///
/// F16-R02: `today` is a local calendar date supplied by the caller. This crate
/// never reads the clock, so a device in another zone cannot shift a group.
pub fn group_tasks(tasks: &[Task], today: &str) -> Vec<TaskGroup> {
    let mut groups: Vec<TaskGroup> = Vec::new();
    let mut push = |key: &str, index: usize| match groups.iter_mut().find(|group| group.key == key)
    {
        Some(group) => group.tasks.push(index),
        None => groups.push(TaskGroup {
            key: key.to_owned(),
            tasks: vec![index],
        }),
    };
    for (index, task) in tasks.iter().enumerate() {
        if task.status == TaskStatus::Done {
            push("done", index);
            continue;
        }
        match task.due.as_deref() {
            None => push("undated", index),
            Some(due) if due == today => push("today", index),
            Some(due) if due > today => push("upcoming", index),
            Some(_) => push("overdue", index),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str =
        "# Tasks\n\n- [ ] buy milk\n- [x] ship it\n  - [ ] nested\n- [ ] pay [due:: 2026-10-15]\n";

    #[test]
    fn parses_task_lines_with_status_text_due_and_depth() {
        let tasks = parse_tasks(SOURCE);
        assert_eq!(tasks.len(), 4);
        assert_eq!(tasks[0].text, "buy milk");
        assert_eq!(tasks[0].status, TaskStatus::Open);
        assert_eq!(tasks[0].depth, 0);
        assert_eq!(tasks[1].status, TaskStatus::Done);
        assert_eq!(tasks[2].depth, 1, "two spaces is one level");
        assert_eq!(tasks[3].due.as_deref(), Some("2026-10-15"));
        // The due marker stays part of the text; it is data inside the line.
        assert!(tasks[3].text.contains("[due::"));
    }

    #[test]
    fn ignores_task_like_text_in_code_fences_and_inline_code() {
        let source = "```\n- [ ] not a task\n```\n\n`- [ ] nope`\n\n- [ ] real\n";
        let tasks = parse_tasks(source);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "real");
    }

    #[test]
    fn ignores_frontmatter_and_non_task_lines() {
        let source =
            "---\ndue: 2026-10-15\n---\n- [x]not a task (no space)\n- plain bullet\n* [X] upper\n";
        let tasks = parse_tasks(source);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Done);
    }

    #[test]
    fn accepts_the_three_bullet_characters() {
        let tasks = parse_tasks("- [ ] a\n* [x] b\n+ [ ] c\n");
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[1].status, TaskStatus::Done);
    }

    #[test]
    fn toggling_writes_only_the_checkbox_byte() {
        let tasks = parse_tasks(SOURCE);
        let target = tasks[0].clone();
        let toggled =
            set_task_status(SOURCE, target.offset, &target.text, TaskStatus::Done).expect("toggle");
        assert_eq!(
            toggled.source,
            "# Tasks\n\n- [x] buy milk\n- [x] ship it\n  - [ ] nested\n- [ ] pay [due:: 2026-10-15]\n"
        );
        // Toggling back restores the original bytes exactly.
        let back = set_task_status(
            &toggled.source,
            toggled.offset,
            &target.text,
            TaskStatus::Open,
        )
        .expect("toggle back");
        assert_eq!(back.source, SOURCE);
    }

    #[test]
    fn toggling_an_already_done_task_is_a_no_op() {
        let tasks = parse_tasks(SOURCE);
        let done = &tasks[1];
        let result =
            set_task_status(SOURCE, done.offset, &done.text, TaskStatus::Done).expect("no-op");
        assert_eq!(result.source, SOURCE);
    }

    #[test]
    fn a_moved_line_is_refused_with_both_texts() {
        // F16-R03: the text at the recorded offset changed, so the write must
        // not happen.
        let error = set_task_status(
            SOURCE,
            parse_tasks(SOURCE)[0].offset,
            "different",
            TaskStatus::Done,
        )
        .expect_err("must refuse");
        assert_eq!(
            error,
            TaskError::Moved {
                expected: "different".to_owned(),
                found: "buy milk".to_owned()
            }
        );
    }

    #[test]
    fn an_offset_that_is_not_a_line_start_is_refused() {
        assert_eq!(
            set_task_status(SOURCE, 3, "buy milk", TaskStatus::Done),
            Err(TaskError::Missing)
        );
    }

    #[test]
    fn identical_text_tasks_are_distinguished_by_offset() {
        // F16-A01: two tasks with the same wording; only the referenced one may
        // change.
        let source = "- [ ] same\n- [ ] same\n";
        let tasks = parse_tasks(source);
        assert_eq!(tasks.len(), 2);
        let second = &tasks[1];
        let toggled = set_task_status(source, second.offset, &second.text, TaskStatus::Done)
            .expect("toggle second");
        assert_eq!(toggled.source, "- [ ] same\n- [x] same\n");
        assert_eq!(
            locate_task(&tasks, tasks[0].offset, "same").map(|task| task.offset),
            Ok(tasks[0].offset)
        );
    }

    #[test]
    fn locating_a_stale_reference_reports_missing_or_ambiguous() {
        let tasks = parse_tasks("- [ ] a\n");
        assert_eq!(locate_task(&tasks, 999, "a"), Err(TaskError::Missing));
        let duplicated = vec![tasks[0].clone(), tasks[0].clone()];
        assert_eq!(
            locate_task(&duplicated, tasks[0].offset, "a"),
            Err(TaskError::Ambiguous)
        );
    }

    #[test]
    fn deleting_a_task_removes_the_line_and_is_not_the_same_as_completing() {
        // F16-R04: completing keeps the record; deleting removes the line.
        let source = "- [ ] first\n- [ ] second\n";
        let tasks = parse_tasks(source);
        let deleted = delete_task(source, tasks[0].offset, &tasks[0].text).expect("delete first");
        assert_eq!(deleted.source, "- [ ] second\n");
        assert_eq!(deleted.offset, None);

        // Completing leaves both lines present.
        let completed = set_task_status(source, tasks[0].offset, &tasks[0].text, TaskStatus::Done)
            .expect("complete");
        assert_eq!(completed.source, "- [x] first\n- [ ] second\n");
        assert_ne!(deleted.source, completed.source);
    }

    #[test]
    fn deleting_the_last_line_leaves_no_trailing_blank_line() {
        let source = "- [ ] only\n";
        let tasks = parse_tasks(source);
        let deleted = delete_task(source, tasks[0].offset, &tasks[0].text).expect("delete");
        assert_eq!(deleted.source, "");
    }

    #[test]
    fn deleting_refuses_a_stale_reference() {
        let source = "- [ ] first\n";
        let tasks = parse_tasks(source);
        assert_eq!(
            delete_task(source, tasks[0].offset, "something else"),
            Err(TaskError::Moved {
                expected: "something else".to_owned(),
                found: "first".to_owned()
            })
        );
    }

    #[test]
    fn deleting_one_of_two_identical_tasks_keeps_the_other() {
        let source = "- [ ] same\n- [ ] same\n";
        let tasks = parse_tasks(source);
        let deleted = delete_task(source, tasks[1].offset, &tasks[1].text).expect("delete");
        assert_eq!(deleted.source, "- [ ] same\n");
    }

    #[test]
    fn due_dates_can_be_set_replaced_and_cleared() {
        let source = "- [ ] pay\n";
        let tasks = parse_tasks(source);

        let added = set_task_due(source, tasks[0].offset, &tasks[0].text, Some("2026-10-20"))
            .expect("set due");
        assert_eq!(added.source, "- [ ] pay [due:: 2026-10-20]\n");
        assert_eq!(
            parse_tasks(&added.source)[0].due.as_deref(),
            Some("2026-10-20")
        );

        let replaced = set_task_due(
            &added.source,
            added.offset.unwrap(),
            &parse_tasks(&added.source)[0].text,
            Some("2026-11-01"),
        )
        .expect("replace due");
        assert_eq!(replaced.source, "- [ ] pay [due:: 2026-11-01]\n");

        let cleared = set_task_due(
            &replaced.source,
            replaced.offset.unwrap(),
            &parse_tasks(&replaced.source)[0].text,
            None,
        )
        .expect("clear due");
        assert_eq!(cleared.source, "- [ ] pay\n");
        assert_eq!(parse_tasks(&cleared.source)[0].due, None);
    }

    #[test]
    fn a_due_date_that_is_not_a_real_day_is_refused() {
        // An invalid date would silently remove the task from every group.
        let source = "- [ ] pay\n";
        let tasks = parse_tasks(source);
        assert_eq!(
            set_task_due(source, tasks[0].offset, &tasks[0].text, Some("2026-02-30")),
            Err(TaskError::InvalidDate("2026-02-30".to_owned()))
        );
        assert_eq!(
            set_task_due(source, tasks[0].offset, &tasks[0].text, Some("tomorrow")),
            Err(TaskError::InvalidDate("tomorrow".to_owned()))
        );
    }

    #[test]
    fn clearing_a_due_date_that_does_not_exist_changes_nothing() {
        let source = "- [ ] pay\n";
        let tasks = parse_tasks(source);
        let cleared = set_task_due(source, tasks[0].offset, &tasks[0].text, None).expect("no-op");
        assert_eq!(cleared.source, source);
    }

    #[test]
    fn due_marker_edits_keep_the_rest_of_the_line_byte_identical() {
        let source = "- [x] wrapped [[link]] with [due:: 2026-10-20] and trailing text\n";
        let tasks = parse_tasks(source);
        let cleared = set_task_due(source, tasks[0].offset, &tasks[0].text, None).expect("clear");
        assert_eq!(
            cleared.source,
            "- [x] wrapped [[link]] with and trailing text\n"
        );
    }

    #[test]
    fn group_tasks_splits_today_upcoming_overdue_undated_and_done() {
        let source = "- [ ] today [due:: 2026-10-15]\n- [ ] future [due:: 2026-10-18]\n- [ ] past [due:: 2026-10-01]\n- [ ] none\n- [x] done [due:: 2026-10-15]\n";
        let tasks = parse_tasks(source);
        let groups = group_tasks(&tasks, "2026-10-15");
        let keys: Vec<&str> = groups.iter().map(|group| group.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["today", "upcoming", "overdue", "undated", "done"]
        );
        assert_eq!(groups[0].tasks, vec![0]);
        assert_eq!(groups[1].tasks, vec![1]);
        assert_eq!(groups[2].tasks, vec![2]);
        assert_eq!(groups[3].tasks, vec![3]);
        assert_eq!(groups[4].tasks, vec![4]);
    }

    #[test]
    fn calendar_dates_reject_impossible_days() {
        assert!(is_calendar_date("2026-10-15"));
        assert!(is_calendar_date("2024-02-29"));
        assert!(!is_calendar_date("2026-02-29"));
        assert!(!is_calendar_date("2026-02-30"));
        assert!(!is_calendar_date("2026-13-01"));
        assert!(!is_calendar_date("2026-1-1"));
        assert!(!is_calendar_date(""));
    }

    #[test]
    fn an_invalid_due_marker_is_not_a_due_date() {
        let tasks = parse_tasks("- [ ] x [due:: 2026-02-30]\n");
        assert_eq!(tasks[0].due, None);
    }

    #[test]
    fn crlf_lines_keep_their_endings_after_toggle() {
        let source = "---\r\n---\r\n- [ ] a\r\n";
        let tasks = parse_tasks(source);
        let toggled = set_task_status(source, tasks[0].offset, &tasks[0].text, TaskStatus::Done)
            .expect("toggle");
        assert_eq!(toggled.source, "---\r\n---\r\n- [x] a\r\n");
    }
}
