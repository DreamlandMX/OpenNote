//! M11 storage/service diagnostics and redacted reports (F44).
//!
//! A diagnostic report exists to help someone find a fault, and it must do that
//! without carrying the user's text or their machine layout. Three rules shape
//! this crate:
//!
//! * only allowlisted fields are reported (F44 §4) — a report never includes a
//!   path, a note body or a credential, by construction rather than by filtering;
//! * a statistic that could not be read is `Unknown`, never `0` (F44-R04), so a
//!   broken counter cannot look like an idle system;
//! * a raw build log is redacted **and previewed** before export (F44-R03), and
//!   nothing is ever sent anywhere automatically (F44-R02).
//!
//! This crate has no network, filesystem or process access: it produces a value
//! that a caller may show or export.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A statistic that may be unavailable.
///
/// The whole point of this type is that "we could not read it" is not the same
/// value as "it is zero" (F44-R04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "value")]
pub enum Measured<T> {
    Known(T),
    Unknown,
}

impl<T> Measured<T> {
    pub fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }

    /// The value, or `None` — never a fabricated default.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown => None,
        }
    }
}

/// The subsystem a status belongs to.
///
/// F44-R01: local saving, network, entitlement, backup and compilation are
/// reported separately, so "everything is fine" is never claimed on the strength
/// of one subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Storage,
    Network,
    Entitlement,
    Backup,
    Compile,
    Application,
}

/// A subsystem's state, kept distinct from "healthy".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum Status {
    /// The check ran and the subsystem reported ready.
    Ready,
    /// The check ran and the subsystem reported a problem.
    Degraded { reason_key: String },
    /// The check could not run: unreachable, not configured or not applicable.
    ///
    /// This is deliberately not `Degraded`: an unreachable service is not proof
    /// of a fault, and reporting it as one would be as wrong as reporting Ready
    /// (STATUS_UNKNOWN).
    Unknown { reason_key: String },
}

impl Status {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Allowlisted fields a report may contain (F44 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReportField {
    AppVersion,
    BuildChannel,
    Platform,
    Locale,
    ErrorCode,
    TimestampMillis,
    Count(CountKind),
    Capability(CapabilityKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CountKind {
    Notes,
    Attachments,
    Drafts,
    TrashEntries,
    HistoryRevisions,
    SearchDocuments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityKind {
    Search,
    Sync,
    CloudBackup,
    LatexCompile,
    Telemetry,
}

/// A value in the report, already restricted to allowlisted shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum FieldValue {
    Text(String),
    Integer(u64),
    Boolean(bool),
    /// The field's value could not be read.
    Unknown,
}

/// One reported field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedField {
    pub key: String,
    pub value: FieldValue,
}

/// Something deliberately left out, so the reader knows the report is partial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Omission {
    pub key: String,
    pub reason_key: String,
}

/// What the caller may show before exporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub correlation_id: String,
    pub fields: Vec<ReportedField>,
    pub statuses: Vec<ScopeStatus>,
    pub redactions: Vec<Redaction>,
    pub omitted: Vec<Omission>,
    /// True when the raw log was included in redacted form.
    pub includes_log: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeStatus {
    pub scope: Scope,
    pub status: Status,
}

/// A redaction that was applied, with a count rather than the removed text.
///
/// The removed text is never kept: a report that quoted what it hid would leak
/// exactly what it claims to protect (F44-R03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Redaction {
    /// `path`, `email`, `url-credential` or `token`.
    pub kind: String,
    /// How many occurrences were replaced.
    pub count: u64,
}

/// The machine facts a caller supplies. Nothing here is derived from note text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFacts {
    pub app_version: String,
    pub build_channel: String,
    pub platform: String,
    pub locale: String,
    /// Absolute roots to strip from any text, e.g. the vault and home directory.
    /// These are used for matching only and never appear in the report.
    pub roots: Vec<String>,
}

/// Everything the preview needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub facts: HostFacts,
    pub correlation_id: String,
    pub counts: Vec<(CountKind, Measured<u64>)>,
    pub capabilities: Vec<(CapabilityKind, Status)>,
    pub statuses: Vec<ScopeStatus>,
    /// The most recent error code, if any.
    pub last_error_code: Option<String>,
    pub timestamp_millis: u64,
    /// Raw build log to redact, when the user asked to include it.
    pub raw_log: Option<String>,
}

/// A random-looking but deterministic correlation id.
///
/// F44 §4: it identifies an operation only. It contains no file name, path or
/// account secret, and it is stable for the same input so two reports from one
/// incident can be matched.
pub fn correlation_id(seed: &str) -> String {
    // FNV-1a, formatted as a uuid-like value. Not cryptography: an identifier
    // must be stable and opaque, not secret.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let hex = format!("{hash:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[0..4],
        &hex[4..16]
    )
}

/// Strips the patterns that must never leave the machine.
///
/// The count of replacements is returned so the preview can say *that* something
/// was removed without showing what it was (F44-R03).
pub fn redact(text: &str, roots: &[String]) -> (String, Vec<Redaction>) {
    let mut value = text.to_owned();
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();

    // Compiler errors often echo the source body after TeX's `l.<line>` prefix.
    // Keep the useful line number and later messages, but remove the body.
    let (without_source, source_count) = strip_source_context(&value);
    value = without_source;
    if source_count > 0 {
        *counts.entry("content").or_insert(0) += source_count;
    }

    // Roots first: a home directory is the longest match and would otherwise be
    // partially caught by the generic path rule.
    for root in roots {
        if root.is_empty() {
            continue;
        }
        for candidate in [root.clone(), root.replace('/', "\\")] {
            if candidate.is_empty() || !value.contains(&candidate) {
                continue;
            }
            let occurrences = value.matches(&candidate).count() as u64;
            value = value.replace(&candidate, "<redacted>");
            *counts.entry("path").or_insert(0) += occurrences;
        }
    }

    // Absolute POSIX paths and Windows drive paths that survived the root pass.
    let (without_paths, path_count) = strip_paths(&value);
    value = without_paths;
    if path_count > 0 {
        *counts.entry("path").or_insert(0) += path_count;
    }

    // Credentials embedded in a URL.
    let (without_url, url_count) = strip_url_credentials(&value);
    value = without_url;
    if url_count > 0 {
        *counts.entry("url-credential").or_insert(0) += url_count;
    }

    // Email addresses.
    let (without_email, email_count) = strip_emails(&value);
    value = without_email;
    if email_count > 0 {
        *counts.entry("email").or_insert(0) += email_count;
    }

    // Tokens and secret-key assignments. A build log can echo an environment
    // line (`API_KEY=...`) or an authorization header; those are credentials
    // just as much as a password embedded in a URL.
    let (without_secrets, secret_count) = strip_secrets(&value);
    value = without_secrets;
    if secret_count > 0 {
        *counts.entry("token").or_insert(0) += secret_count;
    }

    let redactions = counts
        .into_iter()
        .map(|(kind, count)| Redaction {
            kind: kind.to_owned(),
            count,
        })
        .collect();
    (value, redactions)
}

fn strip_source_context(text: &str) -> (String, u64) {
    let mut output = String::with_capacity(text.len());
    let mut continuation = false;
    let mut count = 0u64;

    for segment in text.split_inclusive('\n') {
        let ending = if segment.ends_with("\r\n") {
            "\r\n"
        } else if segment.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let line = &segment[..segment.len() - ending.len()];
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("l.") {
            let digits: String = rest
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                output.push_str("l.");
                output.push_str(&digits);
                output.push_str(" <redacted>");
                output.push_str(ending);
                continuation = true;
                count += 1;
                continue;
            }
        }
        if continuation {
            continuation = false;
            let is_source_continuation =
                line.chars().next().is_some_and(char::is_whitespace) && !line.trim().is_empty();
            if is_source_continuation {
                continue;
            }
        }
        output.push_str(segment);
    }
    (output, count)
}

fn strip_paths(text: &str) -> (String, u64) {
    // A quoted path may contain spaces, which whitespace tokenising can never
    // see as one value. Handle those first.
    let (text, quoted_count) = strip_quoted_paths(text);
    let mut output = String::with_capacity(text.len());
    let mut count = quoted_count;
    for token in text.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end();
        let suffix = &token[trimmed.len()..];
        // The path need not start the token: build logs write
        // `(C:\vault\main.tex`, `file=/home/me/main.tex` and `see /tmp/x`.
        if let Some(start) = absolute_path_start(trimmed) {
            let end = path_end(trimmed, start);
            output.push_str(&trimmed[..start]);
            output.push_str("<redacted>");
            output.push_str(&trimmed[end..]);
            output.push_str(suffix);
            count += 1;
        } else {
            output.push_str(token);
        }
    }
    (output, count)
}

/// Redacts quoted absolute paths, so `"/home/me/my notes/a.tex"` is removed
/// whole rather than being split at the space.
fn strip_quoted_paths(text: &str) -> (String, u64) {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut count = 0u64;
    let mut i = 0;
    while i < bytes.len() {
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            let ch = text[i..]
                .chars()
                .next()
                .expect("index is on a char boundary");
            output.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if let Some(close) = text[i + 1..].find(quote as char) {
            let inner = &text[i + 1..i + 1 + close];
            if absolute_path_start(inner.trim()).is_some() {
                output.push(quote as char);
                output.push_str("<redacted>");
                output.push(quote as char);
                count += 1;
                i += close + 2;
                continue;
            }
        }
        output.push(quote as char);
        i += 1;
    }
    (output, count)
}

/// The first index in `token` that begins an absolute path.
///
/// A relative `notes/a.md` is deliberately not a hit: it carries no machine
/// information. A URL's `//` and path segments are skipped, because a URL is
/// not a local path.
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

/// Where the path starting at `start` ends inside one whitespace token.
///
/// Closing delimiters and sentence punctuation belong to the surrounding log
/// message, not to the path, so they stay visible.
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

/// Keys whose value is a credential when written as `key=value` or `key: value`.
const SECRET_KEYS: &[&str] = &[
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "passphrase",
    "authorization",
    "auth",
    "credential",
    "credentials",
];

fn strip_secrets(text: &str) -> (String, u64) {
    let tokens: Vec<&str> = text.split_inclusive(char::is_whitespace).collect();
    let mut output = String::with_capacity(text.len());
    let mut count = 0u64;
    let mut redact_next = false;
    let mut auth_header = false;
    for (index, token) in tokens.iter().enumerate() {
        let trimmed = token.trim_end();
        let suffix = &token[trimmed.len()..];
        if redact_next {
            // `Authorization: Bearer <token>`: the scheme word is not the
            // secret, so keep it and redact the token that follows.
            if auth_header && is_auth_scheme(trimmed) {
                output.push_str(token);
            } else {
                output.push_str("<redacted>");
                output.push_str(suffix);
                count += 1;
                redact_next = false;
                auth_header = false;
            }
            continue;
        }
        if is_auth_scheme(trimmed)
            && tokens
                .get(index + 1)
                .is_some_and(|next| looks_like_secret_value(next.trim_end()))
        {
            // A bare `Bearer eyJ...` line, with no header name in front.
            output.push_str(token);
            redact_next = true;
            auth_header = true;
            continue;
        }
        if let Some((redacted, value)) = redact_secret_assignment(trimmed) {
            output.push_str(&redacted);
            output.push_str(suffix);
            match value {
                SecretValue::Redacted => count += 1,
                // A bare `Authorization:` header name: its value is the next
                // token.
                SecretValue::Awaiting {
                    auth_header: is_auth,
                } => {
                    redact_next = true;
                    auth_header = is_auth;
                }
            }
            continue;
        }
        output.push_str(token);
    }
    (output, count)
}

fn is_auth_scheme(token: &str) -> bool {
    let token = token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    token.eq_ignore_ascii_case("bearer") || token.eq_ignore_ascii_case("basic")
}

/// Whether a token after a scheme word looks like a credential rather than an
/// ordinary word. A prose sentence such as "basic build failed" must not be
/// treated as an authorization header.
fn looks_like_secret_value(token: &str) -> bool {
    let token = token.trim_matches(|ch: char| {
        matches!(
            ch,
            '"' | '\'' | '(' | ')' | ',' | ';' | '<' | '>' | '[' | ']' | '{' | '}'
        )
    });
    token.len() >= 16
        && token.chars().any(|ch| ch.is_ascii_digit())
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_secret_key(key: &str) -> bool {
    let key = key.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-');
    SECRET_KEYS
        .iter()
        .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

enum SecretValue {
    /// The token's own value was replaced.
    Redacted,
    /// The token only named the key; the next token is the value.
    Awaiting { auth_header: bool },
}

/// Redacts the value of a secret-key assignment inside one token.
fn redact_secret_assignment(token: &str) -> Option<(String, SecretValue)> {
    // A URL's `scheme://` is not an assignment; `https:` must stay intact.
    let separator = token
        .char_indices()
        .find(|(index, ch)| *ch == '=' || (*ch == ':' && !token[*index..].starts_with("://")))?
        .0;
    // In `https://host/?access_token=abc` the key is the query segment, not
    // the whole URL.
    let key_start = token[..separator]
        .rfind(|ch| ch == '?' || ch == '&')
        .map_or(0, |index| index + 1);
    let key = &token[key_start..separator];
    if !is_secret_key(key) {
        return None;
    }
    let auth_header = key
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
        .eq_ignore_ascii_case("authorization")
        || key
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
            .eq_ignore_ascii_case("auth");
    let value = &token[separator + 1..];
    if value.is_empty() {
        return Some((token.to_owned(), SecretValue::Awaiting { auth_header }));
    }
    // In a query string only the value up to `&` is the secret.
    let value_end = value.find('&').unwrap_or(value.len());
    let mut redacted = String::with_capacity(token.len());
    redacted.push_str(&token[..separator + 1]);
    redacted.push_str("<redacted>");
    redacted.push_str(&value[value_end..]);
    Some((redacted, SecretValue::Redacted))
}

fn strip_url_credentials(text: &str) -> (String, u64) {
    let mut output = String::with_capacity(text.len());
    let mut count = 0u64;
    for token in text.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end();
        let suffix = &token[trimmed.len()..];
        // `scheme://user:password@host` -> `scheme://<redacted>@host`
        if let Some(scheme_end) = trimmed.find("://") {
            let after = &trimmed[scheme_end + 3..];
            if let Some(at) = after.find('@') {
                if after[..at].contains(':') {
                    output.push_str(&trimmed[..scheme_end + 3]);
                    output.push_str("<redacted>");
                    output.push_str(&after[at..]);
                    output.push_str(suffix);
                    count += 1;
                    continue;
                }
            }
        }
        output.push_str(token);
    }
    (output, count)
}

fn strip_emails(text: &str) -> (String, u64) {
    let mut output = String::with_capacity(text.len());
    let mut count = 0u64;
    for token in text.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end();
        let suffix = &token[trimmed.len()..];
        let is_email = trimmed.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.contains('/')
        });
        if is_email {
            output.push_str("<redacted>");
            output.push_str(suffix);
            count += 1;
        } else {
            output.push_str(token);
        }
    }
    (output, count)
}

/// Builds the preview a user reviews before any export.
///
/// The field list is fixed: only allowlisted keys are produced, so a caller
/// cannot accidentally add a path or a note body to the report (F44 §4).
pub fn preview(request: &PreviewRequest) -> Preview {
    let mut fields: Vec<ReportedField> = Vec::new();
    let mut omitted: Vec<Omission> = Vec::new();

    fields.push(ReportedField {
        key: "appVersion".to_owned(),
        value: FieldValue::Text(request.facts.app_version.clone()),
    });
    fields.push(ReportedField {
        key: "buildChannel".to_owned(),
        value: FieldValue::Text(request.facts.build_channel.clone()),
    });
    fields.push(ReportedField {
        key: "platform".to_owned(),
        value: FieldValue::Text(request.facts.platform.clone()),
    });
    fields.push(ReportedField {
        key: "locale".to_owned(),
        value: FieldValue::Text(request.facts.locale.clone()),
    });
    fields.push(ReportedField {
        key: "timestampMillis".to_owned(),
        value: FieldValue::Integer(request.timestamp_millis),
    });
    if let Some(code) = request.last_error_code.as_deref() {
        fields.push(ReportedField {
            key: "lastErrorCode".to_owned(),
            value: FieldValue::Text(code.to_owned()),
        });
    }

    for (kind, measured) in &request.counts {
        let key = match kind {
            CountKind::Notes => "count.notes",
            CountKind::Attachments => "count.attachments",
            CountKind::Drafts => "count.drafts",
            CountKind::TrashEntries => "count.trashEntries",
            CountKind::HistoryRevisions => "count.historyRevisions",
            CountKind::SearchDocuments => "count.searchDocuments",
        };
        fields.push(ReportedField {
            key: key.to_owned(),
            // An unreadable counter is Unknown, not zero (F44-R04).
            value: match measured {
                Measured::Known(value) => FieldValue::Integer(*value),
                Measured::Unknown => FieldValue::Unknown,
            },
        });
    }

    for (kind, status) in &request.capabilities {
        let key = match kind {
            CapabilityKind::Search => "capability.search",
            CapabilityKind::Sync => "capability.sync",
            CapabilityKind::CloudBackup => "capability.cloudBackup",
            CapabilityKind::LatexCompile => "capability.latexCompile",
            CapabilityKind::Telemetry => "capability.telemetry",
        };
        fields.push(ReportedField {
            key: key.to_owned(),
            value: FieldValue::Text(status_name(status).to_owned()),
        });
    }

    // Anything the caller wanted to include but must not be reported.
    omitted.push(Omission {
        key: "notePaths".to_owned(),
        reason_key: "diagnostics.omitted.paths".to_owned(),
    });
    omitted.push(Omission {
        key: "noteContent".to_owned(),
        reason_key: "diagnostics.omitted.content".to_owned(),
    });
    omitted.push(Omission {
        key: "credentials".to_owned(),
        reason_key: "diagnostics.omitted.credentials".to_owned(),
    });

    let mut redactions: Vec<Redaction> = Vec::new();
    if let Some(log) = request.raw_log.as_deref() {
        let (_, log_redactions) = redact(log, &request.facts.roots);
        redactions.extend(log_redactions);
    }
    // Merge duplicate kinds so a preview reads as one line per pattern.
    let mut merged: BTreeMap<String, u64> = BTreeMap::new();
    for redaction in redactions {
        *merged.entry(redaction.kind).or_insert(0) += redaction.count;
    }
    let redactions: Vec<Redaction> = merged
        .into_iter()
        .map(|(kind, count)| Redaction { kind, count })
        .collect();

    Preview {
        correlation_id: request.correlation_id.clone(),
        fields,
        statuses: request.statuses.clone(),
        redactions,
        omitted,
        includes_log: request.raw_log.is_some(),
    }
}

fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Ready => "ready",
        Status::Degraded { .. } => "degraded",
        Status::Unknown { .. } => "unknown",
    }
}

/// The exported report, as text the user reviews first.
///
/// The log is redacted here too: a caller that previewed a redacted log and then
/// exported the raw one would defeat the preview (F44-R03).
pub fn export_text(preview: &Preview, request: &PreviewRequest) -> String {
    let mut document = serde_json::json!({
        "correlationId": preview.correlation_id,
        "generatedAtMillis": request.timestamp_millis,
        "fields": preview
            .fields
            .iter()
            .map(|field| serde_json::json!({
                "key": field.key,
                "value": match &field.value {
                    FieldValue::Text(text) => serde_json::json!({ "type": "text", "value": text }),
                    FieldValue::Integer(number) => serde_json::json!({ "type": "integer", "value": number }),
                    FieldValue::Boolean(flag) => serde_json::json!({ "type": "boolean", "value": flag }),
                    FieldValue::Unknown => serde_json::json!({ "type": "unknown" }),
                },
            }))
            .collect::<Vec<_>>(),
        "statuses": preview
            .statuses
            .iter()
            .map(|entry| serde_json::json!({
                "scope": format!("{:?}", entry.scope).to_lowercase(),
                "status": match &entry.status {
                    Status::Ready => serde_json::json!({ "state": "ready" }),
                    Status::Degraded { reason_key } => serde_json::json!({ "state": "degraded", "reasonKey": reason_key }),
                    Status::Unknown { reason_key } => serde_json::json!({ "state": "unknown", "reasonKey": reason_key }),
                },
            }))
            .collect::<Vec<_>>(),
        "redactions": preview
            .redactions
            .iter()
            .map(|redaction| serde_json::json!({ "kind": redaction.kind, "count": redaction.count }))
            .collect::<Vec<_>>(),
        "omitted": preview
            .omitted
            .iter()
            .map(|omission| serde_json::json!({ "key": omission.key, "reasonKey": omission.reason_key }))
            .collect::<Vec<_>>(),
    });
    if let Some(log) = request.raw_log.as_deref() {
        let (redacted, _) = redact(log, &request.facts.roots);
        if let Some(object) = document.as_object_mut() {
            object.insert("log".to_owned(), serde_json::Value::String(redacted));
        }
    }
    serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".to_owned())
}

/// Whether a report may be called healthy.
///
/// F44-R01: one ready subsystem never implies overall health, and an unknown
/// subsystem means "we do not know", not "healthy" (STATUS_UNKNOWN).
pub fn overall(preview: &Preview) -> Status {
    let mut degraded: Option<&str> = None;
    // Capabilities count as well as scopes: a subsystem whose capability could not
    // be determined leaves the whole report unknown, even when every probed scope
    // is ready. Ignoring them reported "healthy" for a build whose compiler state
    // was unknown.
    let capability_statuses: Vec<Status> = preview
        .fields
        .iter()
        .filter(|field| field.key.starts_with("capability."))
        .map(|field| match &field.value {
            FieldValue::Text(text) if text == "ready" => Status::Ready,
            FieldValue::Text(text) if text == "degraded" => Status::Degraded {
                reason_key: "diagnostics.capabilityDegraded".to_owned(),
            },
            _ => Status::Unknown {
                reason_key: "diagnostics.capabilityUnknown".to_owned(),
            },
        })
        .collect();
    let all: Vec<&Status> = preview
        .statuses
        .iter()
        .map(|entry| &entry.status)
        .chain(capability_statuses.iter())
        .collect();
    for status in all {
        match status {
            Status::Degraded { reason_key } => {
                if degraded.is_none() {
                    degraded = Some(reason_key.as_str());
                }
            }
            Status::Unknown { reason_key } => {
                if degraded.is_none() {
                    degraded = Some(reason_key.as_str());
                }
            }
            Status::Ready => {}
        }
    }

    if let Some(reason) = degraded {
        // A degraded subsystem is reported as degraded; an unavailable one as
        // unknown only when nothing is actually wrong.
        let any_degraded = preview
            .statuses
            .iter()
            .any(|entry| matches!(entry.status, Status::Degraded { .. }))
            || preview.fields.iter().any(|field| {
                field.key.starts_with("capability.")
                    && matches!(&field.value, FieldValue::Text(text) if text == "degraded")
            });
        return if any_degraded {
            Status::Degraded {
                reason_key: reason.to_owned(),
            }
        } else {
            Status::Unknown {
                reason_key: reason.to_owned(),
            }
        };
    }
    Status::Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> HostFacts {
        HostFacts {
            app_version: "0.1.0".to_owned(),
            build_channel: "dev".to_owned(),
            platform: "windows-x64".to_owned(),
            locale: "zh-CN".to_owned(),
            roots: vec!["/home/alice/vault".to_owned(), "/home/alice".to_owned()],
        }
    }

    fn request() -> PreviewRequest {
        PreviewRequest {
            facts: facts(),
            correlation_id: "cid-1".to_owned(),
            counts: vec![
                (CountKind::Notes, Measured::Known(42)),
                (CountKind::SearchDocuments, Measured::Unknown),
            ],
            capabilities: vec![(
                CapabilityKind::LatexCompile,
                Status::Unknown {
                    reason_key: "no.toolchain".to_owned(),
                },
            )],
            statuses: vec![ScopeStatus {
                scope: Scope::Storage,
                status: Status::Ready,
            }],
            last_error_code: Some("error.saveFailed".to_owned()),
            timestamp_millis: 1_700_000_000_000,
            raw_log: None,
        }
    }

    #[test]
    fn an_unreadable_count_is_unknown_not_zero() {
        // F44-R04: a broken counter must not look like an idle system.
        let preview = preview(&request());
        let count = preview
            .fields
            .iter()
            .find(|field| field.key == "count.searchDocuments")
            .expect("field");
        assert_eq!(count.value, FieldValue::Unknown);
        assert_ne!(count.value, FieldValue::Integer(0));
    }

    #[test]
    fn only_allowlisted_fields_are_reported() {
        let preview = preview(&request());
        let keys: Vec<&str> = preview
            .fields
            .iter()
            .map(|field| field.key.as_str())
            .collect();
        assert!(keys.contains(&"appVersion"));
        assert!(keys.contains(&"count.notes"));
        // Paths and content are never fields; they are named as omissions.
        assert!(!keys.iter().any(|key| key.contains("path")));
        assert!(!keys.iter().any(|key| key.contains("content")));
        assert!(preview
            .omitted
            .iter()
            .any(|omission| omission.key == "notePaths"));
    }

    #[test]
    fn an_unreachable_service_is_unknown_not_healthy() {
        // STATUS_UNKNOWN: do not report health when the check could not run.
        let preview = preview(&request());
        let capability = preview
            .fields
            .iter()
            .find(|field| field.key == "capability.latexCompile")
            .expect("capability");
        assert_eq!(capability.value, FieldValue::Text("unknown".to_owned()));
        assert_ne!(capability.value, FieldValue::Text("ready".to_owned()));
    }

    #[test]
    fn overall_status_never_claims_health_when_a_scope_is_unknown() {
        let mut input = request();
        input.statuses = vec![
            ScopeStatus {
                scope: Scope::Storage,
                status: Status::Ready,
            },
            ScopeStatus {
                scope: Scope::Network,
                status: Status::Unknown {
                    reason_key: "network.unreachable".to_owned(),
                },
            },
        ];
        let preview = preview(&input);
        let overall = overall(&preview);
        assert!(!overall.is_ready());
    }

    #[test]
    fn overall_status_reports_degraded_when_something_is_wrong() {
        let mut input = request();
        input.statuses = vec![ScopeStatus {
            scope: Scope::Backup,
            status: Status::Degraded {
                reason_key: "backup.failed".to_owned(),
            },
        }];
        let preview = preview(&input);
        assert!(matches!(overall(&preview), Status::Degraded { .. }));
    }

    #[test]
    fn all_ready_scopes_report_ready() {
        // Every scope *and* every capability must be ready: a capability whose
        // state is unknown leaves the report unknown even when the scopes pass.
        let mut input = request();
        input.statuses = vec![
            ScopeStatus {
                scope: Scope::Storage,
                status: Status::Ready,
            },
            ScopeStatus {
                scope: Scope::Compile,
                status: Status::Ready,
            },
        ];
        input.capabilities = vec![
            (CapabilityKind::Search, Status::Ready),
            (CapabilityKind::LatexCompile, Status::Ready),
        ];
        assert!(overall(&preview(&input)).is_ready());
    }

    #[test]
    fn an_unknown_capability_makes_the_report_unknown_not_healthy() {
        // The bug a host test caught: only scopes were consulted, so a build whose
        // compiler state was unknown still reported healthy.
        let mut input = request();
        input.statuses = vec![ScopeStatus {
            scope: Scope::Storage,
            status: Status::Ready,
        }];
        input.capabilities = vec![(
            CapabilityKind::LatexCompile,
            Status::Unknown {
                reason_key: "no.toolchain".to_owned(),
            },
        )];
        let overall = overall(&preview(&input));
        assert!(!overall.is_ready());
        assert!(matches!(overall, Status::Unknown { .. }));
    }

    #[test]
    fn a_degraded_capability_is_reported_as_degraded() {
        let mut input = request();
        input.statuses = vec![ScopeStatus {
            scope: Scope::Storage,
            status: Status::Ready,
        }];
        input.capabilities = vec![(
            CapabilityKind::Sync,
            Status::Degraded {
                reason_key: "sync.failed".to_owned(),
            },
        )];
        assert!(matches!(overall(&preview(&input)), Status::Degraded { .. }));
    }

    #[test]
    fn redaction_removes_roots_paths_credentials_and_emails() {
        let text = "open /home/alice/vault/notes/a.md\n\
pw C:\\Users\\bob\\doc.tex\n\
url https://user:secret@example.com/x\n\
mail alice@example.com\n";
        let (redacted, counts) = redact(text, &["/home/alice/vault".to_owned()]);
        assert!(!redacted.contains("/home/alice/vault/notes/a.md"));
        assert!(!redacted.contains("C:\\Users\\bob"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("alice@example.com"));
        assert!(counts.iter().any(|entry| entry.kind == "path"));
        assert!(counts.iter().any(|entry| entry.kind == "url-credential"));
        assert!(counts.iter().any(|entry| entry.kind == "email"));
    }

    #[test]
    fn redaction_reports_counts_but_never_the_removed_text() {
        // A report that quoted what it hid would leak what it claims to protect.
        let (_, counts) = redact("a /x/y b /p/q", &[]);
        let path = counts
            .iter()
            .find(|entry| entry.kind == "path")
            .expect("path");
        assert!(path.count >= 2);
        // The count is a number; there is no field holding the removed text.
        assert_eq!(path.kind, "path");
    }

    #[test]
    fn redaction_leaves_ordinary_text_alone() {
        let (redacted, counts) = redact("build started in 2.3 seconds", &[]);
        assert_eq!(redacted, "build started in 2.3 seconds");
        assert!(counts.is_empty());
    }

    #[test]
    fn a_relative_path_is_not_treated_as_private() {
        // `notes/a.md` carries no machine information, so redaction leaves it.
        let (redacted, _) = redact("see notes/a.md for details", &[]);
        assert!(redacted.contains("notes/a.md"));
    }

    #[test]
    fn redaction_finds_paths_inside_quotes_parentheses_and_assignments() {
        // Build logs rarely put a path at the start of a whitespace token:
        // TeX writes `(C:\vault\main.tex`, tooling writes `file=/opt/x`, and a
        // quoted path may contain spaces. All of them are absolute user paths.
        let text = "quoted \"/home/alice/my notes/chapter one.tex\"\n\
parenthetical (D:\\Notes\\project\\main.tex)\n\
assigned file=/opt/vault/chapter.md\n\
colon path:/opt/other.tex\n\
windows \"C:\\Program Files\\MiKTeX\\main.log\"\n";
        let (redacted, counts) = redact(text, &[]);
        assert!(!redacted.contains("my notes"));
        assert!(!redacted.contains("D:\\Notes"));
        assert!(!redacted.contains("/opt/vault"));
        assert!(!redacted.contains("/opt/other"));
        assert!(!redacted.contains("Program Files"));
        assert!(counts.iter().any(|entry| entry.kind == "path"));
    }

    #[test]
    fn redaction_keeps_the_log_message_around_a_path() {
        let (redacted, _) = redact("! I can't find file '/home/alice/thesis/main.tex'.", &[]);
        assert!(!redacted.contains("/home/alice"));
        assert!(redacted.contains("I can't find file"));
        assert!(redacted.ends_with("'."));
    }

    #[test]
    fn redaction_removes_tokens_and_secret_assignments() {
        let text = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig\n\
api_key=sk-live-1234567890abcdef\n\
https://api.example.com/?access_token=abc123&page=2\n\
password: hunter2\n";
        let (redacted, counts) = redact(text, &[]);
        assert!(!redacted.contains("eyJhbGci"));
        assert!(!redacted.contains("sk-live"));
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("hunter2"));
        // The scheme and the non-secret query parameter stay readable.
        assert!(redacted.contains("Bearer"));
        assert!(redacted.contains("access_token=<redacted>&page=2"));
        assert!(counts.iter().any(|entry| entry.kind == "token"));
    }

    #[test]
    fn ordinary_prose_and_urls_are_not_treated_as_credentials() {
        // `basic` is an authorization scheme only when a credential follows it.
        let (prose, counts) = redact("basic build failed after 2.3 seconds", &[]);
        assert_eq!(prose, "basic build failed after 2.3 seconds");
        assert!(counts.is_empty());
        // A URL is not a local path and `https:` is not a secret assignment.
        let (url, counts) = redact("download https://example.com/docs/guide.html", &[]);
        assert_eq!(url, "download https://example.com/docs/guide.html");
        assert!(counts.is_empty());
    }

    #[test]
    fn the_export_includes_the_redacted_log_not_the_raw_one() {
        // F44-R03: exporting the raw log would defeat the preview.
        let mut input = request();
        input.raw_log = Some("error in /home/alice/vault/main.tex\n".to_owned());
        let preview = preview(&input);
        assert!(preview.includes_log);
        let text = export_text(&preview, &input);
        assert!(!text.contains("/home/alice/vault/main.tex"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn the_export_never_contains_a_token_or_a_non_home_path() {
        // F44-A01 scans the exported report, not just the preview: a log path
        // outside the home root and an authorization token must both be gone.
        let mut input = request();
        input.raw_log = Some(
            "authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig\n\
vault D:\\Notes\\secret-vault\\draft.tex\n"
                .to_owned(),
        );
        let preview = preview(&input);
        let text = export_text(&preview, &input);
        assert!(!text.contains("eyJhbGci"));
        assert!(!text.contains("D:\\Notes"));
        assert!(!text.contains("secret-vault"));
        assert!(text.contains("Bearer"));
        assert!(text.contains("\"log\""));
    }

    #[test]
    fn the_export_removes_echoed_source_but_keeps_the_next_diagnostic() {
        // F44-A01 includes note content, not only paths. TeX echoes the source
        // after `l.<line>`; the continuation is indented, while the following
        // warning is a new compiler line that must remain useful.
        let mut input = request();
        input.raw_log = Some(
            concat!(
                "! Undefined control sequence.\n",
                "l.12 private draft sentence\n",
                "              more private prose\n",
                "LaTeX Warning: Reference `x' undefined.\n",
            )
            .to_owned(),
        );
        let preview = preview(&input);
        assert!(preview
            .redactions
            .iter()
            .any(|entry| entry.kind == "content" && entry.count == 1));
        let text = export_text(&preview, &input);
        assert!(!text.contains("private draft"));
        assert!(!text.contains("more private prose"));
        assert!(text.contains("l.12 <redacted>"));
        assert!(text.contains("LaTeX Warning"));
    }

    #[test]
    fn the_export_carries_the_correlation_id_and_omissions() {
        let input = request();
        let text = export_text(&preview(&input), &input);
        assert!(text.contains("cid-1"));
        assert!(text.contains("noteContent"));
        assert!(text.contains("correlationId"));
    }

    #[test]
    fn a_correlation_id_is_stable_and_carries_no_path() {
        let first = correlation_id("vault-1:op-1");
        let again = correlation_id("vault-1:op-1");
        assert_eq!(first, again);
        assert_ne!(first, correlation_id("vault-1:op-2"));
        assert!(!first.contains('/'));
        // Shaped like a uuid, so it can be pasted into a bug report.
        assert_eq!(first.len(), 36);
    }

    #[test]
    fn measured_values_never_fabricate_a_default() {
        let unknown: Measured<u64> = Measured::Unknown;
        assert!(!unknown.is_known());
        assert_eq!(unknown.value(), None);
        let known: Measured<u64> = Measured::Known(7);
        assert_eq!(known.value(), Some(&7));
    }

    #[test]
    fn a_preview_without_a_log_does_not_claim_one() {
        let preview = preview(&request());
        assert!(!preview.includes_log);
        let text = export_text(&preview, &request());
        assert!(!text.contains("\"log\""));
    }
}
