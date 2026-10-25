//! Integration tests for the CLI (port of the Python `tests/test_cli.py`).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use assert_cmd::Command;
use devcap::json::{Json, parse};
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("devcap").expect("binary should build")
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_devcap(args: &[&str], path_prefix: Option<&Path>) -> Output {
    let mut cmd = bin();
    cmd.args(args).timeout(std::time::Duration::from_secs(60));
    if let Some(prefix) = path_prefix {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{path}", prefix.display()));
    }
    let output = cmd.output().expect("devcap runs");
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn tools(data: &Json) -> &[Json] {
    match data.get("tools") {
        Some(Json::Array(items)) => items,
        other => panic!("tools missing: {other:?}"),
    }
}

#[test]
fn test_no_args_shows_help() {
    let result = run_devcap(&[], None);
    assert_eq!(result.code, 0);
    assert!(result.stdout.to_lowercase().contains("devcap"));
}

#[test]
fn test_scan_text() {
    let result = run_devcap(&["scan", "--profile", "python-dev"], None);
    assert_eq!(result.code, 0);
    assert!(result.stdout.contains("python3"));
}

#[test]
fn test_scan_json() {
    let result = run_devcap(
        &["scan", "--profile", "python-dev", "--format", "json"],
        None,
    );
    assert_eq!(result.code, 0);
    let data = parse(&result.stdout).expect("valid JSON");
    assert!(data.get("hostname").is_some());
    assert!(data.get("tools").is_some());
}

#[test]
fn test_scan_markdown() {
    let result = run_devcap(
        &["scan", "--profile", "python-dev", "--format", "markdown"],
        None,
    );
    assert_eq!(result.code, 0);
    assert!(result.stdout.contains("##"));
}

#[test]
fn test_check_custom_profile_uses_controlled_binary() {
    let tmp = TempDir::new().unwrap();
    let executable = tmp.path().join("fixture-tool");
    fs::write(&executable, "#!/bin/sh\nprintf 'fixture-tool 1.2.3\\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let profile = tmp.path().join("fixture.toml");
    fs::write(
        &profile,
        "[profile]\nname = \"fixture\"\n[[tools]]\nname = \"fixture-tool\"\nbinary = \"fixture-tool\"\ncategory = \"Testing\"\nrequired = true\n",
    )
    .unwrap();
    let profile_arg = profile.to_str().unwrap();

    let present = run_devcap(&["check", "--config", profile_arg], Some(tmp.path()));
    assert_eq!(present.code, 0, "{}", present.stderr);
    assert!(present.stdout.contains("fixture-tool     1.2.3"));

    fs::remove_file(&executable).unwrap();
    let missing = run_devcap(&["check", "--config", profile_arg], Some(tmp.path()));
    assert_eq!(missing.code, 1);
    assert_eq!(missing.stderr, "\nMissing required tools: fixture-tool\n");
}

#[test]
fn test_list_profiles() {
    let result = run_devcap(&["list-profiles"], None);
    assert_eq!(result.code, 0);
    assert!(result.stdout.contains("full"));
    assert!(result.stdout.contains("python-dev"));
}

#[test]
fn test_unknown_profile() {
    let result = run_devcap(&["scan", "--profile", "nonexistent"], None);
    assert_eq!(result.code, 2);
}

#[test]
fn test_scan_no_parallel() {
    let result = run_devcap(
        &[
            "scan",
            "--profile",
            "python-dev",
            "--no-parallel",
            "--format",
            "json",
        ],
        None,
    );
    assert_eq!(result.code, 0);
    let data = parse(&result.stdout).unwrap();
    assert!(!tools(&data).is_empty());
}

#[test]
fn test_scan_redact_json() {
    let result = run_devcap(
        &[
            "scan",
            "--profile",
            "python-dev",
            "--format",
            "json",
            "--redact",
        ],
        None,
    );
    assert_eq!(result.code, 0);
    let data = parse(&result.stdout).unwrap();
    assert_eq!(data.get("hostname"), Some(&Json::str("[redacted]")));
    for tool in tools(&data) {
        if tool.get("found") == Some(&Json::Bool(true)) {
            assert_eq!(tool.get("path"), Some(&Json::str("[redacted]")));
        }
    }
}

#[test]
fn test_rejects_unsafe_custom_profile() {
    let tmp = TempDir::new().unwrap();
    let profile = tmp.path().join("unsafe.toml");
    fs::write(
        &profile,
        "\n        [[tools]]\n        name = \"owned\"\n        binary = \"sh\"\n        version_flag = \"-c id\"\n        ",
    )
    .unwrap();
    let result = run_devcap(&["scan", "--config", profile.to_str().unwrap()], None);
    assert_eq!(result.code, 2);
    assert!(result.stderr.contains("invalid profile"));
}

#[test]
fn test_rejects_invalid_numeric_options() {
    let cases: [[&str; 2]; 9] = [
        ["--timeout", "0"],
        ["--timeout", "-1"],
        ["--timeout", "nan"],
        ["--timeout", "inf"],
        ["--timeout", "61"],
        ["--max-depth", "0"],
        ["--max-depth", "17"],
        ["--max-workers", "0"],
        ["--max-workers", "65"],
    ];
    for [flag, value] in cases {
        let result = run_devcap(&["scan", "--profile", "python-dev", flag, value], None);
        assert_eq!(result.code, 2, "{flag} {value}");
        assert!(
            result.stderr.to_lowercase().contains("error"),
            "{flag} {value}"
        );
    }
}

#[test]
fn test_profile_and_config_are_mutually_exclusive() {
    let tmp = TempDir::new().unwrap();
    let profile = tmp.path().join("profile.toml");
    fs::write(&profile, "[profile]\nname = 'custom'\n").unwrap();
    let result = run_devcap(
        &[
            "scan",
            "--profile",
            "python-dev",
            "--config",
            profile.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(result.code, 2);
}

#[test]
fn json_output_layout_matches_python_dumps() {
    let tmp = TempDir::new().unwrap();
    let executable = tmp.path().join("fixture-tool");
    fs::write(&executable, "#!/bin/sh\nprintf 'caf\\303\\251 1.2.3\\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let profile = tmp.path().join("fixture.toml");
    fs::write(
        &profile,
        "[[tools]]\nname = \"fixture-tool\"\n[[tools]]\nname = \"absent-tool-xyz\"\n",
    )
    .unwrap();
    let result = run_devcap(
        &[
            "scan",
            "--config",
            profile.to_str().unwrap(),
            "--format",
            "json",
            "--redact",
        ],
        Some(tmp.path()),
    );
    assert_eq!(result.code, 0, "{}", result.stderr);
    let expected_tools = "  \"tools\": [\n    {\n      \"name\": \"fixture-tool\",\n      \"binary\": \"fixture-tool\",\n      \"category\": \"Custom\",\n      \"found\": true,\n      \"version\": \"1.2.3\",\n      \"path\": \"[redacted]\",\n      \"version_diagnostics\": {\n        \"source_stream\": \"stdout\",\n        \"raw_banner\": \"[redacted]\",\n        \"truncated\": false\n      }\n    },\n    {\n      \"name\": \"absent-tool-xyz\",\n      \"binary\": \"absent-tool-xyz\",\n      \"category\": \"Custom\",\n      \"found\": false\n    }\n  ],\n  \"services\": []\n}\n";
    assert!(
        result
            .stdout
            .starts_with("{\n  \"hostname\": \"[redacted]\",\n  \"timestamp\": \"")
    );
    assert!(result.stdout.ends_with(expected_tools), "{}", result.stdout);
}

#[test]
fn probe_timeout_reports_found_without_version() {
    let tmp = TempDir::new().unwrap();
    let executable = tmp.path().join("slow-tool");
    fs::write(&executable, "#!/bin/sh\nsleep 30\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let profile = tmp.path().join("slow.toml");
    fs::write(&profile, "[[tools]]\nname = \"slow-tool\"\n").unwrap();
    let started = std::time::Instant::now();
    let result = run_devcap(
        &[
            "scan",
            "--config",
            profile.to_str().unwrap(),
            "--format",
            "json",
            "--timeout",
            "0.3",
        ],
        Some(tmp.path()),
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(result.code, 0);
    let data = parse(&result.stdout).unwrap();
    let tool = &tools(&data)[0];
    assert_eq!(tool.get("found"), Some(&Json::Bool(true)));
    assert_eq!(tool.get("version"), Some(&Json::Null));
    assert!(tool.get("version_diagnostics").is_none());
}

#[test]
fn parallel_probes_keep_profile_order() {
    let tmp = TempDir::new().unwrap();
    let mut doc = String::new();
    for i in 0..24 {
        let name = format!("ordered-{i:02}");
        let script = tmp.path().join(&name);
        // Later tools answer faster, so completion order is reversed.
        let delay = format!("0.{:02}", 24 - i);
        fs::write(
            &script,
            format!("#!/bin/sh\nsleep {delay}\necho {name} 1.{i}.0\n"),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        doc.push_str(&format!("[[tools]]\nname = \"{name}\"\n"));
    }
    let profile = tmp.path().join("ordered.toml");
    fs::write(&profile, doc).unwrap();
    let result = run_devcap(
        &[
            "scan",
            "--config",
            profile.to_str().unwrap(),
            "--format",
            "json",
            "--max-workers",
            "8",
        ],
        Some(tmp.path()),
    );
    assert_eq!(result.code, 0, "{}", result.stderr);
    let data = parse(&result.stdout).unwrap();
    let names: Vec<String> = tools(&data)
        .iter()
        .map(|t| match t.get("name") {
            Some(Json::Str(s)) => s.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    let expected: Vec<String> = (0..24).map(|i| format!("ordered-{i:02}")).collect();
    assert_eq!(names, expected);
}

#[test]
fn non_utf8_config_and_path_directories_work_like_surrogateescape() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(OsStr::from_bytes(b"bin\xff"));
    fs::create_dir(&dir).unwrap();
    let tool = dir.join("devcap-nu-tool");
    fs::write(&tool, "#!/bin/sh\necho 'nu 4.5.6'\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let profile = dir.join(OsStr::from_bytes(b"p\xfe.toml"));
    fs::write(&profile, "[[tools]]\nname = 'devcap-nu-tool'\n").unwrap();

    let output = bin()
        .arg("scan")
        .arg("--config")
        .arg(&profile)
        .args(["--format", "json"])
        .env("PATH", &dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    // json.dumps writes the lone surrogate CPython decoded the byte to.
    assert!(text.contains("bin\\udcff/devcap-nu-tool"), "{text}");
    assert!(text.contains("\"version\": \"4.5.6\""), "{text}");

    let missing = bin()
        .args(["scan", "--config"])
        .arg(OsStr::from_bytes(b"/nonexistent/\xfe.toml"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(missing.stderr).unwrap(),
        "Error: profile not found: /nonexistent/\\udcfe.toml\n"
    );
}

#[test]
fn relative_config_from_deleted_cwd_is_not_found() {
    let tmp = TempDir::new().unwrap();
    let gone = tmp.path().join("gone");
    fs::create_dir(&gone).unwrap();
    let script = format!(
        "cd '{}' && rmdir '{}' && exec \"$0\" scan --config rel.toml",
        gone.display(),
        gone.display()
    );
    let exe = assert_cmd::cargo::cargo_bin("devcap");
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .arg(&exe) // becomes $0 of the script
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Error: profile not found: rel.toml\n"
    );
}

#[test]
fn integer_options_respect_cpython_digit_limit() {
    let digits = format!("{}1", "0".repeat(4300));
    let result = run_devcap(&["scan", "--max-depth", &digits, "--profile", "nope"], None);
    assert_eq!(result.code, 2);
    assert!(
        result
            .stderr
            .ends_with("devcap scan: error: argument --max-depth: must be an integer\n"),
        "{}",
        result.stderr
    );
}

#[test]
fn probes_do_not_inherit_callers_descriptors() {
    let tmp = TempDir::new().unwrap();
    let executable = tmp.path().join("fd-tool");
    fs::write(
        &executable,
        "#!/bin/sh\nif [ -e /proc/$$/fd/7 ]; then echo 'leaked 1.0'; else echo 'clean 1.0'; fi\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let profile = tmp.path().join("fd.toml");
    fs::write(&profile, "[[tools]]\nname = \"fd-tool\"\n").unwrap();
    // The shell opens descriptor 7 without close-on-exec and execs devcap.
    let output = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "exec 7</dev/null; exec \"$0\" scan --config \"$1\" --format json",
        ])
        .arg(assert_cmd::cargo::cargo_bin("devcap"))
        .arg(&profile)
        .env("PATH", tmp.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    if Path::new("/proc/self/fd").exists() {
        assert!(text.contains("\"raw_banner\": \"clean 1.0\\n\""), "{text}");
    }
}

fn write_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn scan_single(dir: &Path, name: &str, home: Option<&str>) -> String {
    let profile = dir.join("one.toml");
    fs::write(&profile, format!("[[tools]]\nname = \"{name}\"\n")).unwrap();
    let mut cmd = bin();
    cmd.arg("scan")
        .arg("--config")
        .arg(&profile)
        .args(["--format", "json"])
        .env("PATH", dir)
        .timeout(std::time::Duration::from_secs(60));
    match home {
        Some(home) => cmd.env("HOME", home),
        None => cmd.env_remove("HOME"),
    };
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn version_manager_shims_see_home() {
    // Like mise/asdf/pyenv shims: without HOME they print an error banner
    // that would otherwise be reported as the tool's "version".
    let tmp = TempDir::new().unwrap();
    write_tool(
        tmp.path(),
        "shimmed-tool",
        "if [ -z \"$HOME\" ]; then echo 'shim ERROR: HOME is not defined; shim 2026.9.1' >&2; exit 1; fi\n\
         echo \"shimmed-tool 4.5.6 ($HOME)\"\n",
    );
    let text = scan_single(tmp.path(), "shimmed-tool", Some("/home/devcap-test"));
    assert!(text.contains("\"version\": \"4.5.6\""), "{text}");
    assert!(text.contains("(/home/devcap-test)"), "{text}");
    // A relative HOME is not a locator path and is not passed on.
    let text = scan_single(tmp.path(), "shimmed-tool", Some("relative/home"));
    assert!(text.contains("\"version\": \"2026.9.1\""), "{text}");
}

#[test]
fn probes_cannot_read_the_callers_stdin() {
    let tmp = TempDir::new().unwrap();
    // A tool that waits for input before answering would otherwise block
    // until the probe timeout (or read the user's terminal).
    write_tool(
        tmp.path(),
        "stdin-tool",
        "if read line; then echo \"stdin-tool 6.6.6 got:$line\"; else echo 'stdin-tool 1.2.3'; fi\n",
    );
    let profile = tmp.path().join("one.toml");
    fs::write(&profile, "[[tools]]\nname = \"stdin-tool\"\n").unwrap();
    let output = bin()
        .arg("scan")
        .arg("--config")
        .arg(&profile)
        .args(["--format", "json", "--timeout", "10"])
        .env("PATH", tmp.path())
        .write_stdin("secret typed by the user\n")
        .timeout(std::time::Duration::from_secs(60))
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("\"version\": \"1.2.3\""), "{text}");
    assert!(!text.contains("secret"), "{text}");
}

#[test]
fn registry_flags_match_real_cli_conventions() {
    // tmux only accepts -V; kubectl 1.28+ rejects `version --short`.
    let tmp = TempDir::new().unwrap();
    write_tool(
        tmp.path(),
        "tmux",
        "if [ \"$*\" = \"-V\" ]; then echo 'tmux 3.7c'; else echo 'tmux: unknown option -- -' >&2; exit 1; fi\n",
    );
    write_tool(
        tmp.path(),
        "kubectl",
        "if [ \"$*\" = \"version --client\" ]; then printf 'Client Version: v1.31.0\\nKustomize Version: v5.4.2\\n'; \
         else echo 'error: unknown flag: --short' >&2; exit 1; fi\n",
    );
    let text = scan_single(tmp.path(), "tmux", None);
    // The version pattern keeps the numeric part; raw_banner keeps "3.7c".
    assert!(text.contains("\"version\": \"3.7\""), "{text}");
    assert!(text.contains("\"raw_banner\": \"tmux 3.7c\\n\""), "{text}");
    let text = scan_single(tmp.path(), "kubectl", None);
    assert!(text.contains("\"version\": \"1.31.0\""), "{text}");
    // JDK 8 rejects `--version`; `-version` reports on stderr everywhere.
    write_tool(
        tmp.path(),
        "java",
        "if [ \"$*\" = \"-version\" ]; then echo 'java version \"1.8.0_402\"' >&2; \
         else echo 'Unrecognized option: --version' >&2; exit 1; fi\n",
    );
    let text = scan_single(tmp.path(), "java", None);
    assert!(text.contains("\"version\": \"1.8.0\""), "{text}");
    assert!(text.contains("\"source_stream\": \"stderr\""), "{text}");
}

#[test]
fn relative_path_entries_are_not_trusted_outside_projects() {
    // A tool planted in a directory with no project markers, reached through
    // the empty entry a trailing `:` adds to PATH, must not be executed.
    let tmp = TempDir::new().unwrap();
    let ran = tmp.path().join("planted-ran");
    write_tool(
        tmp.path(),
        "planted-tool",
        &format!("touch '{}'\necho planted-tool 6.6.6\n", ran.display()),
    );
    let profile = tmp.path().join("one.toml");
    fs::write(&profile, "[[tools]]\nname = \"planted-tool\"\n").unwrap();
    let scan = |extra: &[&str]| {
        let output = bin()
            .current_dir(tmp.path())
            .arg("scan")
            .arg("--config")
            .arg(&profile)
            .args(["--format", "json"])
            .args(extra)
            .env("PATH", "/nonexistent-devcap-dir:")
            .timeout(std::time::Duration::from_secs(60))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        String::from_utf8(output.stdout).unwrap()
    };
    let text = scan(&[]);
    assert!(text.contains("\"found\": false"), "{text}");
    assert!(
        !ran.exists(),
        "devcap ran a tool from a relative PATH entry"
    );
    let text = scan(&["--include-vendored"]);
    assert!(text.contains("\"version\": \"6.6.6\""), "{text}");
}

#[test]
fn redact_removes_paths_from_error_text_reported_as_a_version() {
    let tmp = TempDir::new().unwrap();
    write_tool(
        tmp.path(),
        "broken-tool",
        "exec /nonexistent-devcap/real-tool \"$@\"\n",
    );
    let profile = tmp.path().join("one.toml");
    fs::write(&profile, "[[tools]]\nname = \"broken-tool\"\n").unwrap();
    let dir = fs::canonicalize(tmp.path()).unwrap();
    for format in ["json", "text", "markdown"] {
        let output = bin()
            .arg("scan")
            .arg("--config")
            .arg(&profile)
            .args(["--format", format, "--redact"])
            .env("PATH", tmp.path())
            .timeout(std::time::Duration::from_secs(60))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("[redacted]"), "{format}: {text}");
        assert!(!text.contains(dir.to_str().unwrap()), "{format}: {text}");
        assert!(!text.contains("/nonexistent-devcap"), "{format}: {text}");
    }
}

#[test]
fn redact_removes_file_urls_and_quoted_paths_from_version_text() {
    let tmp = TempDir::new().unwrap();
    let dir = fs::canonicalize(tmp.path()).unwrap();
    let shown = dir.join("lib");
    // Node.js reports a crashing ES module as a file URL; Rust and Go tools
    // often quote paths with backticks.
    for (name, line) in [
        ("esm-tool", format!("file://{}/cli.mjs:5", shown.display())),
        (
            "tick-tool",
            format!("error: no `Cargo.toml` in `{}`", shown.display()),
        ),
    ] {
        write_tool(
            &dir,
            name,
            &format!("printf '%s\\n' '{line}' >&2\nexit 1\n"),
        );
    }
    let profile = dir.join("two.toml");
    fs::write(
        &profile,
        "[[tools]]\nname = \"esm-tool\"\n\n[[tools]]\nname = \"tick-tool\"\n",
    )
    .unwrap();
    for format in ["json", "text", "markdown"] {
        let output = bin()
            .arg("scan")
            .arg("--config")
            .arg(&profile)
            .args(["--format", format, "--redact"])
            .env("PATH", &dir)
            .timeout(std::time::Duration::from_secs(60))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("[redacted]"), "{format}: {text}");
        assert!(!text.contains(dir.to_str().unwrap()), "{format}: {text}");
    }
}
