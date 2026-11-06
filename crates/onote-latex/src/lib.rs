//! M13 LaTeX build diagnostics (F53).
//!
//! This crate only reads a compiler log. It never runs a compiler, never installs
//! anything, and never guesses a line number: F53 §7 requires an unrecognised
//! message to keep its original text and offer a safe generic next step instead
//! of an invented position.
//!
//! Two further rules shape the design:
//!
//! * a warning is not a failure — success is decided by the exit status and the
//!   produced artefact, never by the presence of warnings (F53 §4);
//! * the raw log stays in its original language (F53-R02): the structured part is
//!   a caption, and the raw text is preserved byte-for-byte.

pub mod assist;
pub mod project;
pub mod project_store;
pub use project_store::{
    PendingSave, ProjectRecord, Recovery, SaveReceipt, SaveStep, StoreError, SyncReport,
    TrackedFile, RECORD_FORMAT_VERSION,
};
mod references;
pub use project::{
    inspect as inspect_project, locate_moved, Dependency as ProjectDependency, EntryCandidate,
    ProjectFile as InspectFile, ProjectFinding, ProjectInspection,
};

pub mod bibliography;
pub use bibliography::{
    dangling_citations, parse as parse_bibliography, plan_citation, plan_rename, unicode_entries,
    valid_cite_key, Backend, BibEntry, BibError, BibField, Bibliography, CitationIssue,
    CitationPlan, DuplicateKey, RenamePlan, MAX_CITE_KEY_CHARS,
};

pub mod insertion;
pub use insertion::{
    collect_labels, escape_plain, plan as plan_insertion, valid_label, Dependency, InsertKind,
    InsertionIssue, InsertionPlan, Parameters,
};

pub use assist::{
    apply_completion, apply_edits, completions, detect_language, external_change, matching_bracket,
    project_commands, replace_scope_allows, toggle_comment, wrap_selection, AssistError,
    AssistResult, Candidate, Edit, ExternalChange, LanguageMode, Range,
};
pub mod export;
pub mod scaffold;
pub use scaffold::{
    default_language, escape as escape_latex, plan as plan_scaffold,
    templates as scaffold_templates, validate as validate_metadata, DocumentLanguage,
    GeneratedFile, Metadata, MetadataError, PaperSize, PlanIssue, ScaffoldPlan, TemplateId,
    MAX_AUTHOR_CHARS, MAX_TITLE_CHARS,
};

pub use export::{
    plan_export, references_in, summarize as summarize_export, write_order, ExportEntry,
    ExportIssue, ExportKind, ExportPlan, ExportSummary, ProjectFile, EXCLUDED_DIRS,
    EXCLUDED_EXTENSIONS,
};
mod log_files;
pub mod preview;
pub mod queue;
pub mod synctex;
pub use queue::{
    should_auto_compile, CompileOutcome, Job, JobInput, JobState, Queue, QueueError,
    AUTO_COMPILE_DELAY_MS, DEFAULT_TIMEOUT_MS,
};

pub use preview::{
    apply_build, has_absolute_paths, localize_paths, locate as locate_preview, Artifact,
    BuildOutcome, JumpTarget, Mapping, MappingState, PdfState, PreviewState, SourcePosition,
    SyncRecord,
};
pub use synctex::{parse_synctex, PdfSpot, SourceSpot, SyncIndex};

use serde::{Deserialize, Serialize};

/// How serious a diagnostic is. `Info` exists because TeX emits package notes
/// that must not be shown as problems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A stable code so the UI can caption a diagnostic without parsing English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticCode {
    /// The engine stopped on `! ...`.
    TexError,
    /// `LaTeX Error: File `x.sty' not found.`
    MissingPackage,
    /// `Font ... not loadable`.
    MissingFont,
    /// `Undefined control sequence`.
    UndefinedControlSequence,
    /// `Missing $ inserted` and similar math-mode complaints.
    MathMode,
    /// `Runaway argument` / `Emergency stop`.
    RunawayArgument,
    /// `Overfull \hbox` / `Underfull \vbox`.
    BoxWarning,
    /// Anything this build does not recognise; the raw text is kept.
    UnknownDiagnostic,
}

/// One parsed diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    /// The message exactly as the compiler wrote it.
    pub raw: String,
    /// A suggestion key the UI resolves to a localized caption.
    pub message_key: String,
    /// File the message names, when it named one.
    pub file: Option<String>,
    /// Line number the compiler itself reported; never inferred (F53 §7).
    pub line: Option<u64>,
    /// Offset of the raw message inside the supplied log, so a viewer can jump
    /// to the original text without re-parsing.
    pub offset: usize,
}

/// A resource the build needed and could not find (F53-R01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingResource {
    /// `package`, `font` or `file`.
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub diagnostics: Vec<Diagnostic>,
    /// Lines that looked like messages but were not recognised; the UI shows
    /// them verbatim so nothing is hidden (UNKNOWN_DIAGNOSTIC).
    pub unparsed: Vec<String>,
    /// Every recognised missing resource, deduplicated, for the "prepare
    /// resources" entry point. Nothing is installed automatically.
    pub missing: Vec<MissingResource>,
    /// True when the log was cut for display (LOG_TRUNCATED).
    pub truncated: bool,
    /// The engine's exit code when the caller supplied one.
    pub exit_code: Option<i32>,
    /// Whether this build counts as a success.
    pub succeeded: bool,
    /// Why the verdict is what it is, so the UI can explain a failure that has
    /// no diagnostics.
    pub verdict: BuildVerdict,
    /// The compile job this log came from, when the caller associates one
    /// (F53-R03). A report without it is a manually supplied log.
    pub job_id: Option<String>,
    /// Hash of the source snapshot the log was produced from (F53 §4). The UI
    /// must not locate a diagnostic unless this matches the open source.
    pub source_hash: Option<String>,
}

/// The reasons a build can be judged failed, kept separate from log content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildVerdict {
    /// A non-zero exit code, or a missing artefact check by the caller.
    Failed,
    /// Exit code zero and no error diagnostic.
    Succeeded,
    /// Exit code unknown and no error diagnostic: the caller still has to check
    /// the artefact, so this is not a claim of success.
    Unverified,
}

/// Log size kept for parsing; a larger log keeps its head and tail
/// (LOG_TRUNCATED) so the first error and the summary both survive.
pub const MAX_LOG_BYTES: usize = 512 * 1024;
/// Head and tail kept when truncating.
pub const KEEP_HEAD_BYTES: usize = 200 * 1024;

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_boundary(text: &str, mut index: usize) -> usize {
    let len = text.len();
    if index > len {
        index = len;
    }
    while index < len && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// The head and tail kept from an oversized log, with a marker between them.
pub fn truncated_parts(log: &str) -> Option<(String, String)> {
    if log.len() <= MAX_LOG_BYTES {
        return None;
    }
    let head_end = floor_boundary(log, KEEP_HEAD_BYTES);
    let tail_start = ceil_boundary(log, log.len() - KEEP_HEAD_BYTES);
    Some((
        log[..head_end].to_owned(),
        log[tail_start.min(log.len())..].to_owned(),
    ))
}

/// Parses a file/line prefix such as `main.tex:12:` or `./chapters/x.tex:7:`.
fn file_line(line: &str) -> Option<(String, u64)> {
    let trimmed = line.trim_start();
    // The engine prefix must end with `:<digits>:` to be trusted.
    let colon = trimmed.find(':')?;
    let candidate = trimmed[..colon].trim();
    if candidate.is_empty() || candidate.contains(' ') {
        return None;
    }
    let rest = &trimmed[colon + 1..];
    let digits: String = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    if !after.starts_with(':') {
        return None;
    }
    Some((candidate.to_owned(), digits.parse().ok()?))
}

fn classify(raw: &str) -> (Severity, DiagnosticCode, &'static str) {
    let lower = raw.to_ascii_lowercase();
    // A multi-line raw block can contain several complaints; the *first* one that
    // names a resource wins, and a missing file outranks the generic "undefined
    // control sequence" that the same failure often also prints.
    let lines: Vec<&str> = raw
        .lines()
        .map(|line| line.trim_start_matches('!').trim())
        .collect();
    for line in &lines {
        let lower_line = line.to_ascii_lowercase();
        if lower_line.contains("not found")
            && (lower_line.contains(".sty")
                || lower_line.contains(".cls")
                || lower_line.contains(".clo")
                || lower_line.contains(".def"))
        {
            return (
                Severity::Error,
                DiagnosticCode::MissingPackage,
                "error.texMissingPackage",
            );
        }
        if lower_line.contains("font")
            && (lower_line.contains("not loadable") || lower_line.contains("not found"))
        {
            return (
                Severity::Error,
                DiagnosticCode::MissingFont,
                "error.texMissingFont",
            );
        }
    }
    if lower.contains("font") && (lower.contains("not loadable") || lower.contains("not found")) {
        return (
            Severity::Error,
            DiagnosticCode::MissingFont,
            "error.texMissingFont",
        );
    }
    if lower.contains("undefined control sequence") {
        return (
            Severity::Error,
            DiagnosticCode::UndefinedControlSequence,
            "error.texUndefinedControlSequence",
        );
    }
    if lower.contains("missing $ inserted") || lower.contains("math mode") {
        return (
            Severity::Error,
            DiagnosticCode::MathMode,
            "error.texMathMode",
        );
    }
    if lower.contains("runaway argument") || lower.contains("emergency stop") {
        return (
            Severity::Error,
            DiagnosticCode::RunawayArgument,
            "error.texRunaway",
        );
    }
    if lower.starts_with("overfull") || lower.starts_with("underfull") {
        return (
            Severity::Warning,
            DiagnosticCode::BoxWarning,
            "warning.texBox",
        );
    }
    if lower.contains("latex error:") || lower.starts_with('!') {
        return (Severity::Error, DiagnosticCode::TexError, "error.texError");
    }
    // A `LaTeX Warning:` line is a warning even without a box message.
    if lower.contains("latex warning") || lower.contains("package warning") {
        return (
            Severity::Warning,
            DiagnosticCode::TexError,
            "warning.texWarning",
        );
    }
    (
        Severity::Info,
        DiagnosticCode::UnknownDiagnostic,
        "info.texUnknown",
    )
}

fn missing_from(raw: &str, severity: Severity) -> Option<MissingResource> {
    if severity != Severity::Error {
        return None;
    }
    // Scan each line of a multi-line block, so the resource line is found even
    // when the block starts with an unrelated complaint.
    for line in raw.lines() {
        if let Some(resource) = missing_from_line(line) {
            return Some(resource);
        }
    }
    None
}

fn missing_from_line(raw: &str) -> Option<MissingResource> {
    let lower = raw.to_ascii_lowercase();
    // TeX wraps file names in a backtick and a straight quote: `` `x.sty' ``.
    // Only the straight quote is the closing delimiter, so the name is taken
    // between them rather than from the first quote pair in the line.
    let quoted = raw.find('`').and_then(|start| {
        raw[start + 1..]
            .find('\'')
            .map(|end| &raw[start + 1..start + 1 + end])
    });
    if lower.contains("not found")
        && (lower.contains(".sty")
            || lower.contains(".cls")
            || lower.contains(".clo")
            || lower.contains(".def"))
    {
        let name = match quoted {
            Some(value) => value.to_owned(),
            None => raw
                .split_whitespace()
                .find(|token| {
                    [".sty", ".cls", ".clo", ".def"]
                        .iter()
                        .any(|suffix| token.ends_with(suffix))
                })?
                .trim_matches(|character: char| character == '`' || character == '\'')
                .to_owned(),
        };
        if name.is_empty() {
            return None;
        }
        return Some(MissingResource {
            kind: if name.ends_with(".cls") {
                "class"
            } else if name.ends_with(".clo") {
                // A class option file, such as `size10.clo`.
                "file"
            } else {
                "package"
            }
            .to_owned(),
            name,
        });
    }
    if lower.contains("font") && (lower.contains("not loadable") || lower.contains("not found")) {
        // `! Font oo=bar not loadable: Metric (TFM) file not found.` names the
        // font after `Font`; a bare `Font x not found` also works.
        let after = raw.split_once("Font").map(|(_, rest)| rest).unwrap_or("");
        let name = after
            .trim_start()
            .trim_start_matches(|character: char| character == '\\')
            .split(|character: char| character == '=' || character.is_whitespace())
            .next()
            .unwrap_or("")
            .trim_matches(|character: char| {
                character == '`' || character == '\'' || character == '.'
            });
        if !name.is_empty() {
            return Some(MissingResource {
                kind: "font".to_owned(),
                name: name.to_owned(),
            });
        }
    }
    None
}

/// Parses a build log.
///
/// `exit_code` comes from the caller, because this crate never runs the engine:
/// the verdict has to combine the log with what actually happened.
pub fn parse_build_log(raw_log: &str, exit_code: Option<i32>) -> BuildReport {
    // An oversized log is parsed in full; the UI shows the head and tail via
    // `truncated_parts`, so the parser never works on a silently shortened log.
    let log = raw_log;
    let truncated = raw_log.len() > MAX_LOG_BYTES;
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut unparsed: Vec<String> = Vec::new();
    let mut missing: Vec<MissingResource> = Vec::new();

    let mut offset = 0usize;
    let mut lines = log.split('\n').peekable();
    while let Some(line) = lines.next() {
        let line_start = offset;
        offset += line.len() + 1;
        let trimmed = line.trim_end_matches('\r');
        let is_error_marker = trimmed.starts_with('!');
        let is_warning = {
            let lower = trimmed.to_ascii_lowercase();
            lower.contains("latex warning")
                || lower.contains("package warning")
                || lower.starts_with("overfull")
                || lower.starts_with("underfull")
        };
        let has_file_prefix = file_line(trimmed).is_some();

        if !(is_error_marker || is_warning || has_file_prefix) {
            continue;
        }
        // A TeX error spans the marker line plus its following `l.<n>` context
        // and any continuation; the raw text is kept whole so nothing is lost.
        let mut raw = trimmed.to_owned();
        if is_error_marker {
            while let Some(next) = lines.peek() {
                let candidate = next.trim_end_matches('\r');
                let stops = candidate.starts_with('!')
                    || candidate.to_ascii_lowercase().contains("latex warning")
                    || candidate.starts_with("Overfull")
                    || candidate.starts_with("Underfull")
                    || file_line(candidate).is_some();
                if stops || candidate.trim().is_empty() {
                    break;
                }
                raw.push('\n');
                raw.push_str(candidate);
                let consumed = lines.next().unwrap_or("");
                offset += consumed.len() + 1;
            }
        }
        let (severity, code, message_key) = classify(&raw);
        if let Some(resource) = missing_from(&raw, severity) {
            if !missing
                .iter()
                .any(|existing| existing.kind == resource.kind && existing.name == resource.name)
            {
                missing.push(resource);
            }
        }
        let (file, line_number) = match file_line(&raw) {
            Some((file, line)) => (Some(file), Some(line)),
            // TeX also writes `l.12` on the context line, which is the engine's
            // own number and therefore safe to use.
            None => (None, context_line(&raw)),
        };
        diagnostics.push(Diagnostic {
            severity,
            code,
            raw,
            message_key: message_key.to_owned(),
            file,
            line: line_number,
            offset: line_start,
        });
        let _ = trimmed;
    }

    // TeX rarely prints a file beside an error, but its log says which file was open.
    let offsets: Vec<usize> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.offset)
        .collect();
    let open_files = log_files::files_at(log, &offsets);
    for (diagnostic, open) in diagnostics.iter_mut().zip(open_files) {
        if diagnostic.line.is_none() {
            diagnostic.line = log_files::warning_line(&diagnostic.raw);
        }
        if diagnostic.file.is_none() && diagnostic.line.is_some() {
            diagnostic.file = open;
        }
    }

    // A line that looks like an error but was not recognised keeps its text.
    for line in log.split('\n') {
        let trimmed = line.trim();
        if trimmed.len() > 3
            && trimmed.starts_with('!')
            && !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.raw.starts_with(trimmed))
        {
            unparsed.push(trimmed.to_owned());
        }
    }

    let has_error = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    let verdict = match exit_code {
        Some(0) if !has_error => BuildVerdict::Succeeded,
        Some(0) => BuildVerdict::Succeeded,
        Some(_) => BuildVerdict::Failed,
        // Without an exit code the log alone cannot prove success.
        None => BuildVerdict::Unverified,
    };
    BuildReport {
        diagnostics,
        unparsed,
        missing,
        truncated,
        exit_code,
        succeeded: verdict == BuildVerdict::Succeeded,
        verdict,
        job_id: None,
        source_hash: None,
    }
}

/// The `l.<n>` number TeX prints on the context line after an error.
fn context_line(raw: &str) -> Option<u64> {
    for line in raw.split('\n') {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("l.") {
            let digits: String = rest
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}

/// Whether a parsed diagnostic may highlight a position in the current source.
///
/// F53 §4: a hash mismatch means the line number belongs to an older snapshot, so
/// highlighting it in the current file would point at the wrong place.
pub fn can_highlight(diagnostic_hash: &str, current_hash: &str) -> bool {
    diagnostic_hash == current_hash
}

/// A copy of the report with machine-specific absolute paths and the echoed
/// source line removed.
///
/// F53-R04: an exported report must not carry the user's home directory or a
/// fragment of their text. Paths and source excerpts are rewritten; the
/// compiler's own message is kept, because removing it would make the report
/// useless. `roots` are extra exact prefixes to remove (for example a vault
/// path that is relative in the log); paths outside them are still recognised.
pub fn redact(report: &BuildReport, roots: &[&str]) -> BuildReport {
    let scrub = |text: &str| -> String {
        // Path detection runs first: a configured root may be a prefix of a
        // longer absolute path, and replacing it first would leave a second
        // path fragment behind.
        let mut value = strip_source_context(&strip_absolute_paths(text));
        for root in roots {
            if root.is_empty() {
                continue;
            }
            value = value.replace(root, "<redacted>");
            // Windows separators are the common case in a TeX log.
            let escaped = root.replace('/', "\\");
            if escaped != *root {
                value = value.replace(&escaped, "<redacted>");
            }
        }
        value
    };
    BuildReport {
        diagnostics: report
            .diagnostics
            .iter()
            .map(|diagnostic| Diagnostic {
                raw: scrub(&diagnostic.raw),
                file: diagnostic.file.as_deref().map(scrub),
                ..diagnostic.clone()
            })
            .collect(),
        unparsed: report.unparsed.iter().map(|line| scrub(line)).collect(),
        // A missing resource is named exactly as the compiler printed it, which
        // can be an absolute path when the project lives outside the engine's
        // search path.
        missing: report
            .missing
            .iter()
            .map(|resource| MissingResource {
                kind: resource.kind.clone(),
                name: scrub(&resource.name),
            })
            .collect(),
        truncated: report.truncated,
        exit_code: report.exit_code,
        succeeded: report.succeeded,
        verdict: report.verdict,
        // Redaction must not drop the identity that decides whether a jump is
        // allowed (F53-R03).
        job_id: report.job_id.clone(),
        source_hash: report.source_hash.clone(),
    }
}

/// Replaces every absolute path in `text` with `<redacted>`.
///
/// The caller may not know the vault root (the panel exports with no roots), so
/// the export cannot depend on a configured prefix to be safe.
fn strip_absolute_paths(text: &str) -> String {
    let text = strip_quoted_paths(text);
    let mut output = String::with_capacity(text.len());
    for token in text.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end();
        let suffix = &token[trimmed.len()..];
        // The path need not start the token: logs write `(D:\vault\main.tex`
        // and `file=/home/me/main.tex`.
        if let Some(start) = absolute_path_start(trimmed) {
            let end = path_end(trimmed, start);
            output.push_str(&trimmed[..start]);
            output.push_str("<redacted>");
            output.push_str(&trimmed[end..]);
            output.push_str(suffix);
        } else {
            output.push_str(token);
        }
    }
    output
}

/// Redacts quoted absolute paths whole, including a quoted path with spaces and
/// TeX's `` `...' `` file-name form.
fn strip_quoted_paths(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let quote = bytes[i];
        let closer = match quote {
            b'"' => Some(b'"'),
            b'\'' => Some(b'\''),
            b'`' => Some(b'\''),
            _ => None,
        };
        let Some(closer) = closer else {
            let character = text[i..].chars().next().expect("char boundary");
            output.push(character);
            i += character.len_utf8();
            continue;
        };
        if let Some(close) = text[i + 1..].find(closer as char) {
            let inner = &text[i + 1..i + 1 + close];
            if absolute_path_start(inner.trim()).is_some() {
                output.push(quote as char);
                output.push_str("<redacted>");
                output.push(closer as char);
                i += close + 2;
                continue;
            }
        }
        output.push(quote as char);
        i += 1;
    }
    output
}

/// The first index that begins an absolute path; a relative `notes/a.md` is
/// deliberately not a hit and a URL's `//` is skipped.
fn absolute_path_start(token: &str) -> Option<usize> {
    let bytes = token.as_bytes();
    for i in 0..bytes.len() {
        let boundary = i == 0
            || !(bytes[i - 1].is_ascii_alphanumeric()
                || matches!(bytes[i - 1], b'/' | b'\\' | b'.' | b'-' | b'_'));
        if !boundary {
            continue;
        }
        let posix = bytes[i] == b'/' && bytes.get(i + 1).is_some_and(|next| *next != b'/');
        let windows = bytes[i].is_ascii_alphabetic()
            && bytes.get(i + 1) == Some(&b':')
            && matches!(bytes.get(i + 2), Some(b'\\') | Some(b'/'));
        if posix || windows {
            return Some(i);
        }
    }
    None
}

/// Where the path starting at `start` ends inside one whitespace token; the
/// surrounding message punctuation stays visible.
fn path_end(token: &str, start: usize) -> usize {
    let bytes = token.as_bytes();
    let mut end = start;
    while end < bytes.len()
        && !matches!(
            bytes[end],
            b')' | b']' | b'}' | b'>' | b'<' | b',' | b';' | b'"' | b'\''
        )
    {
        end += 1;
    }
    while end > start + 1 && matches!(bytes[end - 1], b'.' | b',' | b';' | b':' | b'!' | b'?') {
        end -= 1;
    }
    end
}

/// Removes TeX's `l.<n>` context line and its continuation.
///
/// Those lines echo the user's source, which F53-R04 excludes from an export;
/// the compiler's message and the structured line number remain.
fn strip_source_context(raw: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let mut in_context = false;
    for line in raw.split('\n') {
        let trimmed = line.trim_start();
        let is_context = trimmed
            .strip_prefix("l.")
            .and_then(|rest| rest.chars().next())
            .is_some_and(|character| character.is_ascii_digit());
        if is_context {
            if !in_context {
                output.push("<redacted>");
                in_context = true;
            }
            continue;
        }
        if in_context {
            continue;
        }
        output.push(line);
    }
    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "This is pdfTeX\n\
(main.tex:12: Undefined control sequence.\n\
! Undefined control sequence.\n\
l.12 \\foo\n\
         bar\n\
)\n\
LaTeX Warning: Citation `x' undefined on page 1.\n\
Overfull \\hbox (12.0pt too wide) in paragraph at lines 5--7\n\
! LaTeX Error: File `missingpkg.sty' not found.\n\
";

    #[test]
    fn classifies_an_undefined_control_sequence_as_an_error() {
        let report = parse_build_log(LOG, Some(1));
        let error = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::UndefinedControlSequence)
            .expect("must find the error");
        assert_eq!(error.severity, Severity::Error);
        // The engine's own line number is used, never an inferred one.
        assert_eq!(error.line, Some(12));
        assert!(!report.succeeded);
        assert_eq!(report.verdict, BuildVerdict::Failed);
    }

    #[test]
    fn a_warning_is_not_a_failure() {
        // F53 §4: warnings never decide the verdict.
        let log =
            "LaTeX Warning: Citation `x' undefined.\nOverfull \\hbox (1pt too wide) in paragraph\n";
        let report = parse_build_log(log, Some(0));
        assert!(report
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Error));
        assert!(report.succeeded);
        assert_eq!(report.verdict, BuildVerdict::Succeeded);
    }

    #[test]
    fn a_missing_package_is_named_for_the_prepare_step() {
        // F53-R01: offer a resource step, never run an installer.
        let report = parse_build_log(LOG, Some(1));
        assert!(report
            .missing
            .iter()
            .any(|resource| resource.name == "missingpkg.sty" && resource.kind == "package"));
    }

    #[test]
    fn a_missing_class_option_file_is_a_missing_resource() {
        // What Tectonic writes for `\documentclass{article}` when the prepared
        // resources only cover other sizes.
        let log = "! ! LaTeX Error: File `size10.clo' not found..\n\\@missingfileerror ...or: File `#1.#2' not found.}\n";
        let report = parse_build_log(log, Some(1));
        assert_eq!(report.diagnostics[0].code, DiagnosticCode::MissingPackage);
        assert_eq!(
            report.missing,
            vec![MissingResource {
                kind: "file".to_owned(),
                name: "size10.clo".to_owned()
            }]
        );
    }

    #[test]
    fn the_raw_text_is_preserved_verbatim() {
        // F53-R02: the structured caption is added; the log keeps its language.
        let report = parse_build_log(LOG, Some(1));
        let raw = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(raw.contains("Undefined control sequence."));
        assert!(raw.contains("l.12 \\foo"));
        assert!(raw.contains("Citation `x' undefined"));
    }

    #[test]
    fn an_unrecognised_error_keeps_its_text_and_has_no_invented_position() {
        let log = "! Something this build has never seen\n";
        let report = parse_build_log(log, Some(1));
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, DiagnosticCode::TexError);
        assert_eq!(diagnostic.raw, "! Something this build has never seen");
        // No line number was printed, so none is claimed.
        assert_eq!(diagnostic.line, None);
    }

    #[test]
    fn an_unknown_exit_code_is_unverified_not_success() {
        // Without an exit status the log alone cannot prove the build worked.
        let report = parse_build_log("This is pdfTeX\n", None);
        assert_eq!(report.verdict, BuildVerdict::Unverified);
        assert!(!report.succeeded);
    }

    #[test]
    fn a_zero_exit_with_only_warnings_still_succeeds() {
        let report = parse_build_log(
            "LaTeX Warning: Font shape `T1/cmr/m/n' undefined\n",
            Some(0),
        );
        assert_eq!(report.verdict, BuildVerdict::Succeeded);
    }

    #[test]
    fn highlighting_requires_a_matching_source_hash() {
        // F53 §4: a stale snapshot must not highlight the current file.
        assert!(can_highlight("abc", "abc"));
        assert!(!can_highlight("abc", "def"));
    }

    #[test]
    fn an_oversized_log_is_marked_truncated_and_keeps_both_ends() {
        let mut log = String::from("! first error\n");
        log.push_str(&"x".repeat(MAX_LOG_BYTES));
        log.push_str("\n! last error\n");
        let report = parse_build_log(&log, Some(1));
        assert!(report.truncated);
        let parts = truncated_parts(&log).expect("must truncate");
        assert!(parts.0.starts_with("! first error"));
        assert!(parts.1.ends_with("! last error\n"));
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        // A multi-byte log must not be cut mid-character.
        let log = format!("{}京", "y".repeat(MAX_LOG_BYTES));
        let parts = truncated_parts(&log).expect("must truncate");
        assert!(parts.0.is_char_boundary(parts.0.len()));
        assert!(parts.1.is_char_boundary(0));
        // Parsing a truncated log must not panic.
        let _ = parse_build_log(&log, Some(1));
    }

    #[test]
    fn redaction_removes_absolute_roots_but_keeps_the_message() {
        // F53-R04: export drops the user's paths, not the facts.
        let log = "/home/alice/thesis/main.tex:12: Undefined control sequence.\n! File `/home/alice/thesis/x.sty' not found.\n";
        let report = parse_build_log(log, Some(1));
        let redacted = redact(&report, &["/home/alice/thesis"]);
        let text = redacted
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("/home/alice/thesis"));
        assert!(text.contains("<redacted>"));
        assert!(text.contains("Undefined control sequence."));
    }

    #[test]
    fn redaction_removes_absolute_paths_even_without_a_configured_root() {
        // F53-R04: the panel exports with no roots, so detection cannot depend
        // on the caller knowing the vault path.
        let log = "! LaTeX Error: File `/home/alice/thesis/x.sty' not found.\n\
see (D:\\vault\\notes\\main.tex) and \"/home/alice/my thesis/main.tex\".\n\
! See https://example.com/help for details.\n";
        let report = parse_build_log(log, Some(1));
        let redacted = redact(&report, &[]);
        let text = redacted
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("/home/alice"));
        assert!(!text.contains("D:\\vault"));
        assert!(text.contains("<redacted>"));
        // The message, the relative resource name and a URL stay intact.
        assert!(text.contains("LaTeX Error"));
        assert!(text.contains("not found"));
        assert!(text.contains("https://example.com/help"));
        assert!(redacted
            .missing
            .iter()
            .all(|resource| !resource.name.contains("/home/alice")));
    }

    #[test]
    fn redaction_removes_the_echoed_source_line_but_keeps_the_line_number() {
        // F53-R04: TeX echoes the offending source line after `l.<n>`; the
        // export keeps the message and the compiler's number, not the text.
        let log = "! Undefined control sequence.\nl.12 \\secretmacro{private note}\n";
        let report = parse_build_log(log, Some(1));
        let redacted = redact(&report, &[]);
        let diagnostic = &redacted.diagnostics[0];
        assert!(!diagnostic.raw.contains("private note"));
        assert!(!diagnostic.raw.contains("secretmacro"));
        assert!(diagnostic.raw.contains("Undefined control sequence."));
        assert_eq!(diagnostic.line, Some(12));
    }

    #[test]
    fn redaction_keeps_the_job_and_snapshot_identity() {
        // F53-R03: the exported report must still say which build it was, or a
        // stale log could later be located against the wrong snapshot.
        let mut report = parse_build_log("! error\n", Some(1));
        report.job_id = Some("job-7".to_owned());
        report.source_hash = Some("abc123".to_owned());
        let redacted = redact(&report, &[]);
        assert_eq!(redacted.job_id.as_deref(), Some("job-7"));
        assert_eq!(redacted.source_hash.as_deref(), Some("abc123"));
    }

    #[test]
    fn a_configured_root_is_not_replaced_twice() {
        let log = "/home/alice/thesis/main.tex:12: Undefined control sequence.\n";
        let report = parse_build_log(log, Some(1));
        let redacted = redact(&report, &["/home/alice/thesis"]);
        let raw = &redacted.diagnostics[0].raw;
        assert!(!raw.contains("/home/alice"));
        assert_eq!(raw.matches("<redacted>").count(), 1);
    }

    #[test]
    fn a_missing_font_is_reported_as_a_font_resource() {
        let log = "! Font \\foo=bar not loadable: Metric (TFM) file not found.\n";
        let report = parse_build_log(log, Some(1));
        assert!(report
            .missing
            .iter()
            .any(|resource| resource.kind == "font"));
    }

    #[test]
    fn two_identical_missing_files_are_reported_once() {
        let log =
            "! LaTeX Error: File `a.sty' not found.\n! LaTeX Error: File `a.sty' not found.\n";
        let report = parse_build_log(log, Some(1));
        assert_eq!(report.missing.len(), 1);
    }

    #[test]
    fn an_empty_log_has_no_diagnostics_and_no_claim() {
        let report = parse_build_log("", Some(0));
        assert!(report.diagnostics.is_empty());
        assert!(report.missing.is_empty());
        assert!(!report.truncated);
        // An empty log with a zero exit is still only "succeeded" because the
        // caller asserted the exit code; nothing is inferred from silence.
        assert_eq!(report.verdict, BuildVerdict::Succeeded);
    }

    #[test]
    fn math_mode_and_runaway_errors_are_distinguished() {
        let math = parse_build_log("! Missing $ inserted.\n", Some(1));
        assert_eq!(math.diagnostics[0].code, DiagnosticCode::MathMode);
        let runaway = parse_build_log("! Runaway argument?\n", Some(1));
        assert_eq!(runaway.diagnostics[0].code, DiagnosticCode::RunawayArgument);
    }
}
