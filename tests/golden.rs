//! Differential tests against golden outputs captured from the Python
//! implementation (devcap 0.2.1, CPython 3.14) before the rewrite.
//!
//! `fixtures/golden_cli.json` holds argument vectors that end before any
//! scan (help, usage errors, profile errors) with the exact stdout, stderr
//! and exit status Python produced. `fixtures/golden_profiles.json` holds
//! custom-profile documents with Python's exit status and stderr.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use assert_cmd::Command;
use devcap::json::{Json, parse};

fn load(name: &str) -> Vec<Json> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    match parse(&fs::read_to_string(path).unwrap()).unwrap() {
        Json::Array(items) => items,
        other => panic!("unexpected fixture shape: {other:?}"),
    }
}

fn s(value: Option<&Json>) -> &str {
    match value {
        Some(Json::Str(v)) => v,
        other => panic!("expected string, got {other:?}"),
    }
}

fn int(value: Option<&Json>) -> i64 {
    match value {
        Some(Json::Int(v)) => *v,
        other => panic!("expected integer, got {other:?}"),
    }
}

fn rc(case: &Json) -> i32 {
    int(case.get("rc")) as i32
}

fn base64_decode(input: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for &b in input.as_bytes() {
        if b == b'=' {
            break;
        }
        let v = TABLE.iter().position(|&t| t == b).expect("base64") as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}

#[test]
fn cli_matches_python_golden_outputs() {
    let cases = load("golden_cli.json");
    assert!(cases.len() > 90);
    let mut failures = Vec::new();
    for case in &cases {
        let args: Vec<String> = match case.get("args") {
            Some(Json::Array(items)) => items.iter().map(|a| s(Some(a)).to_string()).collect(),
            other => panic!("{other:?}"),
        };
        let output = Command::cargo_bin("devcap")
            .unwrap()
            .args(&args)
            .output()
            .unwrap();
        let actual = (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        );
        let expected = (
            rc(case),
            s(case.get("stdout")).to_string(),
            s(case.get("stderr")).to_string(),
        );
        if actual != expected {
            failures.push(format!(
                "{args:?}\n  expected {expected:?}\n  actual   {actual:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn profiles_match_python_golden_outputs() {
    let cases = load("golden_profiles.json");
    assert!(cases.len() > 60);
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let dir = root.join("prof");
    fs::create_dir(&dir).unwrap();
    for case in &cases {
        let name = s(case.get("name"));
        let path = dir.join(name);
        if let Some(Json::Str(target)) = case.get("symlink") {
            symlink(target, &path).unwrap();
        } else if case.get("dir").is_some() {
            fs::create_dir(&path).unwrap();
        } else if let Some(Json::Str(byte)) = case.get("repeat_byte") {
            let count = int(case.get("repeat_count")) as usize;
            fs::write(&path, byte.repeat(count)).unwrap();
        } else {
            fs::write(&path, base64_decode(s(case.get("content_b64")))).unwrap();
        }
    }
    let mut failures = Vec::new();
    for case in &cases {
        let name = s(case.get("name"));
        let output = Command::cargo_bin("devcap")
            .unwrap()
            .current_dir(&root)
            // Keep host binaries out of reach so required tools stay missing.
            .env("PATH", "/nonexistent-devcap-path")
            .args([
                "check",
                "--config",
                &format!("prof/{name}"),
                "--format",
                "json",
                "--timeout",
                "1",
            ])
            .output()
            .unwrap();
        let actual_rc = output.status.code().unwrap_or(-1);
        let actual_err =
            String::from_utf8_lossy(&output.stderr).replace(root.to_str().unwrap(), "<DIR>");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let expected_rc = rc(case);
        let expected_err = s(case.get("stderr"));
        let mut ok = actual_rc == expected_rc && actual_err == expected_err;
        if expected_rc == 2 {
            ok &= stdout == s(case.get("stdout"));
        } else if let Some(Json::Array(keys)) = case.get("stdout_json_keys") {
            let parsed = parse(&stdout);
            ok &= match parsed {
                Ok(Json::Object(fields)) => {
                    fields.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>()
                        == keys.iter().map(|k| s(Some(k))).collect::<Vec<_>>()
                }
                _ => false,
            };
        }
        if !ok {
            failures.push(format!(
                "{name}: expected ({expected_rc}, {expected_err:?}) got ({actual_rc}, {actual_err:?})"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
