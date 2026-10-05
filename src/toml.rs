//! Faithful port of CPython 3.14's `tomllib` parser.
//!
//! Acceptance rules and error messages (including the
//! `(at line L, column C)` suffix) match `tomllib.loads`, so custom-profile
//! errors read exactly as they did in the Python implementation. Scalar
//! values other than strings and booleans are kept only as type tags because
//! devcap's schema never reads them.

use std::collections::HashMap;

use crate::pycompat::repr;

/// A parsed TOML table.
pub type Table = HashMap<String, Value>;

/// A parsed TOML value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Bool(bool),
    Integer,
    Float,
    Datetime,
    Date,
    Time,
    Array(Vec<Value>),
    Table(Table),
}

/// Parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TomlError {
    /// `tomllib.TOMLDecodeError` with its formatted message.
    Decode(String),
    /// Where CPython raises `RecursionError` (not a `ValueError`).
    Recursion(String),
}

impl std::fmt::Display for TomlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TomlError::Decode(msg) | TomlError::Recursion(msg) => f.write_str(msg),
        }
    }
}

type Key = Vec<String>;
type PResult<T> = Result<T, TomlError>;

/// `sys.getrecursionlimit()` default, used for key parts and nesting.
const MAX_KEY_PARTS: usize = 1000;
const MAX_FRAMES: usize = 1000;

const FROZEN: u8 = 1;
const EXPLICIT_NEST: u8 = 2;

fn is_bare_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

fn is_ascii_ctrl(c: char) -> bool {
    (c as u32) < 32 || c == '\x7f'
}

fn illegal_basic(c: char) -> bool {
    is_ascii_ctrl(c) && c != '\t'
}

fn illegal_multiline(c: char) -> bool {
    is_ascii_ctrl(c) && c != '\t' && c != '\n'
}

fn key_repr(key: &[String]) -> String {
    match key.len() {
        1 => format!("({},)", repr(&key[0])),
        _ => format!(
            "({})",
            key.iter().map(|k| repr(k)).collect::<Vec<_>>().join(", ")
        ),
    }
}

#[derive(Default, Debug)]
struct FlagNode {
    flags: u8,
    recursive_flags: u8,
    nested: HashMap<String, FlagNode>,
}

#[derive(Default, Debug)]
struct Flags {
    root: HashMap<String, FlagNode>,
    pending: Vec<(Key, u8)>,
}

impl Flags {
    fn add_pending(&mut self, key: Key, flag: u8) {
        self.pending.push((key, flag));
    }

    fn finalize_pending(&mut self) {
        for (key, flag) in std::mem::take(&mut self.pending) {
            self.set(&key, flag, false);
        }
    }

    fn unset_all(&mut self, key: &[String]) {
        let mut cont = &mut self.root;
        for k in &key[..key.len() - 1] {
            match cont.get_mut(k) {
                Some(node) => cont = &mut node.nested,
                None => return,
            }
        }
        cont.remove(&key[key.len() - 1]);
    }

    fn set(&mut self, key: &[String], flag: u8, recursive: bool) {
        let mut cont = &mut self.root;
        for k in &key[..key.len() - 1] {
            cont = &mut cont.entry(k.clone()).or_default().nested;
        }
        let node = cont.entry(key[key.len() - 1].clone()).or_default();
        if recursive {
            node.recursive_flags |= flag;
        } else {
            node.flags |= flag;
        }
    }

    fn is(&self, key: &[String], flag: u8) -> bool {
        if key.is_empty() {
            return false;
        }
        let mut cont = &self.root;
        for k in &key[..key.len() - 1] {
            let Some(inner) = cont.get(k) else {
                return false;
            };
            if inner.recursive_flags & flag != 0 {
                return true;
            }
            cont = &inner.nested;
        }
        match cont.get(&key[key.len() - 1]) {
            Some(node) => (node.flags | node.recursive_flags) & flag != 0,
            None => false,
        }
    }
}

/// Marker for Python's `KeyError` inside `NestedDict`.
struct NoNest;

fn get_or_create_nest<'a>(
    root: &'a mut Table,
    key: &[String],
    access_lists: bool,
) -> Result<&'a mut Table, NoNest> {
    let mut cont = root;
    for k in key {
        let entry = cont
            .entry(k.clone())
            .or_insert_with(|| Value::Table(Table::new()));
        let target = if access_lists && matches!(entry, Value::Array(_)) {
            match entry {
                Value::Array(items) => items.last_mut().ok_or(NoNest)?,
                _ => return Err(NoNest),
            }
        } else {
            entry
        };
        match target {
            Value::Table(table) => cont = table,
            _ => return Err(NoNest),
        }
    }
    Ok(cont)
}

fn append_nest_to_list(root: &mut Table, key: &[String]) -> Result<(), NoNest> {
    let cont = get_or_create_nest(root, &key[..key.len() - 1], true)?;
    let last = &key[key.len() - 1];
    match cont.get_mut(last) {
        Some(Value::Array(items)) => items.push(Value::Table(Table::new())),
        Some(_) => return Err(NoNest),
        None => {
            cont.insert(last.clone(), Value::Array(vec![Value::Table(Table::new())]));
        }
    }
    Ok(())
}

struct Parser {
    src: Vec<char>,
    frames: usize,
}

impl Parser {
    fn err<T>(&self, msg: impl Into<String>, pos: usize) -> PResult<T> {
        Err(TomlError::Decode(self.format_error(&msg.into(), pos)))
    }

    fn format_error(&self, msg: &str, pos: usize) -> String {
        let doc = &self.src;
        let end = pos.min(doc.len());
        let lineno = doc[..end].iter().filter(|&&c| c == '\n').count() + 1;
        let colno = if lineno == 1 {
            pos + 1
        } else {
            let last_nl = doc[..end].iter().rposition(|&c| c == '\n').unwrap_or(0);
            pos - last_nl
        };
        if pos >= doc.len() {
            format!("{msg} (at end of document)")
        } else {
            format!("{msg} (at line {lineno}, column {colno})")
        }
    }

    fn char_at(&self, pos: usize) -> Option<char> {
        self.src.get(pos).copied()
    }

    fn starts_with(&self, pos: usize, s: &str) -> bool {
        s.chars()
            .enumerate()
            .all(|(i, c)| self.char_at(pos + i) == Some(c))
    }

    fn skip_ws(&self, mut pos: usize) -> usize {
        while matches!(self.char_at(pos), Some(' ' | '\t')) {
            pos += 1;
        }
        pos
    }

    fn skip_ws_and_newline(&self, mut pos: usize) -> usize {
        while matches!(self.char_at(pos), Some(' ' | '\t' | '\n')) {
            pos += 1;
        }
        pos
    }

    fn find(&self, pos: usize, expect: &str) -> Option<usize> {
        let needle: Vec<char> = expect.chars().collect();
        if pos > self.src.len() {
            return None;
        }
        self.src[pos..]
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .map(|i| i + pos)
    }

    fn skip_until(
        &self,
        pos: usize,
        expect: &str,
        error_on: fn(char) -> bool,
        error_on_eof: bool,
    ) -> PResult<usize> {
        let new_pos = match self.find(pos, expect) {
            Some(p) => p,
            None => {
                let end = self.src.len();
                if error_on_eof {
                    return self.err(format!("Expected {}", repr(expect)), end);
                }
                end
            }
        };
        if pos < new_pos
            && let Some(offset) = self.src[pos..new_pos].iter().position(|&c| error_on(c))
        {
            let bad = pos + offset;
            return self.err(
                format!(
                    "Found invalid character {}",
                    repr(&self.src[bad].to_string())
                ),
                bad,
            );
        }
        Ok(new_pos)
    }

    fn skip_comment(&self, pos: usize) -> PResult<usize> {
        if self.char_at(pos) == Some('#') {
            return self.skip_until(pos + 1, "\n", illegal_basic, false);
        }
        Ok(pos)
    }

    fn skip_comments_and_array_ws(&self, mut pos: usize) -> PResult<usize> {
        loop {
            let before = pos;
            pos = self.skip_ws_and_newline(pos);
            pos = self.skip_comment(pos)?;
            if pos == before {
                return Ok(pos);
            }
        }
    }

    fn enter(&mut self) -> PResult<()> {
        self.frames += 1;
        if self.frames > MAX_FRAMES {
            return Err(TomlError::Recursion(
                "maximum recursion depth exceeded while parsing TOML".to_string(),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.frames -= 1;
    }

    fn parse_document(&mut self) -> PResult<Table> {
        let mut data = Table::new();
        let mut flags = Flags::default();
        let mut header: Key = Vec::new();
        let mut pos = 0;
        loop {
            pos = self.skip_ws(pos);
            let Some(c) = self.char_at(pos) else {
                break;
            };
            if c == '\n' {
                pos += 1;
                continue;
            }
            if is_bare_key_char(c) || c == '"' || c == '\'' {
                pos = self.key_value_rule(pos, &mut data, &mut flags, &header)?;
                pos = self.skip_ws(pos);
            } else if c == '[' {
                flags.finalize_pending();
                let (new_pos, key) = if self.char_at(pos + 1) == Some('[') {
                    self.create_list_rule(pos, &mut data, &mut flags)?
                } else {
                    self.create_dict_rule(pos, &mut data, &mut flags)?
                };
                header = key;
                pos = self.skip_ws(new_pos);
            } else if c != '#' {
                return self.err("Invalid statement", pos);
            }
            pos = self.skip_comment(pos)?;
            match self.char_at(pos) {
                None => break,
                Some('\n') => pos += 1,
                Some(_) => {
                    return self.err("Expected newline or end of document after a statement", pos);
                }
            }
        }
        Ok(data)
    }

    fn create_dict_rule(
        &mut self,
        pos: usize,
        data: &mut Table,
        flags: &mut Flags,
    ) -> PResult<(usize, Key)> {
        let pos = self.skip_ws(pos + 1);
        let (pos, key) = self.parse_key(pos)?;
        if flags.is(&key, EXPLICIT_NEST) || flags.is(&key, FROZEN) {
            return self.err(format!("Cannot declare {} twice", key_repr(&key)), pos);
        }
        flags.set(&key, EXPLICIT_NEST, false);
        if get_or_create_nest(data, &key, true).is_err() {
            return self.err("Cannot overwrite a value", pos);
        }
        if !self.starts_with(pos, "]") {
            return self.err("Expected ']' at the end of a table declaration", pos);
        }
        Ok((pos + 1, key))
    }

    fn create_list_rule(
        &mut self,
        pos: usize,
        data: &mut Table,
        flags: &mut Flags,
    ) -> PResult<(usize, Key)> {
        let pos = self.skip_ws(pos + 2);
        let (pos, key) = self.parse_key(pos)?;
        if flags.is(&key, FROZEN) {
            return self.err(
                format!("Cannot mutate immutable namespace {}", key_repr(&key)),
                pos,
            );
        }
        flags.unset_all(&key);
        flags.set(&key, EXPLICIT_NEST, false);
        if append_nest_to_list(data, &key).is_err() {
            return self.err("Cannot overwrite a value", pos);
        }
        if !self.starts_with(pos, "]]") {
            return self.err("Expected ']]' at the end of an array declaration", pos);
        }
        Ok((pos + 2, key))
    }

    fn key_value_rule(
        &mut self,
        pos: usize,
        data: &mut Table,
        flags: &mut Flags,
        header: &[String],
    ) -> PResult<usize> {
        let (pos, key, value) = self.parse_key_value_pair(pos)?;
        let (key_parent, key_stem) = key.split_at(key.len() - 1);
        let key_stem = &key_stem[0];
        let abs_key_parent: Key = header.iter().chain(key_parent).cloned().collect();

        for i in 1..key.len() {
            let cont_key: Key = header.iter().chain(&key[..i]).cloned().collect();
            if flags.is(&cont_key, EXPLICIT_NEST) {
                return self.err(
                    format!("Cannot redefine namespace {}", key_repr(&cont_key)),
                    pos,
                );
            }
            flags.add_pending(cont_key, EXPLICIT_NEST);
        }

        if flags.is(&abs_key_parent, FROZEN) {
            return self.err(
                format!(
                    "Cannot mutate immutable namespace {}",
                    key_repr(&abs_key_parent)
                ),
                pos,
            );
        }

        let Ok(nest) = get_or_create_nest(data, &abs_key_parent, true) else {
            return self.err("Cannot overwrite a value", pos);
        };
        if nest.contains_key(key_stem) {
            return self.err("Cannot overwrite a value", pos);
        }
        if matches!(value, Value::Table(_) | Value::Array(_)) {
            let full: Key = header.iter().chain(&key).cloned().collect();
            flags.set(&full, FROZEN, true);
        }
        nest.insert(key_stem.clone(), value);
        Ok(pos)
    }

    fn parse_key_value_pair(&mut self, pos: usize) -> PResult<(usize, Key, Value)> {
        self.enter()?;
        let (pos, key) = self.parse_key(pos)?;
        if self.char_at(pos) != Some('=') {
            return self.err("Expected '=' after a key in a key/value pair", pos);
        }
        let pos = self.skip_ws(pos + 1);
        let (pos, value) = self.parse_value(pos)?;
        self.leave();
        Ok((pos, key, value))
    }

    fn parse_key(&self, pos: usize) -> PResult<(usize, Key)> {
        let (pos, part) = self.parse_key_part(pos)?;
        let mut key = vec![part];
        let mut pos = self.skip_ws(pos);
        loop {
            if self.char_at(pos) != Some('.') {
                return Ok((pos, key));
            }
            pos = self.skip_ws(pos + 1);
            let (new_pos, part) = self.parse_key_part(pos)?;
            key.push(part);
            if key.len() > MAX_KEY_PARTS {
                return Err(TomlError::Recursion(format!(
                    "TOML key has more than the allowed {MAX_KEY_PARTS} parts"
                )));
            }
            pos = self.skip_ws(new_pos);
        }
    }

    fn parse_key_part(&self, pos: usize) -> PResult<(usize, String)> {
        match self.char_at(pos) {
            Some(c) if is_bare_key_char(c) => {
                let mut end = pos;
                while self.char_at(end).is_some_and(is_bare_key_char) {
                    end += 1;
                }
                Ok((end, self.src[pos..end].iter().collect()))
            }
            Some('\'') => self.parse_literal_str(pos),
            Some('"') => self.parse_basic_str(pos + 1, false),
            _ => self.err("Invalid initial character for a key part", pos),
        }
    }

    fn parse_array(&mut self, pos: usize) -> PResult<(usize, Value)> {
        self.enter()?;
        let mut array = Vec::new();
        let mut pos = self.skip_comments_and_array_ws(pos + 1)?;
        if self.starts_with(pos, "]") {
            self.leave();
            return Ok((pos + 1, Value::Array(array)));
        }
        loop {
            let (new_pos, value) = self.parse_value(pos)?;
            array.push(value);
            pos = self.skip_comments_and_array_ws(new_pos)?;
            match self.char_at(pos) {
                Some(']') => {
                    self.leave();
                    return Ok((pos + 1, Value::Array(array)));
                }
                Some(',') => {}
                _ => return self.err("Unclosed array", pos),
            }
            pos = self.skip_comments_and_array_ws(pos + 1)?;
            if self.starts_with(pos, "]") {
                self.leave();
                return Ok((pos + 1, Value::Array(array)));
            }
        }
    }

    fn parse_inline_table(&mut self, pos: usize) -> PResult<(usize, Value)> {
        self.enter()?;
        let mut table = Table::new();
        let mut flags = Flags::default();
        let mut pos = self.skip_ws(pos + 1);
        if self.starts_with(pos, "}") {
            self.leave();
            return Ok((pos + 1, Value::Table(table)));
        }
        loop {
            let (new_pos, key, value) = self.parse_key_value_pair(pos)?;
            pos = new_pos;
            let (key_parent, key_stem) = key.split_at(key.len() - 1);
            let key_stem = &key_stem[0];
            if flags.is(&key, FROZEN) {
                return self.err(
                    format!("Cannot mutate immutable namespace {}", key_repr(&key)),
                    pos,
                );
            }
            let Ok(nest) = get_or_create_nest(&mut table, key_parent, false) else {
                return self.err("Cannot overwrite a value", pos);
            };
            if nest.contains_key(key_stem) {
                return self.err(
                    format!("Duplicate inline table key {}", repr(key_stem)),
                    pos,
                );
            }
            let is_container = matches!(value, Value::Table(_) | Value::Array(_));
            nest.insert(key_stem.clone(), value);
            pos = self.skip_ws(pos);
            match self.char_at(pos) {
                Some('}') => {
                    self.leave();
                    return Ok((pos + 1, Value::Table(table)));
                }
                Some(',') => {}
                _ => return self.err("Unclosed inline table", pos),
            }
            if is_container {
                flags.set(&key, FROZEN, true);
            }
            pos = self.skip_ws(pos + 1);
        }
    }

    fn parse_basic_str_escape(&self, pos: usize, multiline: bool) -> PResult<(usize, String)> {
        let id: String = self.src[pos.min(self.src.len())..(pos + 2).min(self.src.len())]
            .iter()
            .collect();
        let mut pos = pos + 2;
        if multiline && (id == "\\ " || id == "\\\t" || id == "\\\n") {
            if id != "\\\n" {
                pos = self.skip_ws(pos);
                match self.char_at(pos) {
                    None => return Ok((pos, String::new())),
                    Some('\n') => pos += 1,
                    Some(_) => return self.err("Unescaped '\\' in a string", pos),
                }
            }
            pos = self.skip_ws_and_newline(pos);
            return Ok((pos, String::new()));
        }
        let replacement = match id.as_str() {
            "\\u" => return self.parse_hex_char(pos, 4),
            "\\U" => return self.parse_hex_char(pos, 8),
            "\\b" => '\u{8}',
            "\\t" => '\t',
            "\\n" => '\n',
            "\\f" => '\u{c}',
            "\\r" => '\r',
            "\\\"" => '"',
            "\\\\" => '\\',
            _ => return self.err("Unescaped '\\' in a string", pos),
        };
        Ok((pos, replacement.to_string()))
    }

    fn parse_hex_char(&self, pos: usize, len: usize) -> PResult<(usize, String)> {
        let start = pos.min(self.src.len());
        let end = (pos + len).min(self.src.len());
        let hex: String = self.src[start..end].iter().collect();
        if hex.chars().count() != len || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return self.err("Invalid hex value", pos);
        }
        let pos = pos + len;
        let value = u32::from_str_radix(&hex, 16).unwrap_or(u32::MAX);
        match char::from_u32(value) {
            Some(c) => Ok((pos, c.to_string())),
            None => self.err("Escaped character is not a Unicode scalar value", pos),
        }
    }

    fn parse_literal_str(&self, pos: usize) -> PResult<(usize, String)> {
        let start = pos + 1;
        let end = self.skip_until(start, "'", illegal_basic, true)?;
        Ok((end + 1, self.src[start..end].iter().collect()))
    }

    fn parse_multiline_str(&self, pos: usize, literal: bool) -> PResult<(usize, Value)> {
        let mut pos = pos + 3;
        if self.starts_with(pos, "\n") {
            pos += 1;
        }
        let (delim, mut pos, mut result) = if literal {
            let end = self.skip_until(pos, "'''", illegal_multiline, true)?;
            ('\'', end + 3, self.src[pos..end].iter().collect::<String>())
        } else {
            let (p, r) = self.parse_basic_str(pos, true)?;
            ('"', p, r)
        };
        for _ in 0..2 {
            if self.char_at(pos) != Some(delim) {
                break;
            }
            pos += 1;
            result.push(delim);
        }
        Ok((pos, Value::Str(result)))
    }

    fn parse_basic_str(&self, pos: usize, multiline: bool) -> PResult<(usize, String)> {
        let error_on: fn(char) -> bool = if multiline {
            illegal_multiline
        } else {
            illegal_basic
        };
        let mut result = String::new();
        let mut pos = pos;
        let mut start = pos;
        loop {
            let Some(c) = self.char_at(pos) else {
                return self.err("Unterminated string", pos);
            };
            if c == '"' {
                if !multiline {
                    result.extend(&self.src[start..pos]);
                    return Ok((pos + 1, result));
                }
                if self.starts_with(pos, "\"\"\"") {
                    result.extend(&self.src[start..pos]);
                    return Ok((pos + 3, result));
                }
                pos += 1;
                continue;
            }
            if c == '\\' {
                result.extend(&self.src[start..pos]);
                let (new_pos, escaped) = self.parse_basic_str_escape(pos, multiline)?;
                result.push_str(&escaped);
                pos = new_pos;
                start = pos;
                continue;
            }
            if error_on(c) {
                return self.err(format!("Illegal character {}", repr(&c.to_string())), pos);
            }
            pos += 1;
        }
    }

    fn parse_value(&mut self, pos: usize) -> PResult<(usize, Value)> {
        self.enter()?;
        let result = self.parse_value_inner(pos);
        if result.is_ok() {
            self.leave();
        }
        result
    }

    fn parse_value_inner(&mut self, pos: usize) -> PResult<(usize, Value)> {
        let c = self.char_at(pos);
        match c {
            Some('"') => {
                if self.starts_with(pos, "\"\"\"") {
                    return self.parse_multiline_str(pos, false);
                }
                let (p, s) = self.parse_basic_str(pos + 1, false)?;
                return Ok((p, Value::Str(s)));
            }
            Some('\'') => {
                if self.starts_with(pos, "'''") {
                    return self.parse_multiline_str(pos, true);
                }
                let (p, s) = self.parse_literal_str(pos)?;
                return Ok((p, Value::Str(s)));
            }
            Some('t') if self.starts_with(pos, "true") => return Ok((pos + 4, Value::Bool(true))),
            Some('f') if self.starts_with(pos, "false") => {
                return Ok((pos + 5, Value::Bool(false)));
            }
            Some('[') => return self.parse_array(pos),
            Some('{') => return self.parse_inline_table(pos),
            _ => {}
        }
        if let Some((end, valid, has_time)) = self.match_datetime(pos) {
            if !valid {
                return self.err("Invalid date or datetime", pos);
            }
            return Ok((
                end,
                if has_time {
                    Value::Datetime
                } else {
                    Value::Date
                },
            ));
        }
        if let Some(end) = self.match_time(pos) {
            return Ok((end, Value::Time));
        }
        if let Some((end, is_float)) = self.match_number(pos) {
            // `int(text, 0)` enforces CPython's default 4300-digit limit for
            // decimal strings (prefixed bases are powers of two and exempt).
            let token = &self.src[pos..end];
            let prefixed =
                token.len() > 1 && token[0] == '0' && matches!(token[1], 'x' | 'b' | 'o');
            if !is_float && !prefixed {
                let digits = token.iter().filter(|c| c.is_ascii_digit()).count();
                if digits > crate::pycompat::INT_MAX_STR_DIGITS {
                    return Err(TomlError::Decode(crate::pycompat::int_limit_message(
                        digits,
                    )));
                }
            }
            return Ok((
                end,
                if is_float {
                    Value::Float
                } else {
                    Value::Integer
                },
            ));
        }
        let first_three: String = self.src[pos.min(self.src.len())..(pos + 3).min(self.src.len())]
            .iter()
            .collect();
        if first_three == "inf" || first_three == "nan" {
            return Ok((pos + 3, Value::Float));
        }
        let first_four: String = self.src[pos.min(self.src.len())..(pos + 4).min(self.src.len())]
            .iter()
            .collect();
        if matches!(first_four.as_str(), "-inf" | "+inf" | "-nan" | "+nan") {
            return Ok((pos + 4, Value::Float));
        }
        self.err("Invalid value", pos)
    }

    fn digit_at(&self, pos: usize) -> Option<u32> {
        self.char_at(pos)
            .filter(|c| c.is_ascii_digit())
            .map(|c| c as u32 - '0' as u32)
    }

    fn two_digits(&self, pos: usize) -> Option<u32> {
        Some(self.digit_at(pos)? * 10 + self.digit_at(pos + 1)?)
    }

    /// `_TIME_RE_STR` at `pos`; returns the end position.
    fn match_time(&self, pos: usize) -> Option<usize> {
        let hour = self.two_digits(pos)?;
        if hour > 23 || self.char_at(pos + 2) != Some(':') {
            return None;
        }
        let minute = self.two_digits(pos + 3)?;
        if minute > 59 || self.char_at(pos + 5) != Some(':') {
            return None;
        }
        let second = self.two_digits(pos + 6)?;
        if second > 59 {
            return None;
        }
        let mut end = pos + 8;
        if self.char_at(end) == Some('.') && self.digit_at(end + 1).is_some() {
            end += 1;
            while self.digit_at(end).is_some() {
                end += 1;
            }
        }
        Some(end)
    }

    /// `RE_DATETIME` at `pos`: (end, date-is-valid, has-time).
    fn match_datetime(&self, pos: usize) -> Option<(usize, bool, bool)> {
        let mut year = 0;
        for i in 0..4 {
            year = year * 10 + self.digit_at(pos + i)?;
        }
        if self.char_at(pos + 4) != Some('-') {
            return None;
        }
        let month = self.two_digits(pos + 5)?;
        if !(1..=12).contains(&month) || self.char_at(pos + 7) != Some('-') {
            return None;
        }
        let day = self.two_digits(pos + 8)?;
        if !(1..=31).contains(&day) {
            return None;
        }
        let mut end = pos + 10;
        let mut has_time = false;
        if matches!(self.char_at(end), Some('T' | 't' | ' '))
            && let Some(time_end) = self.match_time(end + 1)
        {
            has_time = true;
            end = time_end;
            match self.char_at(end) {
                Some('Z' | 'z') => end += 1,
                Some('+' | '-') => {
                    if let Some(h) = self.two_digits(end + 1)
                        && h <= 23
                        && self.char_at(end + 3) == Some(':')
                        && self.two_digits(end + 4).is_some_and(|m| m <= 59)
                    {
                        end += 6;
                    }
                }
                _ => {}
            }
        }
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days_in_month = match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        let valid = year >= 1 && day <= days_in_month;
        Some((end, valid, has_time))
    }

    /// Digits matching `D(?:_?D)*` starting at `pos` (first must match).
    fn match_digit_run(&self, pos: usize, is_digit: fn(char) -> bool) -> Option<usize> {
        if !self.char_at(pos).is_some_and(is_digit) {
            return None;
        }
        let mut end = pos + 1;
        loop {
            if self.char_at(end).is_some_and(is_digit) {
                end += 1;
            } else if self.char_at(end) == Some('_') && self.char_at(end + 1).is_some_and(is_digit)
            {
                end += 2;
            } else {
                return Some(end);
            }
        }
    }

    /// `RE_NUMBER` at `pos`: (end, has-float-part).
    fn match_number(&self, pos: usize) -> Option<(usize, bool)> {
        if self.char_at(pos) == Some('0') {
            let prefixed = match self.char_at(pos + 1) {
                Some('x') => self.match_digit_run(pos + 2, |c| c.is_ascii_hexdigit()),
                Some('b') => self.match_digit_run(pos + 2, |c| c == '0' || c == '1'),
                Some('o') => self.match_digit_run(pos + 2, |c| ('0'..='7').contains(&c)),
                _ => None,
            };
            if let Some(end) = prefixed {
                return Some((end, false));
            }
        }
        let mut p = pos;
        if matches!(self.char_at(p), Some('+' | '-')) {
            p += 1;
        }
        let int_end = match self.char_at(p) {
            Some('0') => p + 1,
            Some(c) if c.is_ascii_digit() => self.match_digit_run(p, |c| c.is_ascii_digit())?,
            _ => return None,
        };
        let mut end = int_end;
        if self.char_at(end) == Some('.')
            && let Some(frac_end) = self.match_digit_run(end + 1, |c| c.is_ascii_digit())
        {
            end = frac_end;
        }
        if matches!(self.char_at(end), Some('e' | 'E')) {
            let mut q = end + 1;
            if matches!(self.char_at(q), Some('+' | '-')) {
                q += 1;
            }
            if let Some(exp_end) = self.match_digit_run(q, |c| c.is_ascii_digit()) {
                end = exp_end;
            }
        }
        Some((end, end > int_end))
    }
}

/// Parse a TOML document like `tomllib.loads`.
pub fn loads(text: &str) -> Result<Table, TomlError> {
    let src: Vec<char> = text.replace("\r\n", "\n").chars().collect();
    let mut parser = Parser { src, frames: 0 };
    parser.parse_document()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(doc: &str) -> String {
        loads(doc).unwrap_err().to_string()
    }

    #[test]
    fn decimal_integers_respect_cpython_digit_limit() {
        assert!(loads(&format!("x = {}\n", "1".repeat(4300))).is_ok());
        assert_eq!(
            err(&format!("x = {}\n", "1".repeat(4301))),
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 \
             digits; use sys.set_int_max_str_digits() to increase the limit"
        );
        assert!(loads(&format!("x = {}1\n", "1_".repeat(4300))).is_err());
        // Power-of-two bases and floats are exempt.
        assert!(loads(&format!("x = 0x{}\n", "f".repeat(5000))).is_ok());
        assert!(loads(&format!("x = {}.5\n", "1".repeat(5000))).is_ok());
    }

    #[test]
    fn parses_profiles() {
        let table = loads(
            "[profile]\nname = \"x\" # c\n[[tools]]\nname = 'a'\nrequired = true\n[[tools]]\nname=\"b\"\n[services]\nsystem = [\"sshd\",]\n",
        )
        .unwrap();
        let Some(Value::Array(tools)) = table.get("tools") else {
            panic!("tools")
        };
        assert_eq!(tools.len(), 2);
        let Some(Value::Table(svc)) = table.get("services") else {
            panic!("services")
        };
        assert_eq!(
            svc.get("system"),
            Some(&Value::Array(vec![Value::Str("sshd".into())]))
        );
    }

    #[test]
    fn scalar_types() {
        let t = loads(
            "a=1\nb=1.5\nc=1979-05-27\nd=1979-05-27T07:32:00Z\ne=07:32:00\nf=inf\ng=0x1F\nh=1e3\ni=\"\"\"x\"\"\"\"\nj='''y'''''\nk=\"\\u00e9\"",
        )
        .unwrap();
        assert_eq!(t["a"], Value::Integer);
        assert_eq!(t["b"], Value::Float);
        assert_eq!(t["c"], Value::Date);
        assert_eq!(t["d"], Value::Datetime);
        assert_eq!(t["e"], Value::Time);
        assert_eq!(t["f"], Value::Float);
        assert_eq!(t["g"], Value::Integer);
        assert_eq!(t["h"], Value::Float);
        assert_eq!(t["i"], Value::Str("x\"".into()));
        assert_eq!(t["j"], Value::Str("y''".into()));
        assert_eq!(t["k"], Value::Str("é".into()));
    }

    #[test]
    fn error_messages_match_tomllib() {
        assert_eq!(
            err("a = []\n[a.b]"),
            "Cannot declare ('a', 'b') twice (at line 2, column 5)"
        );
        assert_eq!(
            err("a=[1]\n[[a]]"),
            "Cannot mutate immutable namespace ('a',) (at line 2, column 4)"
        );
        assert_eq!(
            err("x = 0x"),
            "Expected newline or end of document after a statement (at line 1, column 6)"
        );
        assert_eq!(
            err("a.b=1\n[a]"),
            "Cannot declare ('a',) twice (at line 2, column 3)"
        );
        assert_eq!(
            err("[a]\nb=1\n[a]"),
            "Cannot declare ('a',) twice (at line 3, column 3)"
        );
        assert_eq!(
            err("a={b=1}\na.c=2"),
            "Cannot mutate immutable namespace ('a',) (at end of document)"
        );
        assert!(loads("a='''x''''").is_ok());
        assert_eq!(
            err("a=\"\\"),
            "Unescaped '\\' in a string (at end of document)"
        );
        assert_eq!(
            err("a=\"\\x\""),
            "Unescaped '\\' in a string (at line 1, column 6)"
        );
        assert_eq!(
            err("a = \"\\u12\""),
            "Invalid hex value (at line 1, column 8)"
        );
        assert_eq!(
            err("a=1979-02-30"),
            "Invalid date or datetime (at line 1, column 3)"
        );
        assert_eq!(
            err("a=0000-01-01"),
            "Invalid date or datetime (at line 1, column 3)"
        );
        assert_eq!(
            err("a=trueish"),
            "Expected newline or end of document after a statement (at line 1, column 7)"
        );
        assert_eq!(
            err("a=\"x\n"),
            "Illegal character '\\n' (at line 1, column 5)"
        );
        assert_eq!(err("a='x"), "Expected \"'\" (at end of document)");
        assert_eq!(err("a='''x"), "Expected \"'''\" (at end of document)");
        assert_eq!(
            err("\u{feff}a=1"),
            "Invalid statement (at line 1, column 1)"
        );
        assert_eq!(err("a=1\r\nb="), "Invalid value (at end of document)");
        assert_eq!(err("a=+"), "Invalid value (at line 1, column 3)");
        assert_eq!(err("a=[1,2"), "Unclosed array (at end of document)");
        assert_eq!(err("a={b=1"), "Unclosed inline table (at end of document)");
        assert_eq!(
            err("[[a]]\n[a]"),
            "Cannot declare ('a',) twice (at line 2, column 3)"
        );
        assert_eq!(
            err("a.b.c=1\na.b={}"),
            "Cannot overwrite a value (at end of document)"
        );
        assert_eq!(
            err("a={b={c=1}, b.d=2}"),
            "Cannot mutate immutable namespace ('b', 'd') (at line 1, column 18)"
        );
        assert_eq!(
            err("x=1 # \x01"),
            "Found invalid character '\\x01' (at line 1, column 7)"
        );
        assert_eq!(
            err("tools = \"python3\"\nx"),
            "Expected '=' after a key in a key/value pair (at end of document)"
        );
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let ok = format!("a={}{}", "[".repeat(400), "]".repeat(400));
        assert!(loads(&ok).is_ok());
        let deep = format!("a={}{}", "[".repeat(5000), "]".repeat(5000));
        assert!(matches!(loads(&deep), Err(TomlError::Recursion(_))));
        let long_key = format!("{}=1", vec!["k"; 1001].join("."));
        assert!(matches!(loads(&long_key), Err(TomlError::Recursion(_))));
    }
}
