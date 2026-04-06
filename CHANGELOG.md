# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

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
