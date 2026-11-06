pub(crate) fn local_path(note_path: &str, raw: &str) -> Option<Result<String, ()>> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('#') || raw.starts_with("//") {
        return None;
    }
    if raw.len() >= 2 && raw.as_bytes()[0].is_ascii_alphabetic() && raw.as_bytes()[1] == b':' {
        return Some(Err(()));
    }
    if has_remote_scheme(raw) {
        return None;
    }
    let path = raw.split(['#', '?']).next().unwrap_or("");
    if path.is_empty() {
        return None;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Some(Err(()));
    }
    let decoded = percent_decode(path).unwrap_or_else(|| path.to_owned());
    let normalized = decoded.replace('\\', "/");
    let mut parts = note_path.split('/').map(str::to_owned).collect::<Vec<_>>();
    parts.pop();
    for part in normalized.split('/') {
        if part.is_empty() || part.chars().any(|c| c.is_control() || c == ':') {
            return Some(Err(()));
        }
        if part == "." {
            continue;
        }
        if part == ".." {
            if parts.pop().is_none() {
                return Some(Err(()));
            }
            continue;
        }
        if part.eq_ignore_ascii_case(".opennote") {
            return Some(Err(()));
        }
        parts.push(part.into());
    }
    if parts.is_empty() {
        return Some(Err(()));
    }
    Some(Ok(parts.join("/")))
}

fn has_remote_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !matches!(scheme.to_ascii_lowercase().as_str(), "file" | "javascript")
}

fn percent_decode(value: &str) -> Option<String> {
    let source = value.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        if source[index] == b'%' && index + 2 < source.len() {
            let high = hex_value(source[index + 1]);
            let low = hex_value(source[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(source[index]);
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn markdown_targets(source: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut lines = source.lines();
    let front_matter = lines
        .next()
        .is_some_and(|line| line.trim_end_matches('\r').trim() == "---");
    let mut in_front_matter = front_matter;
    let mut fence: Option<(char, usize)> = None;
    let all_lines = if front_matter {
        lines.collect::<Vec<_>>()
    } else {
        source.lines().collect::<Vec<_>>()
    };
    for line in all_lines {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if in_front_matter {
            if matches!(trimmed, "---" | "...") {
                in_front_matter = false;
            }
            continue;
        }
        if let Some((marker, count)) = fence_marker(trimmed) {
            match fence {
                Some((open, size)) if open == marker && count >= size => fence = None,
                None => fence = Some((marker, count)),
                _ => (),
            }
            continue;
        }
        if fence.is_some() || line.starts_with("    ") || line.starts_with('\t') {
            continue;
        }
        let visible = strip_inline_code(line);
        targets.extend(inline_targets(&visible));
        if let Some(target) = reference_definition(&visible) {
            targets.push(target);
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let count = line.chars().take_while(|c| *c == marker).count();
    (count >= 3).then_some((marker, count))
}

fn strip_inline_code(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut chars = line.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch != '`' {
            result.push(ch);
            continue;
        }
        let mut ticks = 1;
        while chars.peek().is_some_and(|(_, next)| *next == '`') {
            chars.next();
            ticks += 1;
        }
        let mut closing = 0;
        while let Some((_, next)) = chars.next() {
            if next == '`' {
                closing += 1;
                if closing == ticks {
                    break;
                }
            } else {
                closing = 0;
            }
        }
        result.push(' ');
    }
    result
}

fn inline_targets(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut targets = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'[' || escaped(bytes, index) {
            index += 1;
            continue;
        }
        let Some(close) = find_unescaped(bytes, index + 1, b']') else {
            break;
        };
        if bytes.get(close + 1) != Some(&b'(') {
            index = close + 1;
            continue;
        }
        if let Some((target, end)) = destination(bytes, close + 2) {
            targets.push(target);
            index = end;
        } else {
            index = close + 2;
        }
    }
    targets
}

fn reference_definition(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let close = find_unescaped(bytes, 1, b']')?;
    if bytes.get(close + 1) != Some(&b':') {
        return None;
    }
    definition_destination(bytes, close + 2)
}

fn definition_destination(bytes: &[u8], mut index: usize) -> Option<String> {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) == Some(&b'<') {
        let start = index + 1;
        let end = find_unescaped(bytes, start, b'>')?;
        return Some(unescape(&bytes[start..end]));
    }
    let start = index;
    while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
        if bytes[index] == b'\\' {
            index += 2;
        } else {
            index += 1;
        }
    }
    let end = index.min(bytes.len());
    let target = unescape(&bytes[start..end]);
    (!target.is_empty()).then_some(target)
}

fn destination(bytes: &[u8], mut index: usize) -> Option<(String, usize)> {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) == Some(&b'<') {
        let start = index + 1;
        let end = find_unescaped(bytes, start, b'>')?;
        return Some((unescape(&bytes[start..end]), end + 1));
    }
    let start = index;
    let mut depth = 1usize;
    while index < bytes.len() {
        let value = bytes[index];
        if value == b'\\' {
            index += 2;
            continue;
        }
        match value {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let target = unescape(&bytes[start..index]);
                    return (!target.is_empty()).then_some((target, index + 1));
                }
            }
            value if value.is_ascii_whitespace() && depth == 1 => {
                let target = unescape(&bytes[start..index]);
                return (!target.is_empty()).then_some((target, index));
            }
            _ => (),
        }
        index += 1;
    }
    None
}

fn find_unescaped(bytes: &[u8], mut index: usize, wanted: u8) -> Option<usize> {
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == wanted {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn escaped(bytes: &[u8], index: usize) -> bool {
    let mut slashes = 0;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

fn unescape(bytes: &[u8]) -> String {
    let mut value = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 1 < bytes.len() {
            index += 1;
        }
        value.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&value).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_inline_and_definition_targets_without_code_or_remote_urls() {
        let source = r#"---
image: ![front](front.png)
---
![local](images/a.png "title")
[doc](<files/a b.pdf>)
[remote](https://example.com/a.png)
`![code](ignored.png)`
```
![fenced](ignored-too.png)
```
[ref]: ../assets/ref.png
"#;
        assert_eq!(
            markdown_targets(source),
            vec![
                "../assets/ref.png",
                "files/a b.pdf",
                "https://example.com/a.png",
                "images/a.png",
            ]
        );
    }

    #[test]
    fn local_paths_allow_nested_parents_but_reject_vault_escape() {
        assert_eq!(
            local_path("notes/a.md", "images/a%20b.png#part"),
            Some(Ok("notes/images/a b.png".into()))
        );
        assert_eq!(local_path("notes/a.md", "../../escape.png"), Some(Err(())));
        assert_eq!(local_path("notes/a.md", "C:/absolute.png"), Some(Err(())));
        assert_eq!(local_path("notes/a.md", "/absolute.png"), Some(Err(())));
        assert_eq!(local_path("notes/a.md", "https://example.com/a.png"), None);
        assert_eq!(local_path("notes/a.md", "#heading"), None);
    }
}
