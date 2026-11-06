//! F24-R02/R03: Markdown and Notion exports, turned into vault notes.
//!
//! Notion appends a 32-character id to every file and folder name, links notes
//! with percent-encoded relative paths, and exports each database as a CSV next
//! to a folder of row pages. Nothing here guesses: a link that resolves is
//! rewritten, one that does not is left as written and reported, and a block this
//! build cannot convert keeps its original text and is reported too (F24-A02).

use std::collections::BTreeMap;

/// `Title 0123…` without its id; other names are returned unchanged.
pub fn strip_notion_id(name: &str) -> String {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    let cleaned = match stem.rsplit_once(' ') {
        Some((title, id)) if !title.trim().is_empty() => {
            // A database's full table is written as `Title <id>_all.csv`.
            let (id, suffix) = match id.strip_suffix("_all") {
                Some(id) => (id, "_all"),
                None => (id, ""),
            };
            if id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                format!("{}{suffix}", title.trim_end())
            } else {
                stem.to_owned()
            }
        }
        _ => stem.to_owned(),
    };
    match extension {
        Some(extension) => format!("{cleaned}.{extension}"),
        None => cleaned,
    }
}

/// A whole path with the id removed from every segment.
pub fn notion_clean_path(path: &str) -> String {
    path.split('/')
        .map(strip_notion_id)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = bytes.get(at + 1..at + 3)?;
            let value = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(value);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// Joins a relative target onto a folder; None when it leaves the root.
fn join_relative(folder: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = folder.split('/').filter(|part| !part.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// `target` as written from a note at `from`.
fn relative_path(from: &str, target: &str) -> String {
    let from_parts: Vec<&str> = folder_of(from)
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let to_parts: Vec<&str> = target.split('/').collect();
    let mut shared = 0;
    while shared < from_parts.len()
        && shared + 1 < to_parts.len()
        && from_parts[shared] == to_parts[shared]
    {
        shared += 1;
    }
    let mut out: Vec<&str> = vec![".."; from_parts.len() - shared];
    out.extend_from_slice(&to_parts[shared..]);
    out.join("/")
}

/// What became of a note's links.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConvertedLinks {
    pub text: String,
    /// Local targets that were not part of the import, left as written.
    pub dead: Vec<String>,
}

fn is_remote(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("//")
        || lower.starts_with('#')
        || lower.split_once(':').is_some_and(|(scheme, _)| {
            scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        })
}

/// Rewrites `[text](target)` and `![alt](target)` from source paths to where those
/// files went. `moved` maps each imported source path to its destination, and
/// `source_note` and `destination_note` say where this note came from and went.
/// Code is left alone, so an example in a fence stays an example.
pub fn convert_links(
    body: &str,
    source_note: &str,
    destination_note: &str,
    moved: &BTreeMap<String, String>,
) -> ConvertedLinks {
    let mut result = ConvertedLinks {
        text: String::with_capacity(body.len()),
        dead: Vec::new(),
    };
    let source_folder = folder_of(source_note);
    let mut fence: Option<&str> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            result.text.push_str(line);
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            });
            result.text.push_str(line);
            continue;
        }
        convert_line(line, source_folder, destination_note, moved, &mut result);
    }
    result
}

fn convert_line(
    line: &str,
    source_folder: &str,
    destination_note: &str,
    moved: &BTreeMap<String, String>,
    result: &mut ConvertedLinks,
) {
    let bytes = line.as_bytes();
    let mut at = 0usize;
    let mut in_code = false;
    while at < bytes.len() {
        if bytes[at] == b'`' {
            in_code = !in_code;
        }
        if !in_code && bytes[at] == b']' && bytes.get(at + 1) == Some(&b'(') {
            let open = at + 2;
            let angle = bytes.get(open) == Some(&b'<');
            let close = if angle {
                line[open..].find('>').map(|offset| open + offset)
            } else {
                // Balanced parentheses, so `a (1).md` written plainly stays whole.
                let mut depth = 0i32;
                line[open..]
                    .char_indices()
                    .find_map(|(offset, character)| match character {
                        '(' => {
                            depth += 1;
                            None
                        }
                        ')' if depth == 0 => Some(open + offset),
                        ')' => {
                            depth -= 1;
                            None
                        }
                        _ => None,
                    })
            };
            if let Some(close) = close {
                let inner_start = if angle { open + 1 } else { open };
                let written = &line[inner_start..close];
                let (path, fragment) = match written.split_once('#') {
                    Some((path, fragment)) => (path, Some(fragment)),
                    None => (written, None),
                };
                let end = if angle { close + 1 } else { close };
                let has_title = !angle && path.contains(' ') && path.contains('"');
                if !has_title && !path.is_empty() && !is_remote(written) {
                    let resolved = percent_decode(path)
                        .and_then(|decoded| join_relative(source_folder, &decoded))
                        .and_then(|source| moved.get(&source));
                    match resolved {
                        Some(destination) => {
                            let relative = relative_path(destination_note, destination);
                            let target = match fragment {
                                Some(fragment) => format!("{relative}#{fragment}"),
                                None => relative,
                            };
                            result.text.push_str("](");
                            if target.contains([' ', '(', ')']) {
                                result.text.push('<');
                                result.text.push_str(&target);
                                result.text.push('>');
                            } else {
                                result.text.push_str(&target);
                            }
                            at = end;
                            continue;
                        }
                        None => result.dead.push(written.to_owned()),
                    }
                }
            }
        }
        let character = line[at..].chars().next().unwrap_or(' ');
        result.text.push(character);
        at += character.len_utf8();
    }
}

/// Reports what a Notion page holds that plain Markdown does not.
/// The text itself stays in the note unchanged.
pub fn unsupported_blocks(body: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    let mut note = |label: &'static str| {
        if !found.contains(&label) {
            found.push(label);
        }
    };
    let mut fence = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if trimmed.starts_with("<aside") {
            note("callout");
        }
        if trimmed.starts_with("<details") {
            note("toggle");
        }
        if trimmed.starts_with("<table") {
            note("table");
        }
        if trimmed.contains("<iframe") {
            note("embed");
        }
    }
    found
}

/// Parses RFC 4180 CSV. Quoted fields may hold commas, quotes and line breaks.
/// None for bytes that are not UTF-8, or a quote that never closes.
pub fn parse_csv(bytes: &[u8]) -> Option<Vec<Vec<String>>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if quoted {
            match character {
                '"' if characters.peek() == Some(&'"') => {
                    field.push('"');
                    characters.next();
                }
                '"' => quoted = false,
                other => field.push(other),
            }
            continue;
        }
        match character {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' if characters.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            other => field.push(other),
        }
    }
    if quoted {
        return None;
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Some(rows)
}

fn yaml_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// One database row as properties: the first column names the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseRow {
    pub title: String,
    pub properties: Vec<(String, String)>,
}

/// Rows of a Notion database CSV, skipping blanks and columns without a name.
pub fn database_rows(rows: &[Vec<String>]) -> Vec<DatabaseRow> {
    let Some((header, body)) = rows.split_first() else {
        return Vec::new();
    };
    body.iter()
        .filter_map(|row| {
            let title = row.first()?.trim();
            if title.is_empty() {
                return None;
            }
            let properties = header
                .iter()
                .zip(row.iter())
                .skip(1)
                .filter(|(name, value)| !name.trim().is_empty() && !value.is_empty())
                .map(|(name, value)| (name.trim().to_owned(), value.clone()))
                .collect();
            Some(DatabaseRow {
                title: title.to_owned(),
                properties,
            })
        })
        .collect()
}

/// Puts a row's properties at the top of its note as front matter, and takes out
/// the copy Notion writes as `Name: value` lines under the title. Any other text
/// stays. A note that already has front matter keeps it, with the properties added.
pub fn apply_row_properties(body: &str, properties: &[(String, String)]) -> String {
    if properties.is_empty() {
        return body.to_owned();
    }
    let yaml: String = properties
        .iter()
        .map(|(name, value)| format!("{}: {}\n", yaml_quoted(name), yaml_quoted(value)))
        .collect();
    let without_copies = remove_property_lines(body, properties);
    if let Some(rest) = without_copies.strip_prefix("---\n") {
        if let Some(close) = rest.find("\n---") {
            let (existing, after) = rest.split_at(close + 1);
            return format!("---\n{existing}{yaml}{after}");
        }
    }
    format!("---\n{yaml}---\n{without_copies}")
}

/// Removes the `Name: value` block Notion puts right after a row page's title.
fn remove_property_lines(body: &str, properties: &[(String, String)]) -> String {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let mut at = 0;
    while at < lines.len() && lines[at].trim().is_empty() {
        at += 1;
    }
    if at < lines.len() && lines[at].starts_with('#') {
        at += 1;
    }
    let start = at;
    while at < lines.len() && lines[at].trim().is_empty() {
        at += 1;
    }
    let block = at;
    while at < lines.len() {
        let Some((name, value)) = lines[at].trim_end().split_once(':') else {
            break;
        };
        let matches = properties
            .iter()
            .any(|(key, known)| key == name.trim() && known.trim() == value.trim());
        if !matches {
            break;
        }
        at += 1;
    }
    if at == block {
        return body.to_owned();
    }
    let mut out: String = lines[..start].concat();
    out.push_str(&lines[at..].concat());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn the_notion_id_comes_off_names_and_paths() {
        assert_eq!(strip_notion_id(&format!("My page {ID}.md")), "My page.md");
        assert_eq!(strip_notion_id(&format!("Folder {ID}")), "Folder");
        assert_eq!(strip_notion_id("plain.md"), "plain.md");
        assert_eq!(strip_notion_id(&format!("DB {ID}_all.csv")), "DB_all.csv");
        assert_eq!(strip_notion_id(&format!("{ID}.md")), format!("{ID}.md"));
        assert_eq!(strip_notion_id("short abc.md"), "short abc.md");
        assert_eq!(
            notion_clean_path(&format!("A {ID}/B {ID}/图 {ID}.png")),
            "A/B/图.png"
        );
    }

    fn moved(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn links_follow_the_files_they_point_to() {
        let map = moved(&[
            ("A 1/B 2.md", "Imp/A/B.md"),
            ("A 1/x 3.png", "Imp/A/x.png"),
            ("C.md", "Imp/C.md"),
        ]);
        let out = convert_links(
            "[b](B%202.md) ![p](x%203.png) [c](../C.md#h)\n",
            "A 1/Self.md",
            "Imp/A/Self.md",
            &map,
        );
        assert_eq!(out.text, "[b](B.md) ![p](x.png) [c](../C.md#h)\n");
        assert!(out.dead.is_empty());
    }

    #[test]
    fn a_new_path_with_spaces_is_written_so_markdown_keeps_it_whole() {
        let map = moved(&[("B.md", "Imp/My note (1).md")]);
        let out = convert_links("[b](B.md)", "A.md", "Imp/A.md", &map);
        assert_eq!(out.text, "[b](<My note (1).md>)");
    }

    #[test]
    fn remote_links_and_code_stay_as_written_and_a_lost_link_is_reported() {
        let map = moved(&[("B.md", "B.md")]);
        let source = "[w](https://x.test/B.md) [m](mailto:a@b.c) [a](#here)\n`[b](B.md)`\n```\n[b](B.md)\n```\n[gone](None.md)\n";
        let out = convert_links(source, "A.md", "A.md", &map);
        assert_eq!(out.text, source);
        assert_eq!(out.dead, vec!["None.md".to_owned()]);
    }

    #[test]
    fn a_link_out_of_the_export_is_reported_not_followed() {
        let out = convert_links("[x](../../secret.md)", "A.md", "A.md", &BTreeMap::new());
        assert_eq!(out.dead, vec!["../../secret.md".to_owned()]);
    }

    #[test]
    fn csv_handles_quotes_commas_and_line_breaks() {
        let rows =
            parse_csv("\u{feff}Name,Note\r\n\"a, b\",\"say \"\"hi\"\"\nthere\"\nc,\n".as_bytes())
                .unwrap();
        assert_eq!(rows[0], vec!["Name", "Note"]);
        assert_eq!(rows[1], vec!["a, b", "say \"hi\"\nthere"]);
        assert_eq!(rows[2], vec!["c", ""]);
        assert!(parse_csv(b"\"open").is_none());
        assert!(parse_csv(b"\xff\xfe").is_none());
    }

    #[test]
    fn rows_become_properties_and_blanks_are_dropped() {
        let rows = parse_csv(b"Name,Status,,Tags\nOne,Done,x,\n,Skip,,\n").unwrap();
        assert_eq!(
            database_rows(&rows),
            vec![DatabaseRow {
                title: "One".to_owned(),
                properties: vec![("Status".to_owned(), "Done".to_owned())],
            }]
        );
    }

    #[test]
    fn properties_become_front_matter_and_the_written_copy_goes() {
        let properties = vec![
            ("Status".to_owned(), "In \"QA\"".to_owned()),
            ("Date".to_owned(), "May 1".to_owned()),
        ];
        let body = "# One\n\nStatus: In \"QA\"\nDate: May 1\n\nReal: text stays\n";
        assert_eq!(
            apply_row_properties(body, &properties),
            "---\n\"Status\": \"In \\\"QA\\\"\"\n\"Date\": \"May 1\"\n---\n# One\n\nReal: text stays\n"
        );
    }

    #[test]
    fn body_text_that_only_looks_like_a_property_stays() {
        let properties = vec![("Status".to_owned(), "Done".to_owned())];
        let body = "# One\n\nStatus: not the same\n";
        assert_eq!(
            apply_row_properties(body, &properties),
            format!("---\n\"Status\": \"Done\"\n---\n{body}")
        );
    }

    #[test]
    fn existing_front_matter_keeps_its_fields() {
        let properties = vec![("S".to_owned(), "1".to_owned())];
        assert_eq!(
            apply_row_properties("---\na: b\n---\nx\n", &properties),
            "---\na: b\n\"S\": \"1\"\n---\nx\n"
        );
    }

    #[test]
    fn unsupported_blocks_are_named_once_and_code_is_ignored() {
        let body = "<aside>tip</aside>\n<aside>more</aside>\n```\n<details>\n```\n<table>\n";
        assert_eq!(unsupported_blocks(body), vec!["callout", "table"]);
    }
}
