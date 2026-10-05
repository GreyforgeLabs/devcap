//! Small re-implementations of CPython string semantics that devcap's output
//! depends on (`str.isspace`, `str.splitlines`, `repr`, `float()`/`int()`
//! parsing, `posixpath.normpath`). Unicode properties come from tables
//! generated from CPython itself (`scripts/gen_unicode_tables.py`).

use crate::unicode_tables::{ALNUM, DECIMAL, PRINTABLE, SPACE};

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let cp = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// First code point used to carry an undecodable OS byte (see [`fsdecode`]).
const ESCAPE_BASE: u32 = 0x10_FF00;

/// Decode OS bytes (argv, environment, paths) like CPython's
/// `os.fsdecode` with `surrogateescape`: valid UTF-8 is kept and every
/// undecodable byte `0xXY` becomes a private escape character. Rust strings
/// cannot hold lone surrogates, so the escapes live at U+10FF80..=U+10FFFF
/// (the end of Supplementary Private Use Area-B); [`fsencode`] restores the
/// original bytes, and output renders them the way CPython renders
/// `\udcXY`. Decoding then encoding is lossless for every byte string; a
/// genuine U+10FF80..=U+10FFFF character in OS input is displayed as its
/// escaped UTF-8 bytes, and one in other text (profile data, probe output)
/// is rendered as if it were an escape (a documented limitation).
pub fn fsdecode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let escape = |out: &mut String, b: u8| {
        out.push(char::from_u32(ESCAPE_BASE + u32::from(b)).unwrap_or('\u{fffd}'));
    };
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if escaped_byte(c).is_some() {
                // Keep the round trip lossless: a genuine character from the
                // escape range is carried as its four escaped UTF-8 bytes.
                for &b in c.encode_utf8(&mut [0u8; 4]).as_bytes() {
                    escape(&mut out, b);
                }
            } else {
                out.push(c);
            }
        }
        for &b in chunk.invalid() {
            escape(&mut out, b);
        }
    }
    out
}

/// The raw byte an escape character from [`fsdecode`] stands for.
pub fn escaped_byte(c: char) -> Option<u8> {
    let cp = c as u32;
    (ESCAPE_BASE + 0x80..=ESCAPE_BASE + 0xff)
        .contains(&cp)
        .then(|| (cp - ESCAPE_BASE) as u8)
}

/// Inverse of [`fsdecode`] (`os.fsencode` with `surrogateescape`).
pub fn fsencode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match escaped_byte(c) {
            Some(b) => out.push(b),
            None => out.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes()),
        }
    }
    out
}

/// A filesystem path for a string produced by [`fsdecode`].
pub fn os_path(s: &str) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStringExt;
    std::path::PathBuf::from(std::ffi::OsString::from_vec(fsencode(s)))
}

/// Render escape characters as CPython's `backslashreplace` error handler
/// writes lone surrogates to stderr (`\udcXY`).
pub fn backslash_escapes(s: &str) -> String {
    if !s.chars().any(|c| escaped_byte(c).is_some()) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match escaped_byte(c) {
            Some(b) => out.push_str(&format!("\\udc{b:02x}")),
            None => out.push(c),
        }
    }
    out
}

/// `str.isspace()` for a single character.
pub fn is_space(c: char) -> bool {
    if c.is_ascii() {
        matches!(
            c,
            ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\x1c'..='\x1f'
        )
    } else {
        in_table(SPACE, c)
    }
}

/// `str.isdecimal()` for a single character (regex `\d` on `str`).
pub fn is_decimal(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        in_table(DECIMAL, c)
    }
}

/// Decimal value of a `str.isdecimal()` character.
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    let cp = c as u32;
    DECIMAL
        .iter()
        .find(|&&(lo, hi)| lo <= cp && cp <= hi)
        .map(|&(lo, _)| (cp - lo) % 10)
}

/// Regex `\w` on `str`: `str.isalnum()` or underscore.
pub fn is_word(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || c == '_'
    } else {
        in_table(ALNUM, c)
    }
}

/// `str.isprintable()` for a single character.
pub fn is_printable(c: char) -> bool {
    if c.is_ascii() {
        (' '..='~').contains(&c)
    } else {
        in_table(PRINTABLE, c)
    }
}

/// `str.strip()` with no arguments.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.split()` with no arguments.
pub fn split_whitespace(s: &str) -> impl Iterator<Item = &str> {
    s.split(is_space).filter(|part| !part.is_empty())
}

fn is_line_boundary(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `str.splitlines(keepends)`.
pub fn splitlines(s: &str, keepends: bool) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((idx, c)) = iter.next() {
        if !is_line_boundary(c) {
            continue;
        }
        let mut end = idx + c.len_utf8();
        if c == '\r' && bytes.get(end) == Some(&b'\n') {
            iter.next();
            end += 1;
        }
        lines.push(if keepends {
            &s[start..end]
        } else {
            &s[start..idx]
        });
        start = end;
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// Number of Unicode code points (Python `len`).
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Python `repr()` of a `str`.
pub fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if escaped_byte(c).is_some() => out.push_str(&backslash_escapes(&c.to_string())),
            c if is_printable(c) => out.push(c),
            c => {
                let cp = c as u32;
                if cp < 0x100 {
                    out.push_str(&format!("\\x{cp:02x}"));
                } else if cp < 0x10000 {
                    out.push_str(&format!("\\u{cp:04x}"));
                } else {
                    out.push_str(&format!("\\U{cp:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// `_PyUnicode_TransformDecimalAndSpaceToASCII`: returns `None` when a
/// non-ASCII character is neither whitespace nor a decimal digit.
fn transform_decimal_and_space(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if (c as u32) < 127 {
            out.push(c);
        } else if is_space(c) {
            out.push(' ');
        } else {
            let d = decimal_value(c)?;
            out.push(char::from(b'0' + d as u8));
        }
    }
    Some(out)
}

fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

fn trim_c_space(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii() && is_c_space(c as u8))
}

/// Apply CPython's underscore rule (only between digits) and drop them.
fn remove_underscores(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut prev = '\0';
    for c in s.chars() {
        if c == '_' {
            if !prev.is_ascii_digit() {
                return None;
            }
        } else {
            if prev == '_' && !c.is_ascii_digit() {
                return None;
            }
            out.push(c);
        }
        prev = c;
    }
    if prev == '_' {
        return None;
    }
    Some(out)
}

fn valid_float_syntax(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let lower = body.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
        return true;
    }
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut int_digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        int_digits += 1;
    }
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            frac_digits += 1;
        }
    }
    if int_digits + frac_digits == 0 {
        return false;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let mut exp_digits = 0;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            exp_digits += 1;
        }
        if exp_digits == 0 {
            return false;
        }
    }
    i == bytes.len()
}

/// Python `float(str)`. Returns `None` where CPython raises `ValueError`.
pub fn parse_float(value: &str) -> Option<f64> {
    let ascii = transform_decimal_and_space(value)?;
    let trimmed = trim_c_space(&ascii);
    let cleaned = if trimmed.contains('_') {
        remove_underscores(trimmed)?
    } else {
        trimmed.to_string()
    };
    if !valid_float_syntax(&cleaned) {
        return None;
    }
    let (negative, body) = match cleaned.as_bytes().first() {
        Some(b'-') => (true, &cleaned[1..]),
        Some(b'+') => (false, &cleaned[1..]),
        _ => (false, cleaned.as_str()),
    };
    let lower = body.to_ascii_lowercase();
    let magnitude = match lower.as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => body.parse::<f64>().ok()?,
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// CPython's default limit on decimal digits in `int(str)` conversions.
pub const INT_MAX_STR_DIGITS: usize = 4300;

/// The `ValueError` text CPython raises past [`INT_MAX_STR_DIGITS`].
pub fn int_limit_message(digits: usize) -> String {
    format!(
        "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: \
         value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
    )
}

/// Result of Python `int(str)` reduced to what devcap needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyInt {
    /// Fits in an `i64`.
    Value(i64),
    /// Syntactically valid but outside the `i64` range.
    Huge { negative: bool },
}

/// Python `int(str)` (base 10). Returns `None` where CPython raises `ValueError`.
pub fn parse_int(value: &str) -> Option<PyInt> {
    let ascii = transform_decimal_and_space(value)?;
    let trimmed = trim_c_space(&ascii);
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() || !digits.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let cleaned = remove_underscores(digits)?;
    if cleaned.is_empty() || !cleaned.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // CPython's default `sys.get_int_max_str_digits()` limit (leading zeros
    // count, underscores and the sign do not).
    if cleaned.len() > INT_MAX_STR_DIGITS {
        return None;
    }
    let significant = cleaned.trim_start_matches('0');
    if significant.len() > 18 {
        return Some(PyInt::Huge { negative });
    }
    let magnitude: i64 = if significant.is_empty() {
        0
    } else {
        significant.parse().ok()?
    };
    Some(PyInt::Value(if negative { -magnitude } else { magnitude }))
}

/// Python `%g` formatting for the small set of values devcap prints.
pub fn format_g(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// `posixpath.normpath`.
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let initial_slashes = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else if path.starts_with('/') {
        1
    } else {
        0
    };
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial_slashes == 0 && comps.is_empty()) || comps.last() == Some(&"..")
        {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let joined = comps.join("/");
    let result = format!("{}{}", "/".repeat(initial_slashes), joined);
    if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

/// `posixpath.join(a, b)` for two components.
pub fn path_join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsdecode_round_trips_and_renders_like_surrogateescape() {
        let raw: &[u8] = b"a\xff/\xc3\xa9\xed\xa0\x80\xf4\x8f\xbf\xbf.toml";
        let decoded = fsdecode(raw);
        assert_eq!(fsencode(&decoded), raw);
        assert!(decoded.contains('\u{e9}'));
        // CPython: os.fsdecode(raw).encode("ascii", "backslashreplace")
        assert_eq!(
            backslash_escapes(&decoded),
            "a\\udcff/\u{e9}\\udced\\udca0\\udc80\\udcf4\\udc8f\\udcbf\\udcbf.toml"
        );
        assert_eq!(repr(&fsdecode(b"x\xfe")), "'x\\udcfe'");
        assert_eq!(fsdecode(b"plain"), "plain");
        assert_eq!(escaped_byte('a'), None);
    }

    #[test]
    fn parse_int_enforces_cpython_digit_limit() {
        let ok = format!("{}1", "0".repeat(4299));
        assert_eq!(parse_int(&ok), Some(PyInt::Value(1)));
        assert_eq!(parse_int(&format!("{}1", "0".repeat(4300))), None);
        assert_eq!(parse_int(&format!("-{}", "1".repeat(4301))), None);
        assert_eq!(
            parse_int(&format!("{}1", "1_".repeat(4299))),
            Some(PyInt::Huge { negative: false })
        );
        assert_eq!(parse_int(&format!("{}1", "1_".repeat(4300))), None);
    }

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines("a\nb\r\nc\rd", false), vec!["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\nb\n", true), vec!["a\n", "b\n"]);
        assert_eq!(splitlines("\n\n", false), vec!["", ""]);
        assert_eq!(splitlines("x\u{2028}y\x0bz", false), vec!["x", "y", "z"]);
        assert!(splitlines("", false).is_empty());
    }

    #[test]
    fn repr_matches_python() {
        assert_eq!(repr("abc"), "'abc'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(repr("\x00\n\x7f"), "'\\x00\\n\\x7f'");
        assert_eq!(repr("é\u{a0}"), "'é\\xa0'");
        assert_eq!(repr("'''"), "\"'''\"");
    }

    #[test]
    fn float_parsing_matches_python() {
        assert_eq!(parse_float(" 1_0 "), Some(10.0));
        assert_eq!(parse_float("1__0"), None);
        assert_eq!(parse_float("_1"), None);
        assert_eq!(parse_float("1_"), None);
        assert_eq!(parse_float("1_.5"), None);
        assert_eq!(parse_float("-1e5"), Some(-1e5));
        assert_eq!(parse_float("-.5"), Some(-0.5));
        assert_eq!(parse_float("5."), Some(5.0));
        assert_eq!(parse_float("1.e2"), Some(100.0));
        assert_eq!(parse_float("."), None);
        assert_eq!(parse_float(".e5"), None);
        assert_eq!(parse_float("1e"), None);
        assert_eq!(parse_float("-1x"), None);
        assert!(parse_float("nan").unwrap().is_nan());
        assert_eq!(parse_float("-Infinity"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_float("1e400"), Some(f64::INFINITY));
        assert_eq!(parse_float("\u{663}.\u{665}"), Some(3.5));
        assert_eq!(parse_float(""), None);
        assert_eq!(parse_float("0x10"), None);
    }

    #[test]
    fn int_parsing_matches_python() {
        assert_eq!(parse_int(" +0_8 "), Some(PyInt::Value(8)));
        assert_eq!(parse_int("08"), Some(PyInt::Value(8)));
        assert_eq!(parse_int("-0"), Some(PyInt::Value(0)));
        assert_eq!(parse_int("\u{663}"), Some(PyInt::Value(3)));
        assert_eq!(
            parse_int("99999999999999999999999"),
            Some(PyInt::Huge { negative: false })
        );
        assert_eq!(parse_int("1.5"), None);
        assert_eq!(parse_int("_1"), None);
        assert_eq!(parse_int("+_1"), None);
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("x"), None);
    }

    #[test]
    fn normpath_matches_python() {
        assert_eq!(normpath("/usr//bin/./git"), "/usr/bin/git");
        assert_eq!(normpath("//a/b"), "//a/b");
        assert_eq!(normpath("///a/b"), "/a/b");
        assert_eq!(normpath("/a/../../b"), "/b");
        assert_eq!(normpath("a/../../b"), "../b");
        assert_eq!(normpath(""), ".");
    }

    #[test]
    fn whitespace_matches_python() {
        assert!(is_space('\x1c'));
        assert!(is_space('\u{3000}'));
        assert!(!is_space('\u{200b}'));
        assert_eq!(strip("\x1f a \u{a0}"), "a");
        assert_eq!(
            split_whitespace(" a  b\x1dc ").collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }
}
