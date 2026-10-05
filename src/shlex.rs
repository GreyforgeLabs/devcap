//! Port of Python's `shlex.split(s)` (POSIX mode, no comments).

/// Split like `shlex.split`. Errors carry CPython's messages.
pub fn split(input: &str) -> Result<Vec<String>, String> {
    const WHITESPACE: &[char] = &[' ', '\t', '\r', '\n'];
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut tokens = Vec::new();
    // `state` mirrors shlex: Some(' '), Some('a'), Some(quote), Some('\\'), None (EOF).
    let mut state: Option<char> = Some(' ');
    loop {
        let mut token = String::new();
        let mut quoted = false;
        let mut escaped_state = ' ';
        loop {
            let next = chars.get(pos).copied();
            pos += 1;
            match state {
                None => {
                    token.clear();
                    break;
                }
                Some(' ') => match next {
                    None => {
                        state = None;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(&c) => {
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some('\\') => {
                        escaped_state = 'a';
                        state = Some('\\');
                    }
                    Some(c) if c == '\'' || c == '"' => state = Some(c),
                    Some(c) => {
                        token.push(c);
                        state = Some('a');
                    }
                },
                Some(q) if q == '\'' || q == '"' => {
                    quoted = true;
                    match next {
                        None => return Err("No closing quotation".to_string()),
                        Some(c) if c == q => state = Some('a'),
                        Some('\\') if q == '"' => {
                            escaped_state = q;
                            state = Some('\\');
                        }
                        Some(c) => token.push(c),
                    }
                }
                Some('\\') => {
                    let Some(c) = next else {
                        return Err("No escaped character".to_string());
                    };
                    if (escaped_state == '\'' || escaped_state == '"')
                        && c != '\\'
                        && c != escaped_state
                    {
                        token.push('\\');
                    }
                    token.push(c);
                    state = Some(escaped_state);
                }
                Some(_) => match next {
                    // state 'a'
                    None => {
                        state = None;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(&c) => {
                        state = Some(' ');
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some(c) if c == '\'' || c == '"' => state = Some(c),
                    Some('\\') => {
                        escaped_state = 'a';
                        state = Some('\\');
                    }
                    Some(c) => token.push(c),
                },
            }
        }
        if !quoted && token.is_empty() {
            if state.is_none() {
                return Ok(tokens);
            }
            continue;
        }
        tokens.push(token);
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn splits_like_python() {
        assert_eq!(
            split("version --client --short").unwrap(),
            s(&["version", "--client", "--short"])
        );
        assert_eq!(split("  -V  ").unwrap(), s(&["-V"]));
        assert_eq!(split("a 'b c' \"d e\"").unwrap(), s(&["a", "b c", "d e"]));
        assert_eq!(split("x ''").unwrap(), s(&["x", ""]));
        assert_eq!(split("a\\ b").unwrap(), s(&["a b"]));
        assert_eq!(split("\"a\\\"b\\c\"").unwrap(), s(&["a\"b\\c"]));
        assert_eq!(split("'a\\b'").unwrap(), s(&["a\\b"]));
        assert_eq!(split("a'b'c").unwrap(), s(&["abc"]));
        assert!(split("").unwrap().is_empty());
        assert!(split("   ").unwrap().is_empty());
    }

    #[test]
    fn errors_like_python() {
        assert_eq!(split("'abc").unwrap_err(), "No closing quotation");
        assert_eq!(split("abc\\").unwrap_err(), "No escaped character");
    }
}
