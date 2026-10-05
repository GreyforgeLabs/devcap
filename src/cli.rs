//! Command line interface for devcap.

use std::io::Write;

use crate::argparse::{Exit, Kind, Namespace, OptAction, Parser, Val};
use crate::formatters::format_by_name;
use crate::process::COMMAND_TIMEOUT_SECONDS;
use crate::profile::{
    MAX_PROFILE_DEPTH, ProfileError, list_builtin_profiles, load_builtin_profile,
    load_custom_profile,
};
use crate::pycompat::{PyInt, backslash_escapes, format_g, fsencode, parse_float, parse_int};
use crate::scanner::{
    DEFAULT_SCAN_WORKERS, MAX_COMMAND_TIMEOUT_SECONDS, MAX_SCAN_WORKERS, redact_scan, scan_tools,
};

const MAIN_HELP: &str = include_str!("help/main.txt");
const MAIN_USAGE: &str = include_str!("help/main.usage");
const SCAN_HELP: &str = include_str!("help/scan.txt");
const SCAN_USAGE: &str = include_str!("help/scan.usage");
const CHECK_HELP: &str = include_str!("help/check.txt");
const CHECK_USAGE: &str = include_str!("help/check.usage");
const LIST_HELP: &str = include_str!("help/list-profiles.txt");
const LIST_USAGE: &str = include_str!("help/list-profiles.usage");

const FORMATS: &[&str] = &["text", "json", "markdown"];

fn finite_positive_timeout(value: &str) -> Result<Val, String> {
    let parsed = parse_float(value).ok_or_else(|| "must be a number".to_string())?;
    if !parsed.is_finite() || parsed <= 0.0 || parsed > MAX_COMMAND_TIMEOUT_SECONDS {
        return Err(format!(
            "must be greater than 0 and at most {}",
            format_g(MAX_COMMAND_TIMEOUT_SECONDS)
        ));
    }
    Ok(Val::Float(parsed))
}

fn bounded_positive_integer(value: &str, maximum: i64) -> Result<Val, String> {
    let parsed = parse_int(value).ok_or_else(|| "must be an integer".to_string())?;
    match parsed {
        PyInt::Value(v) if (1..=maximum).contains(&v) => Ok(Val::Int(v)),
        _ => Err(format!("must be between 1 and {maximum}")),
    }
}

fn max_depth_value(value: &str) -> Result<Val, String> {
    bounded_positive_integer(value, MAX_PROFILE_DEPTH)
}

fn max_workers_value(value: &str) -> Result<Val, String> {
    bounded_positive_integer(value, MAX_SCAN_WORKERS as i64)
}

fn help_action() -> OptAction {
    OptAction {
        strings: &["-h", "--help"],
        dest: "help",
        kind: Kind::Help,
        default: None,
    }
}

fn scan_like_parser(prog: &'static str, usage: &'static str, help: &'static str) -> Parser {
    let store = |strings, dest, choices, convert, default| OptAction {
        strings,
        dest,
        kind: Kind::Store { choices, convert },
        default,
    };
    let flag = |strings, dest| OptAction {
        strings,
        dest,
        kind: Kind::StoreTrue,
        default: Some(Val::Bool(false)),
    };
    Parser {
        prog,
        usage,
        help,
        actions: vec![
            help_action(),
            store(
                &["--format"],
                "format",
                Some(FORMATS),
                None,
                Some(Val::Str("text".to_string())),
            ),
            store(&["--profile"], "profile", None, None, None),
            store(&["--config"], "config", None, None, None),
            store(
                &["--timeout"],
                "timeout",
                None,
                Some(finite_positive_timeout),
                Some(Val::Float(COMMAND_TIMEOUT_SECONDS)),
            ),
            store(
                &["--max-depth"],
                "max_depth",
                None,
                Some(max_depth_value),
                Some(Val::Int(MAX_PROFILE_DEPTH)),
            ),
            store(
                &["--max-workers"],
                "max_workers",
                None,
                Some(max_workers_value),
                Some(Val::Int(DEFAULT_SCAN_WORKERS as i64)),
            ),
            flag(&["--no-parallel"], "no_parallel"),
            flag(&["--include-vendored"], "include_vendored"),
            flag(&["--redact"], "redact"),
        ],
        mutex_groups: vec![vec![2, 3]],
        subparsers: Vec::new(),
    }
}

/// Build the top-level parser.
pub fn build_parser() -> Parser {
    Parser {
        prog: "devcap",
        usage: MAIN_USAGE,
        help: MAIN_HELP,
        actions: vec![help_action()],
        mutex_groups: Vec::new(),
        subparsers: vec![
            (
                "scan",
                scan_like_parser("devcap scan", SCAN_USAGE, SCAN_HELP),
            ),
            (
                "check",
                scan_like_parser("devcap check", CHECK_USAGE, CHECK_HELP),
            ),
            (
                "list-profiles",
                Parser {
                    prog: "devcap list-profiles",
                    usage: LIST_USAGE,
                    help: LIST_HELP,
                    actions: vec![help_action()],
                    mutex_groups: Vec::new(),
                    subparsers: Vec::new(),
                },
            ),
        ],
    }
}

/// Exit status used when stdout cannot be written (CPython exits 120 when
/// flushing stdout fails at shutdown).
const WRITE_FAILURE: i32 = 120;

/// Run the CLI with explicit output streams; returns the exit status.
pub fn main_with(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let parser = build_parser();
    let args = match parser.parse_args(argv) {
        Ok(args) => args,
        Err(Exit {
            code,
            stdout,
            stderr,
        }) => {
            let _ = out.write_all(stdout.as_bytes());
            let _ = err.write_all(backslash_escapes(&stderr).as_bytes());
            return code;
        }
    };
    let code = match args.command.as_deref() {
        Some("list-profiles") => cmd_list_profiles(out),
        Some("scan") | Some("check") => cmd_scan(&args, out, err),
        _ => write_or_fail(out, MAIN_HELP, 0),
    };
    if out.flush().is_err() {
        return WRITE_FAILURE;
    }
    code
}

fn write_or_fail(out: &mut dyn Write, text: &str, code: i32) -> i32 {
    match out.write_all(text.as_bytes()) {
        Ok(()) => code,
        Err(_) => WRITE_FAILURE,
    }
}

fn cmd_list_profiles(out: &mut dyn Write) -> i32 {
    let mut text = String::from("Available profiles:\n");
    for name in list_builtin_profiles() {
        let profile = match load_builtin_profile(name, MAX_PROFILE_DEPTH) {
            Ok(p) => p,
            Err(e) => unreachable!("built-in profile {name} is invalid: {e}"),
        };
        text.push_str(&format!(
            "  {name:<16} {} ({} tools)\n",
            profile.description,
            profile.tools.len()
        ));
    }
    write_or_fail(out, &text, 0)
}

fn cmd_scan(args: &Namespace, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let max_depth = args.int("max_depth").unwrap_or(MAX_PROFILE_DEPTH);
    let profile_arg = args.str("profile").filter(|p| !p.is_empty());
    let config_arg = args.str("config").filter(|c| !c.is_empty());
    let loaded = if let Some(config) = config_arg {
        load_custom_profile(config, max_depth)
    } else if let Some(profile) = profile_arg {
        load_builtin_profile(profile, max_depth)
    } else {
        load_builtin_profile("full", max_depth)
    };
    let profile = match loaded {
        Ok(profile) => profile,
        Err(ProfileError::NotFound(_)) => {
            let message = match profile_arg {
                Some(name) => format!(
                    "Error: unknown profile '{name}'\nAvailable: {}\n",
                    list_builtin_profiles().join(", ")
                ),
                None => format!(
                    "Error: profile not found: {}\n",
                    args.str("config").unwrap_or_default()
                ),
            };
            let _ = err.write_all(backslash_escapes(&message).as_bytes());
            return 2;
        }
        Err(ProfileError::Invalid(msg)) => {
            let message = format!("Error: invalid profile: {msg}\n");
            let _ = err.write_all(backslash_escapes(&message).as_bytes());
            return 2;
        }
    };

    let max_workers = args
        .int("max_workers")
        .unwrap_or(DEFAULT_SCAN_WORKERS as i64) as usize;
    let result = scan_tools(
        Some(&profile.tools),
        Some(&profile.services),
        !args.flag("no_parallel"),
        max_workers,
        args.flag("include_vendored"),
        args.float("timeout").unwrap_or(COMMAND_TIMEOUT_SECONDS),
    );
    let mut result = match result {
        Ok(r) => r,
        Err(msg) => {
            // Unreachable from the CLI: arguments are validated above.
            let _ = err.write_all(backslash_escapes(&format!("Error: {msg}\n")).as_bytes());
            return 2;
        }
    };
    if args.flag("redact") {
        result = redact_scan(&result, true, true);
    }

    let format = args.str("format").unwrap_or("text");
    let mut rendered = format_by_name(format, &result);
    rendered.push('\n');
    // Text and Markdown write undecodable path bytes back out unchanged
    // (CPython's UTF-8 mode); JSON output is pure ASCII.
    if out.write_all(&fsencode(&rendered)).is_err() {
        return WRITE_FAILURE;
    }

    if args.command.as_deref() == Some("check") && !profile.required_tools.is_empty() {
        let missing: Vec<&str> = result
            .results
            .iter()
            .filter(|r| profile.required_tools.contains(&r.name) && !r.found)
            .map(|r| r.name.as_str())
            .collect();
        if !missing.is_empty() {
            if out.flush().is_err() {
                return WRITE_FAILURE;
            }
            let _ = err.write_all(
                format!("\nMissing required tools: {}\n", missing.join(", ")).as_bytes(),
            );
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> (i32, String, String) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = main_with(&argv, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn scan_error(args: &[&str]) -> String {
        let (code, out, err) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert!(out.is_empty());
        err.lines().last().unwrap_or_default().to_string()
    }

    #[test]
    fn help_and_no_args() {
        let (code, out, err) = run(&[]);
        assert_eq!((code, out.as_str(), err.as_str()), (0, MAIN_HELP, ""));
        assert_eq!(run(&["--he"]).1, MAIN_HELP);
        assert_eq!(run(&["scan", "-h", "--timeout", "0"]).1, SCAN_HELP);
        assert_eq!(run(&["scan", "-hx"]).1, SCAN_HELP);
        assert_eq!(run(&["scan", "-hhh"]).1, SCAN_HELP);
        assert_eq!(run(&["check", "--help"]).1, CHECK_HELP);
        assert_eq!(run(&["list-profiles", "-h"]).1, LIST_HELP);
    }

    #[test]
    fn argparse_error_messages() {
        assert_eq!(
            scan_error(&["scan", "--max", "3"]),
            "devcap scan: error: ambiguous option: --max could match --max-depth, --max-workers"
        );
        assert_eq!(
            scan_error(&["scan", "--max=3"]),
            "devcap scan: error: ambiguous option: --max=3 could match --max-depth, --max-workers"
        );
        assert_eq!(
            scan_error(&["scan", "--timeout", "-1x"]),
            "devcap scan: error: argument --timeout: must be a number"
        );
        assert_eq!(
            scan_error(&["scan", "--timeout", "-1e5"]),
            "devcap scan: error: argument --timeout: must be greater than 0 and at most 60"
        );
        assert_eq!(
            scan_error(&["scan", "--profile", "-x"]),
            "devcap scan: error: argument --profile: expected one argument"
        );
        assert_eq!(
            scan_error(&["scan", "--no-parallel=1"]),
            "devcap scan: error: argument --no-parallel: ignored explicit argument '1'"
        );
        assert_eq!(
            scan_error(&["scan", "--help=x"]),
            "devcap scan: error: argument -h/--help: ignored explicit argument 'x'"
        );
        assert_eq!(
            scan_error(&["scan", "-h=x"]),
            "devcap scan: error: argument -h/--help: ignored explicit argument 'x'"
        );
        assert_eq!(
            scan_error(&["-x"]),
            "devcap: error: unrecognized arguments: -x"
        );
        assert_eq!(
            scan_error(&["-x", "scan", "--profile", "nope"]),
            "devcap: error: unrecognized arguments: -x"
        );
        assert_eq!(
            scan_error(&["scan", "--", "--profile", "nope"]),
            "devcap: error: unrecognized arguments: -- --profile nope"
        );
        assert_eq!(
            scan_error(&["--format", "json"]),
            "devcap: error: argument command: invalid choice: 'json' (choose from 'scan', 'check', 'list-profiles')"
        );
        assert_eq!(
            scan_error(&["scan", "--profile", "a", "--config", ""]),
            "devcap scan: error: argument --config: not allowed with argument --profile"
        );
        assert_eq!(
            scan_error(&["scan", "--format="]),
            "devcap scan: error: argument --format: invalid choice: '' (choose from 'text', 'json', 'markdown')"
        );
        assert_eq!(
            scan_error(&["scan", "--max-depth", "99999999999999999999999"]),
            "devcap scan: error: argument --max-depth: must be between 1 and 16"
        );
        assert_eq!(
            scan_error(&["scan", "-x", "-y"]),
            "devcap: error: unrecognized arguments: -x -y"
        );
        assert_eq!(
            scan_error(&["--version"]),
            "devcap: error: unrecognized arguments: --version"
        );
        assert_eq!(
            scan_error(&["scan", "-=x"]),
            "devcap scan: error: ambiguous option: -=x could match -h, --help, --format, --profile, --config, --timeout, --max-depth, --max-workers, --no-parallel, --include-vendored, --redact"
        );
    }

    #[test]
    fn usage_precedes_error() {
        let (_, _, err) = run(&["scan", "--timeout", "0"]);
        assert_eq!(
            err,
            format!(
                "{SCAN_USAGE}devcap scan: error: argument --timeout: must be greater than 0 and at most 60\n"
            )
        );
        let (_, _, err) = run(&["bogus"]);
        assert!(err.starts_with(MAIN_USAGE));
    }

    #[test]
    fn profile_errors() {
        let (code, _, err) = run(&["scan", "--profile", "nope"]);
        assert_eq!(code, 2);
        assert_eq!(
            err,
            "Error: unknown profile 'nope'\nAvailable: devops, full, node-dev, python-dev, rust-dev, sysadmin\n"
        );
        let (code, _, err) = run(&["scan", "--profile", "../x"]);
        assert_eq!(code, 2);
        assert_eq!(
            err,
            "Error: invalid profile: profile name must be a command name, not a path or shell expression\n"
        );
        let (code, _, err) = run(&["scan", "--config", "/nonexist.toml"]);
        assert_eq!(code, 2);
        assert_eq!(err, "Error: profile not found: /nonexist.toml\n");
        let (code, _, err) = run(&["--", "scan", "--max-depth", " +0_8 ", "--profile", "nope"]);
        assert_eq!(code, 2);
        assert!(err.starts_with("Error: unknown profile 'nope'"));
    }

    #[test]
    fn list_profiles_output() {
        let (code, out, _) = run(&["list-profiles"]);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "Available profiles:\n  devops           DevOps and infrastructure toolchain (20 tools)\n  full             Complete scan of all known tools and services (103 tools)\n  node-dev         Node.js / JavaScript development environment (13 tools)\n  python-dev       Python development environment (12 tools)\n  rust-dev         Rust development environment (11 tools)\n  sysadmin         Linux system administration toolkit (22 tools)\n"
        );
    }
}
