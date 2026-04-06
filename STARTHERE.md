# STARTHERE.md — AI Bootstrap Guide

> This file is designed for AI coding assistants. If you are a human,
> see [README.md](README.md) for the human-friendly guide.

## Quick Bootstrap

```bash
git clone https://github.com/GreyforgeLabs/devcap.git && cd devcap && ./scripts/setup.sh
```

## What This Project Does

`devcap` scans a machine for installed development tools, extracts versions, and reports results in text, JSON, or markdown. It supports built-in and custom TOML profiles for filtering which tools to scan. Zero runtime dependencies — stdlib only.

## Project Structure

```text
devcap/
  src/devcap/
    __init__.py       # public API exports
    __main__.py       # python -m devcap
    cli.py            # argparse CLI (scan, check, list-profiles)
    scanner.py        # core engine: shutil.which + subprocess + ThreadPoolExecutor
    registry.py       # 84 tool definitions across 14 categories
    formatters.py     # text, json, markdown output
    profiles.py       # TOML profile loader
    profiles/         # built-in TOML profiles
      full.toml
      python-dev.toml
      node-dev.toml
      rust-dev.toml
      devops.toml
      sysadmin.toml
  tests/              # pytest test suite
  scripts/setup.sh    # idempotent setup
  README.md           # human-facing docs
  STARTHERE.md        # this file
```

## Setup Prerequisites

- Python 3.11+

## Installation Steps

1. Clone: `git clone https://github.com/GreyforgeLabs/devcap.git`
2. Enter directory: `cd devcap`
3. Run setup: `./scripts/setup.sh`

## Verification

```bash
devcap scan --format json
# Expected: JSON object with hostname, timestamp, platform, tools[], services[]

devcap check --profile python-dev
# Expected: exit code 0 on any machine with python3, pip, git
```

## Key Entry Points

- `src/devcap/cli.py` — CLI entry point (`main()` function)
- `src/devcap/scanner.py` — core scanning engine
- `src/devcap/registry.py` — tool definitions (add new tools here)
- `src/devcap/profiles.py` — TOML profile loading
- `src/devcap/formatters.py` — output formatting

## Common Tasks

```bash
# Run tests
pytest

# Run linter
ruff check .

# Scan this machine
devcap scan

# Add a new tool: edit src/devcap/registry.py, add ToolDef to REGISTRY
```
