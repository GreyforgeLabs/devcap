# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| latest  | Yes       |

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly.

**Do NOT open a public issue.**

Instead, use one of these methods:

1. **GitHub Security Advisories** (preferred): Use the "Report a vulnerability" button on the Security tab of this repository.
2. **Email**: Contact the maintainers through [greyforge.tech](https://greyforge.tech).

## What to Include

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if you have one)

## Local Scan Boundary

`devcap` is a local inventory tool. It executes discovered binaries with version flags, so custom profiles and PATH entries from untrusted repositories must be treated as executable inputs. The default scanner rejects high-risk custom interpreter commands, skips vendored/project-local PATH segments and relative PATH entries (`.`, an empty entry such as a trailing `:`, or `bin`) unless `--include-vendored` is set, validates custom profile schema, separates `systemctl` options from service names, and sanitizes terminal/Markdown display output.

Each probe runs in its own session with stdin at `/dev/null`, `LANG=C`/`LC_ALL=C`, a minimal `PATH`, and no other inherited variables except non-secret locator paths: `HOME`, the XDG base directories, and version-manager roots (`ASDF_DIR`, `ASDF_DATA_DIR`, `MISE_DATA_DIR`, `MISE_CONFIG_DIR`, `PYENV_ROOT`, `RBENV_ROOT`, `NODENV_ROOT`, `VOLTA_HOME`, `RUSTUP_HOME`, `CARGO_HOME`), passed only when they hold an absolute path. Tokens, `LD_*`, and interpreter option variables never reach a probe.

Inventory output may include hostnames, OS details, executable paths, tool versions, and service state. Use `--redact` to suppress the hostname, executable paths, raw version banners, and paths inside version text (a failing probe reports its first error line, which often names the executable), then review JSON, text, and Markdown output before publishing it or uploading it as a public artifact.

## Response Timeline

- **Acknowledgment**: Within 48 hours
- **Assessment**: Within 7 days
- **Fix or mitigation**: Depends on severity, but we aim for 30 days for critical issues

## Disclosure

We follow coordinated disclosure. Please allow us reasonable time to address the issue before making it public.

---

Built by [Greyforge](https://greyforge.tech)
