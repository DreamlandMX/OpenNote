//! F53: which file an engine log was reading at each point.
//!
//! TeX prints `(` and a path when it opens a file and `)` when it closes it, so an
//! error's file is whatever sits on top of that stack. The log is not written for
//! parsing: paths wrap at 79 columns, and a message can hold stray brackets. So
//! error text is not scanned, and an answer is only given while the stack is
//! consistent. Saying nothing is better than naming the wrong file.

/// One step through the log.
fn looks_like_path(token: &str) -> bool {
    !token.is_empty()
        && !token.contains(['<', '>', '[', ']', '{', '}', '"'])
        && (token.contains('.') || token.contains('/') || token.contains('\\'))
}

/// The column TeX wraps its log output at.
const WRAP_COLUMN: usize = 79;

/// For each byte offset asked for, the file on top of the stack there.
/// `offsets` must be sorted ascending; the result lines up with it.
pub fn files_at(log: &str, offsets: &[usize]) -> Vec<Option<String>> {
    let mut answers = vec![None; offsets.len()];
    let mut next = 0usize;
    // `None` stands for a bracket that opened something other than a file.
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut consistent = true;
    let mut position = 0usize;
    let mut skip_context = false;
    let mut lines = log.split_inclusive('\n').peekable();
    while let Some(raw) = lines.next() {
        let line_start = position;
        position += raw.len();
        while next < offsets.len() && offsets[next] < position {
            if offsets[next] >= line_start {
                if consistent {
                    answers[next] = stack.iter().rev().find_map(|entry| entry.clone());
                }
                next += 1;
            } else {
                next += 1;
            }
        }
        let line = raw.trim_end_matches(['\r', '\n']);
        // An error's message and its echo of the source are the user's text, not
        // the engine's bookkeeping; a bracket there says nothing about files.
        if line.starts_with('!') {
            skip_context = true;
            continue;
        }
        if skip_context {
            if line.is_empty() {
                skip_context = false;
            }
            continue;
        }
        let mut characters = line.char_indices().peekable();
        while let Some((at, character)) = characters.next() {
            match character {
                '(' => {
                    // The token runs to a space or a bracket, possibly over a wrap.
                    let mut token = String::new();
                    let mut end = line.len();
                    for (offset, inner) in line[at + 1..].char_indices() {
                        if inner == ' ' || inner == '(' || inner == ')' {
                            end = at + 1 + offset;
                            break;
                        }
                        token.push(inner);
                    }
                    let mut reached_end = end == line.len();
                    let mut wrapped_length = line.chars().count();
                    while reached_end && wrapped_length == WRAP_COLUMN {
                        let Some(following) = lines.peek() else {
                            break;
                        };
                        let following = following.trim_end_matches(['\r', '\n']);
                        let piece: String = following
                            .chars()
                            .take_while(|inner| !matches!(inner, ' ' | '(' | ')'))
                            .collect();
                        reached_end = piece.len() == following.len();
                        wrapped_length = following.chars().count();
                        token.push_str(&piece);
                        if !reached_end {
                            break;
                        }
                        let consumed = lines.next().unwrap_or("");
                        position += consumed.len();
                    }
                    stack.push(looks_like_path(&token).then_some(token));
                    while let Some((peeked, _)) = characters.peek() {
                        if *peeked >= end {
                            break;
                        }
                        characters.next();
                    }
                }
                ')' => {
                    if stack.pop().is_none() {
                        // More closes than opens: this log cannot be trusted any more.
                        consistent = false;
                    }
                }
                _ => {}
            }
        }
    }
    answers
}

/// The line number a warning states, such as `on input line 12`.
pub fn warning_line(raw: &str) -> Option<u64> {
    let (_, after) = raw.split_once("input line ")?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    if let Ok(line) = digits.parse() {
        return Some(line);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_at_marker(log: &str) -> Option<String> {
        let at = log.find("! ").expect("marker");
        files_at(log, &[at]).pop().flatten()
    }

    #[test]
    fn an_error_belongs_to_the_innermost_open_file() {
        let log =
            "(main.tex (./sections/intro.tex\n! Undefined control sequence.\nl.3 \\foo\n\n) )\n";
        assert_eq!(file_at_marker(log).as_deref(), Some("./sections/intro.tex"));
    }

    #[test]
    fn a_closed_file_is_no_longer_the_answer() {
        let log = "(main.tex (./a.tex) (./b.tex)\n! Undefined control sequence.\nl.9 \\foo\n\n)\n";
        assert_eq!(file_at_marker(log).as_deref(), Some("main.tex"));
    }

    #[test]
    fn an_absolute_windows_path_is_kept_whole() {
        let log = "(C:\\Users\\me\\jobs\\1\\input\\main.tex\n! Undefined control sequence.\n";
        assert_eq!(
            file_at_marker(log).as_deref(),
            Some("C:\\Users\\me\\jobs\\1\\input\\main.tex")
        );
    }

    #[test]
    fn a_path_wrapped_at_79_columns_is_joined() {
        let first = format!("(C:/{}", "a".repeat(75));
        assert_eq!(first.chars().count(), 79);
        let log = format!("{first}\nbbb/main.tex\n! Undefined control sequence.\n");
        assert_eq!(
            file_at_marker(&log),
            Some(format!("C:/{}bbb/main.tex", "a".repeat(75)))
        );
    }

    #[test]
    fn brackets_in_messages_and_source_echoes_do_not_count() {
        let log =
            "(main.tex\n! Missing ) inserted.\nl.3 f(x)) + (\n\n! Undefined control sequence.\n";
        // The second error is still read as being in main.tex.
        let at = log.rfind("! ").unwrap();
        assert_eq!(
            files_at(log, &[at]).pop().flatten().as_deref(),
            Some("main.tex")
        );
    }

    #[test]
    fn a_package_note_in_brackets_is_not_a_file() {
        let log = "(main.tex (hyperref) rerun for links\n! Undefined control sequence.\n";
        assert_eq!(file_at_marker(log).as_deref(), Some("main.tex"));
    }

    #[test]
    fn an_unbalanced_log_gives_no_answer() {
        let log = ") ) (main.tex\n! Undefined control sequence.\n";
        assert_eq!(file_at_marker(log), None);
    }

    #[test]
    fn no_open_file_gives_no_answer() {
        assert_eq!(file_at_marker("! Undefined control sequence.\n"), None);
    }

    #[test]
    fn several_offsets_line_up_with_their_answers() {
        let log = "(a.tex\n! one\n\n)\n(b.tex\n! two\n\n)\n";
        let first = log.find("! one").unwrap();
        let second = log.find("! two").unwrap();
        assert_eq!(
            files_at(log, &[first, second]),
            vec![Some("a.tex".to_owned()), Some("b.tex".to_owned())]
        );
    }

    #[test]
    fn a_warning_states_its_line() {
        assert_eq!(
            warning_line("LaTeX Warning: Citation `x' on page 1 undefined on input line 12."),
            Some(12)
        );
        assert_eq!(warning_line("LaTeX Warning: nothing here"), None);
    }
}
