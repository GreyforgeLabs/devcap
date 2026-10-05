# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.0] - 2026-10-05

### Changed

- Rewrote devcap in Rust. It ships as a single native binary with the six built-in TOML profiles embedded, and it no longer needs a Python runtime. The project stays maintenance-only: this release ports the existing tool and adds no features.
- Kept the CLI the same: subcommands, flags, defaults, help text, error messages, exit codes, the profile file format, and text, JSON, and Markdown output, including JSON key order and `ensure_ascii` escaping.
- Ported argparse behavior (prefix abbreviations, `--opt=value`, ambiguous-option errors, `--` handling) and the CPython `tomllib` parser, so usage errors and TOML error messages (`... (at line L, column C)`) read exactly as they did before.
- Version probes still run in a bounded worker pool (`--max-workers`, default 16), with the same per-probe timeout, 64 KiB per-stream output cap, process-group kill, minimal `LANG=C` environment, and new-session (`setsid`) isolation. Each probe calls `execve` directly, as CPython did, so an executable without a shebang is never re-run through `/bin/sh`. Results keep profile order.
- Measured on greyarch (Linux x86_64; details in `docs/benchmarks.md`): `--help` starts in 2.2 ms instead of 76.4 ms. `scan --profile python-dev` takes 15.6 ms instead of 101.6 ms. A full 103-tool scan takes 623 ms instead of 755 ms; that scan cannot finish faster than its slowest probe, about 580 ms here. devcap's own peak RSS during a full scan is 4.3 MiB instead of 22.4 MiB. The stripped binary is 751 KB.
- Replaced the Python packaging, PyPI publish workflow, and pytest suite with Cargo, a cargo-based CI and release workflow, and Rust tests. Every original test case was ported, and new golden tests replay outputs captured from the Python 0.2.1 implementation.
- Corrected the README profile table: `devops` lists 20 tools, not 21.

### Intentional deviations from 0.2.1

- Inputs that crashed Python with a traceback and exit status 1 now produce an `Error: invalid profile: ...` message and exit status 2. This covers `--config ~user/...` when the home directory cannot be found, TOML nested deeper than CPython's recursion limit (`maximum recursion depth exceeded while parsing TOML`), and dotted keys with more than 1000 parts.
- A deleted or unreadable working directory no longer crashes a scan; it is treated as "not a project checkout".
- Help and usage text always uses argparse's plain 80-column layout, which is what pipes and CI saw before. Python 3.14 re-wrapped help to the terminal width (`COLUMNS`) and colorized it on a TTY.
- If stdout cannot be written (for example, a closed pipe), devcap exits with status 120 but no longer prints Python's `BrokenPipeError` traceback.
- Service checks share the bounded worker pool with tool probes; Python checked services one after another once all tools were done. Output order is unchanged, and `--no-parallel` still runs everything sequentially.
- `~user` in `--config` is resolved from `/etc/passwd` and does not go through NSS.
- Non-UTF-8 bytes in arguments, `PATH`, `HOME`, and the working directory are handled like Python's `surrogateescape`: such files and tools are found, error messages and JSON show `\udcXX`, and text or Markdown output writes the original bytes (Python 0.2.1 crashed with `UnicodeEncodeError` there unless it ran in UTF-8 mode). Internally the undecodable bytes are carried as the private-use characters U+10FF80 to U+10FFFF, so those 128 rarely used characters, if they appear in arguments, profile text, or probe output, are displayed as escaped bytes rather than as themselves.

## [0.2.1] - 2026-09-27

### Changed

- Mark the standalone registry as maintenance-only while preserving the scanner for existing users.
- Make the CLI check integration test independent of host-installed Python development tools.

## [0.2.0] - 2026-08-28

### Security

- Scan every bounded banner line for a version before using descriptive fallback text.
- Honor the configured stdout/stderr preference with a true fallback and JSON diagnostics.
- Bound subprocess output, use canonical executable paths, and minimize probe environments.
- Read custom profiles once through the opened descriptor and enforce byte/depth limits.
- Reject unknown profile keys and duplicate case-normalized tool names.
- Validate timeout, profile-depth, and worker-count options as finite positive bounds.
- Validate custom TOML profile schema, command fields, service names, and profile size before scanning.
- Reject high-risk custom interpreter commands and shell-control characters in custom version flags.
- Skip vendored/project-local PATH segments by default; add `--include-vendored` for trusted checkouts.
- Add `--redact` to replace hostnames and executable paths before public sharing.
- Sanitize terminal control sequences and Markdown table delimiters in human-readable output.
- Add `systemctl --` argument separation for service checks and process-group cleanup on scan timeouts.
- Harden GitHub Actions release/publish workflows with pinned action commits, tag/version checks, job timeouts, and narrower permissions.

## [0.1.0] - 2026-04-06

### Added

- Initial release
- Scan 84 tools across 14 categories (Languages, Package Managers, Build Tools, Version Control, Containers, Editors, Linting & Formatting, Testing, Debugging & Profiling, Network, Database, Search & Files, AI Tools, Miscellaneous)
- Three output formats: text, JSON, markdown
- Six built-in profiles: full, python-dev, node-dev, rust-dev, devops, sysadmin
- Custom TOML profile support
- Parallel scanning via ThreadPoolExecutor (~7x speedup)
- systemd service status checks
- `check` subcommand for CI gating (exit 1 if required tools missing)
- Binary alias resolution (fd/fdfind, bat/batcat)
- Vendored path skipping (node_modules, .venv)
