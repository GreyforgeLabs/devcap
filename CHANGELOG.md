# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
