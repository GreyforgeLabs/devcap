//! Text cleanup helpers for terminal and Markdown output.

use crate::pycompat::{char_len, split_whitespace};

/// Remove ANSI escape sequences matching
/// `\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])`.
fn strip_ansi(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b'
            && let Some(len) = ansi_match_len(&chars[i + 1..])
        {
            i += 1 + len;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn ansi_match_len(rest: &[char]) -> Option<usize> {
    let first = *rest.first()?;
    if ('@'..='Z').contains(&first) || ('\\'..='_').contains(&first) {
        return Some(1);
    }
    if first != '[' {
        return None;
    }
    let mut i = 1;
    while i < rest.len() && ('0'..='?').contains(&rest[i]) {
        i += 1;
    }
    while i < rest.len() && (' '..='/').contains(&rest[i]) {
        i += 1;
    }
    if i < rest.len() && ('@'..='~').contains(&rest[i]) {
        Some(i + 1)
    } else {
        None
    }
}

fn is_control(c: char) -> bool {
    matches!(c, '\x00'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f' | '\x7f'..='\u{9f}')
}

/// Return a single-line string without terminal control sequences.
pub fn clean_text(value: &str, max_length: Option<usize>) -> String {
    let text = strip_ansi(value);
    let text: String = text
        .chars()
        .map(|c| {
            if is_control(c) || matches!(c, '\r' | '\n' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let text = split_whitespace(&text).collect::<Vec<_>>().join(" ");
    match max_length {
        Some(max) if char_len(&text) > max => {
            let keep: String = text.chars().take(max.saturating_sub(3)).collect();
            format!("{keep}...")
        }
        _ => text,
    }
}

/// Escape a value for use inside a Markdown table cell.
pub fn markdown_cell(value: &str, max_length: Option<usize>) -> String {
    let cleaned = clean_text(value, max_length);
    let mut out = String::with_capacity(cleaned.len());
    for c in cleaned.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '`' => out.push_str("\\`"),
            c => out.push(c),
        }
    }
    out
}

/// Format a sanitized value as Markdown inline code.
pub fn markdown_inline_code(value: &str, max_length: Option<usize>) -> String {
    format!("`{}`", markdown_cell(value, max_length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_and_controls() {
        assert_eq!(clean_text("\x1b[31mred\x1b[0m", None), "red");
        assert_eq!(clean_text("a\x1b[12", None), "a [12");
        assert_eq!(clean_text("a\x1bMb", None), "ab");
        assert_eq!(clean_text("a\x1b[b", None), "a");
        assert_eq!(clean_text("bad\x07line", None), "bad line");
        assert_eq!(clean_text("x\u{85}y", None), "x y");
        assert_eq!(clean_text("  a \n\t b  ", None), "a b");
    }

    #[test]
    fn truncates_by_code_points() {
        let long = "é".repeat(100);
        let cleaned = clean_text(&long, Some(80));
        assert_eq!(cleaned.chars().count(), 80);
        assert!(cleaned.ends_with("..."));
        assert_eq!(clean_text("abc", Some(3)), "abc");
    }

    #[test]
    fn markdown_escaping() {
        assert_eq!(
            markdown_cell("a|b`c\\d<e>&", None),
            "a\\|b\\`c\\\\d&lt;e&gt;&amp;"
        );
        assert_eq!(markdown_inline_code("x|y", Some(120)), "`x\\|y`");
    }
}
