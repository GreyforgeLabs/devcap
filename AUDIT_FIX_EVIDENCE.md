# Audit remediation evidence

Release: `0.2.0`

Date: 2026-08-28

## GF-AUD-027

- Version detection searches all bounded lines for the first valid version before descriptive fallback.
- The configured stdout/stderr stream is preferred, with the other stream used as a true fallback.
- JSON output records the selected source stream, bounded raw banner, and truncation state.
- Subprocess capture replaces invalid UTF-8 and enforces per-stream byte limits.

Targeted tests cover warning-first output, both stream preferences, an empty preferred stream,
multiline and non-version banners, invalid UTF-8, and output flooding.

## GF-AUD-028

- Custom TOML is opened once, checked with `fstat`, and read from that exact descriptor with a
  one-megabyte cap.
- Unknown keys and duplicate case-normalized tool names fail before scanning; registry construction
  also rejects normalized duplicates.
- Executables are canonicalized to absolute paths and probes receive a minimal deterministic
  environment.
- Timeout, structural profile depth, and worker count are explicit positive bounded values.

Targeted tests cover file growth/replacement, oversized profiles, unknown keys, duplicate names,
malicious inherited environment state, and invalid numeric bounds.

## Validation

- Tier 2 subsystem gate: `python -m pytest -q`
- Static checks: `python -m ruff check src tests`
- Formatting: `python -m ruff format --check src tests`
- Packaging: `python -m build` and `python -m twine check dist/*`
- Patch hygiene: `git diff --check`

No external probes beyond deterministic local test subprocesses, publication, or deployment occurred
during remediation.
