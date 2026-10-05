//! Minimal JSON value + writer reproducing `json.dumps(obj, indent=2)`
//! (with Python's default `ensure_ascii=True`).

/// A JSON value with insertion-ordered objects.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Convenience constructor for strings.
    pub fn str(value: &str) -> Json {
        Json::Str(value.to_string())
    }

    /// `Some(s)` → string, `None` → null.
    pub fn opt_str(value: Option<&str>) -> Json {
        value.map_or(Json::Null, Json::str)
    }

    /// Object field lookup (test helper).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            // An undecodable OS byte: `json.dumps` writes the lone surrogate.
            c if crate::pycompat::escaped_byte(c).is_some() => {
                out.push_str(&crate::pycompat::backslash_escapes(&c.to_string()));
            }
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Json, level: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(i) => out.push_str(&i.to_string()),
        Json::Str(s) => write_string(out, s),
        Json::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline_indent(out, level + 1);
                write_value(out, item, level + 1);
            }
            newline_indent(out, level);
            out.push(']');
        }
        Json::Object(fields) => {
            if fields.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline_indent(out, level + 1);
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item, level + 1);
            }
            newline_indent(out, level);
            out.push('}');
        }
    }
}

fn newline_indent(out: &mut String, level: usize) {
    out.push('\n');
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// Serialize like `json.dumps(value, indent=2)`.
pub fn dumps(value: &Json) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out
}

/// Parse JSON text (used by tests only; strict RFC 8259 subset devcap emits).
pub fn parse(text: &str) -> Result<Json, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    let value = parse_value(&chars, &mut pos)?;
    skip_ws(&chars, &mut pos);
    if pos != chars.len() {
        return Err(format!("trailing data at {pos}"));
    }
    Ok(value)
}

fn skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && matches!(chars[*pos], ' ' | '\n' | '\r' | '\t') {
        *pos += 1;
    }
}

fn expect_lit(chars: &[char], pos: &mut usize, lit: &str, value: Json) -> Result<Json, String> {
    for c in lit.chars() {
        if chars.get(*pos) != Some(&c) {
            return Err(format!("bad literal at {pos}"));
        }
        *pos += 1;
    }
    Ok(value)
}

fn parse_value(chars: &[char], pos: &mut usize) -> Result<Json, String> {
    skip_ws(chars, pos);
    match chars.get(*pos) {
        Some('n') => expect_lit(chars, pos, "null", Json::Null),
        Some('t') => expect_lit(chars, pos, "true", Json::Bool(true)),
        Some('f') => expect_lit(chars, pos, "false", Json::Bool(false)),
        Some('"') => parse_string(chars, pos).map(Json::Str),
        Some(c) if *c == '-' || c.is_ascii_digit() => {
            let start = *pos;
            *pos += 1;
            while chars.get(*pos).is_some_and(|c| c.is_ascii_digit()) {
                *pos += 1;
            }
            let text: String = chars[start..*pos].iter().collect();
            text.parse()
                .map(Json::Int)
                .map_err(|e| format!("bad integer at {start}: {e}"))
        }
        Some('[') => {
            *pos += 1;
            let mut items = Vec::new();
            skip_ws(chars, pos);
            if chars.get(*pos) == Some(&']') {
                *pos += 1;
                return Ok(Json::Array(items));
            }
            loop {
                items.push(parse_value(chars, pos)?);
                skip_ws(chars, pos);
                match chars.get(*pos) {
                    Some(',') => *pos += 1,
                    Some(']') => {
                        *pos += 1;
                        return Ok(Json::Array(items));
                    }
                    _ => return Err(format!("bad array at {pos}")),
                }
            }
        }
        Some('{') => {
            *pos += 1;
            let mut fields = Vec::new();
            skip_ws(chars, pos);
            if chars.get(*pos) == Some(&'}') {
                *pos += 1;
                return Ok(Json::Object(fields));
            }
            loop {
                skip_ws(chars, pos);
                let key = parse_string(chars, pos)?;
                skip_ws(chars, pos);
                if chars.get(*pos) != Some(&':') {
                    return Err(format!("expected ':' at {pos}"));
                }
                *pos += 1;
                let value = parse_value(chars, pos)?;
                fields.push((key, value));
                skip_ws(chars, pos);
                match chars.get(*pos) {
                    Some(',') => *pos += 1,
                    Some('}') => {
                        *pos += 1;
                        return Ok(Json::Object(fields));
                    }
                    _ => return Err(format!("bad object at {pos}")),
                }
            }
        }
        _ => Err(format!("unexpected value at {pos}")),
    }
}

fn parse_string(chars: &[char], pos: &mut usize) -> Result<String, String> {
    if chars.get(*pos) != Some(&'"') {
        return Err(format!("expected string at {pos}"));
    }
    *pos += 1;
    let mut out = String::new();
    let mut pending_high: Option<u32> = None;
    loop {
        let c = *chars.get(*pos).ok_or("unterminated string")?;
        *pos += 1;
        match c {
            '"' => return Ok(out),
            '\\' => {
                let e = *chars.get(*pos).ok_or("bad escape")?;
                *pos += 1;
                match e {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\x08'),
                    'f' => out.push('\x0c'),
                    '/' => out.push('/'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'u' => {
                        let hex: String =
                            chars.get(*pos..*pos + 4).ok_or("bad \\u")?.iter().collect();
                        *pos += 4;
                        let unit = u32::from_str_radix(&hex, 16).map_err(|e| e.to_string())?;
                        if (0xD800..0xDC00).contains(&unit) {
                            pending_high = Some(unit);
                            continue;
                        }
                        let cp = match pending_high.take() {
                            Some(high) if (0xDC00..0xE000).contains(&unit) => {
                                0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00)
                            }
                            _ => unit,
                        };
                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                    }
                    _ => return Err("bad escape".to_string()),
                }
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_matches_python_layout() {
        let value = Json::Object(vec![
            ("a".into(), Json::Array(vec![])),
            ("b".into(), Json::Object(vec![])),
            (
                "c".into(),
                Json::Array(vec![Json::Bool(true), Json::Null, Json::Int(-2)]),
            ),
        ]);
        assert_eq!(
            dumps(&value),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    true,\n    null,\n    -2\n  ]\n}"
        );
        assert_eq!(parse(&dumps(&value)).unwrap(), value);
    }

    #[test]
    fn dumps_ensure_ascii() {
        assert_eq!(
            dumps(&Json::str("é\u{1f600}\x7f\x01\"\\\n\x08\x0c")),
            "\"\\u00e9\\ud83d\\ude00\\u007f\\u0001\\\"\\\\\\n\\b\\f\""
        );
    }

    #[test]
    fn parse_roundtrip() {
        let value = Json::Object(vec![
            ("x".into(), Json::str("é\u{1f600}")),
            ("y".into(), Json::Array(vec![Json::Bool(false)])),
        ]);
        assert_eq!(parse(&dumps(&value)).unwrap(), value);
    }
}
