//! Core scan engine — tool detection, version extraction, parallel scanning.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::json::Json;
use crate::process::{
    self, CommandResult, MAX_COMMAND_OUTPUT_BYTES, minimal_probe_environment, run_command,
};
use crate::pycompat::{is_decimal, is_word, normpath, os_path, path_join, splitlines, strip};
use crate::registry::{ToolDef, registry};
use crate::safe_text::clean_text;
use crate::{shlex, sys};

pub use crate::process::{MAX_COMMAND_TIMEOUT_SECONDS, validate_timeout};

pub const MAX_VERSION_BANNER_BYTES: usize = 4096;
pub const MAX_VERSION_LINES: usize = 64;
pub const MAX_SCAN_WORKERS: usize = 64;
pub const DEFAULT_SCAN_WORKERS: usize = 16;

/// Path segments considered vendored and skipped by default.
pub const VENDORED_SEGMENTS: &[&str] = &[
    "node_modules",
    ".venv",
    "venv",
    "__pypackages__",
    ".tox",
    ".nox",
];
const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "pyproject.toml",
    "package.json",
    "Cargo.toml",
    "go.mod",
];
const WORKER_STACK: usize = 512 * 1024;

/// Result of scanning a single tool.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolResult {
    pub name: String,
    pub binary: String,
    pub category: String,
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub version_source: Option<String>,
    pub version_banner: Option<String>,
    pub version_banner_truncated: bool,
}

impl ToolResult {
    /// A not-found result.
    pub fn missing(tool: &ToolDef) -> Self {
        ToolResult {
            name: tool.name.clone(),
            binary: tool.binary.clone(),
            category: tool.category.clone(),
            ..Default::default()
        }
    }

    /// JSON representation (same keys and order as Python's `to_dict`).
    pub fn to_json(&self) -> Json {
        let mut fields = vec![
            ("name".to_string(), Json::str(&self.name)),
            ("binary".to_string(), Json::str(&self.binary)),
            ("category".to_string(), Json::str(&self.category)),
            ("found".to_string(), Json::Bool(self.found)),
        ];
        if self.found {
            fields.push((
                "version".to_string(),
                Json::opt_str(self.version.as_deref()),
            ));
            fields.push(("path".to_string(), Json::opt_str(self.path.as_deref())));
            if let Some(source) = &self.version_source {
                fields.push((
                    "version_diagnostics".to_string(),
                    Json::Object(vec![
                        ("source_stream".to_string(), Json::str(source)),
                        (
                            "raw_banner".to_string(),
                            Json::str(self.version_banner.as_deref().unwrap_or("")),
                        ),
                        (
                            "truncated".to_string(),
                            Json::Bool(self.version_banner_truncated),
                        ),
                    ]),
                ));
            }
        }
        Json::Object(fields)
    }
}

/// Result of checking a systemd service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceResult {
    pub name: String,
    pub active: bool,
    pub user_service: bool,
}

impl ServiceResult {
    /// JSON representation.
    pub fn to_json(&self) -> Json {
        Json::Object(vec![
            ("name".to_string(), Json::str(&self.name)),
            ("active".to_string(), Json::Bool(self.active)),
            ("user_service".to_string(), Json::Bool(self.user_service)),
        ])
    }
}

/// Parsed version plus the bounded stream evidence used to derive it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VersionProbe {
    pub version: Option<String>,
    pub source_stream: Option<String>,
    pub raw_banner: String,
    pub truncated: bool,
}

/// Top-level scan output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanResult {
    pub hostname: String,
    pub timestamp: String,
    pub platform: String,
    pub results: Vec<ToolResult>,
    pub services: Vec<ServiceResult>,
}

impl ScanResult {
    /// JSON representation.
    pub fn to_json(&self) -> Json {
        Json::Object(vec![
            ("hostname".to_string(), Json::str(&self.hostname)),
            ("timestamp".to_string(), Json::str(&self.timestamp)),
            ("platform".to_string(), Json::str(&self.platform)),
            (
                "tools".to_string(),
                Json::Array(self.results.iter().map(ToolResult::to_json).collect()),
            ),
            (
                "services".to_string(),
                Json::Array(self.services.iter().map(ServiceResult::to_json).collect()),
            ),
        ])
    }
}

/// Return a copy with sensitive local metadata replaced.
pub fn redact_scan(scan: &ScanResult, hostname: bool, paths: bool) -> ScanResult {
    let redacted = || Some("[redacted]".to_string());
    let results = scan
        .results
        .iter()
        .map(|r| ToolResult {
            path: if paths && r.path.as_deref().is_some_and(|p| !p.is_empty()) {
                redacted()
            } else {
                r.path.clone()
            },
            version_banner: if r.version_banner.as_deref().is_some_and(|b| !b.is_empty()) {
                redacted()
            } else {
                r.version_banner.clone()
            },
            ..r.clone()
        })
        .collect();
    ScanResult {
        hostname: if hostname {
            "[redacted]".to_string()
        } else {
            scan.hostname.clone()
        },
        timestamp: scan.timestamp.clone(),
        platform: scan.platform.clone(),
        results,
        services: scan.services.clone(),
    }
}

fn is_vendored(path: &str) -> bool {
    path.replace('\\', "/")
        .split('/')
        .any(|part| VENDORED_SEGMENTS.contains(&part))
}

/// The process state binary lookup depends on (`PATH` and the working
/// directory), captured once so lookups are deterministic and testable.
#[derive(Debug, Clone)]
pub struct LookupEnv {
    /// Raw `PATH` value (`os.defpath` when unset).
    pub path: String,
    /// Current working directory (`None` if it cannot be determined).
    pub cwd: Option<PathBuf>,
}

impl LookupEnv {
    /// Capture the current process environment.
    pub fn current() -> Self {
        LookupEnv {
            path: match std::env::var_os("PATH") {
                Some(value) => {
                    use std::os::unix::ffi::OsStrExt;
                    crate::pycompat::fsdecode(value.as_bytes())
                }
                None => process::DEFPATH.to_string(),
            },
            cwd: std::env::current_dir().ok(),
        }
    }

    fn cwd_looks_like_project(&self) -> bool {
        let Some(cwd) = &self.cwd else {
            return false;
        };
        PROJECT_MARKERS
            .iter()
            .any(|marker| std::fs::metadata(cwd.join(marker)).is_ok())
    }

    fn is_project_local(&self, path: &str) -> bool {
        if !path.starts_with('/') {
            return true;
        }
        if !self.cwd_looks_like_project() {
            return false;
        }
        let Some(cwd) = &self.cwd else {
            return false;
        };
        let resolved_path = resolve_lenient(&os_path(path));
        let resolved_cwd = resolve_lenient(cwd);
        resolved_path.starts_with(&resolved_cwd)
    }

    fn is_untrusted_path(&self, path: &str) -> bool {
        is_vendored(path) || self.is_project_local(path)
    }

    fn abspath(&self, path: &str) -> String {
        if path.starts_with('/') {
            normpath(path)
        } else {
            let cwd = self
                .cwd
                .as_ref()
                .map(|c| {
                    use std::os::unix::ffi::OsStrExt;
                    crate::pycompat::fsdecode(c.as_os_str().as_bytes())
                })
                .unwrap_or_default();
            normpath(&path_join(&cwd, path))
        }
    }

    /// Absolute executable path after validating its resolved target.
    fn canonical_executable(&self, path: &str) -> Option<String> {
        let absolute = self.abspath(path);
        let absolute_path = os_path(&absolute);
        let resolved = std::fs::canonicalize(&absolute_path).ok()?;
        let is_file = std::fs::metadata(&resolved).is_ok_and(|m| m.is_file());
        if !is_file || !sys::access_x(&absolute_path) {
            return None;
        }
        Some(absolute)
    }

    fn which_explicit(&self, cmd: &str) -> Option<String> {
        let full = if cmd.starts_with('/') {
            os_path(cmd)
        } else {
            self.cwd.clone().unwrap_or_default().join(os_path(cmd))
        };
        let meta = std::fs::metadata(&full).ok()?;
        if sys::access_fx(&full) && !meta.is_dir() {
            Some(cmd.to_string())
        } else {
            None
        }
    }

    /// Executable matches for `candidate` in PATH order.
    fn iter_binary_matches(&self, candidate: &str) -> Vec<String> {
        if candidate.contains('/') {
            return self
                .which_explicit(candidate)
                .and_then(|p| self.canonical_executable(&p))
                .into_iter()
                .collect();
        }
        let mut matches: Vec<String> = Vec::new();
        for directory in self.path.split(':') {
            let directory = if directory.is_empty() { "." } else { directory };
            let joined = path_join(directory, candidate);
            if let Some(canonical) = self
                .which_explicit(&joined)
                .and_then(|p| self.canonical_executable(&p))
                && !matches.contains(&canonical)
            {
                matches.push(canonical);
            }
        }
        matches
    }

    /// [`find_binary`] against this captured environment.
    pub fn find_binary(&self, tool: &ToolDef, include_vendored: bool) -> Option<String> {
        let mut fallback: Option<String> = None;
        let candidates = std::iter::once(&tool.binary).chain(tool.aliases.iter());
        for candidate in candidates {
            for path in self.iter_binary_matches(candidate) {
                if self.is_untrusted_path(&path) {
                    if include_vendored && fallback.is_none() {
                        fallback = Some(path);
                    }
                    continue;
                }
                return Some(path);
            }
        }
        fallback
    }
}

fn resolve_lenient(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Find the binary on PATH, trying aliases and skipping vendored paths by default.
pub fn find_binary(tool: &ToolDef, include_vendored: bool) -> Option<String> {
    LookupEnv::current().find_binary(tool, include_vendored)
}

/// Bounded decoded banner evidence and whether it was truncated.
fn bounded_banner(text: &str) -> (String, bool) {
    let lines = splitlines(text, true);
    let mut truncated = lines.len() > MAX_VERSION_LINES;
    let selected: String = lines.iter().take(MAX_VERSION_LINES).copied().collect();
    let mut encoded = selected.into_bytes();
    if encoded.len() > MAX_VERSION_BANNER_BYTES {
        encoded.truncate(MAX_VERSION_BANNER_BYTES);
        truncated = true;
    }
    (String::from_utf8_lossy(&encoded).into_owned(), truncated)
}

/// `VERSION_RE.search(line).group(1)` where
/// `VERSION_RE = r"v?(\d+\.\d+(?:\.\d+)?(?:[-+.]\w+)*)"`.
fn search_version(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let digits = |mut i: usize| {
        let start = i;
        while i < chars.len() && is_decimal(chars[i]) {
            i += 1;
        }
        (i > start).then_some(i)
    };
    for start in 0..chars.len() {
        let Some(mut end) = digits(start) else {
            continue;
        };
        if chars.get(end) != Some(&'.') {
            continue;
        }
        let Some(after_minor) = digits(end + 1) else {
            continue;
        };
        end = after_minor;
        if chars.get(end) == Some(&'.')
            && let Some(after_patch) = digits(end + 1)
        {
            end = after_patch;
        }
        while matches!(chars.get(end), Some('-' | '+' | '.'))
            && chars.get(end + 1).is_some_and(|&c| is_word(c))
        {
            end += 1;
            while end < chars.len() && is_word(chars[end]) {
                end += 1;
            }
        }
        return Some(chars[start..end].iter().collect());
    }
    None
}

struct Analysis {
    version: Option<String>,
    descriptive: Option<String>,
    banner: String,
    truncated: bool,
}

/// Find a version anywhere in a bounded banner before descriptive fallback.
fn analyze_banner(output: &str) -> Analysis {
    let (banner, truncated) = bounded_banner(output);
    let mut descriptive: Option<String> = None;
    for raw_line in splitlines(&banner, false) {
        let line = clean_text(strip(raw_line), None);
        if line.is_empty() {
            continue;
        }
        if descriptive.is_none() {
            descriptive = Some(clean_text(&line, Some(80)));
        }
        if let Some(version) = search_version(&line) {
            return Analysis {
                version: Some(version),
                descriptive,
                banner,
                truncated,
            };
        }
    }
    Analysis {
        version: None,
        descriptive,
        banner,
        truncated,
    }
}

/// Extract the first valid version from bounded output, then use a banner fallback.
pub fn extract_version(output: &str) -> Option<String> {
    let analysis = analyze_banner(output);
    analysis.version.or(analysis.descriptive)
}

/// Interpret a probe's output streams (honoring the preferred stream).
fn probe_from_result(tool: &ToolDef, result: &CommandResult) -> VersionProbe {
    let preferred = tool.version_source.as_str();
    let fallback = if preferred == "stderr" {
        "stdout"
    } else {
        "stderr"
    };
    let stdout = analyze_banner(&result.stdout_text());
    let stderr = analyze_banner(&result.stderr_text());
    let pick = |source: &str| if source == "stdout" { &stdout } else { &stderr };
    let order = [preferred, fallback];
    for source in order {
        let a = pick(source);
        if let Some(version) = &a.version {
            return VersionProbe {
                version: Some(version.clone()),
                source_stream: Some(source.to_string()),
                raw_banner: a.banner.clone(),
                truncated: a.truncated || result.output_truncated,
            };
        }
    }
    for source in order {
        let a = pick(source);
        if let Some(descriptive) = &a.descriptive {
            return VersionProbe {
                version: Some(descriptive.clone()),
                source_stream: Some(source.to_string()),
                raw_banner: a.banner.clone(),
                truncated: a.truncated || result.output_truncated,
            };
        }
    }
    VersionProbe {
        truncated: result.output_truncated,
        ..Default::default()
    }
}

/// Run a version probe and honor the configured stream with a true fallback.
pub fn get_version(tool: &ToolDef, path: &str, timeout: f64) -> VersionProbe {
    get_version_with(tool, path, timeout, |cmd, env, timeout| {
        run_command(cmd, Some(env), timeout, MAX_COMMAND_OUTPUT_BYTES)
            .ok()
            .flatten()
    })
}

/// [`get_version`] with an injectable command runner (test seam).
pub fn get_version_with(
    tool: &ToolDef,
    path: &str,
    timeout: f64,
    runner: impl Fn(&[String], Vec<(String, String)>, f64) -> Option<CommandResult>,
) -> VersionProbe {
    let Ok(flag_parts) = shlex::split(&tool.version_flag) else {
        return VersionProbe::default();
    };
    let mut cmd = vec![path.to_string()];
    cmd.extend(flag_parts);
    match runner(&cmd, minimal_probe_environment(path, false), timeout) {
        Some(result) => probe_from_result(tool, &result),
        None => VersionProbe::default(),
    }
}

/// Scan a single tool for presence and version.
pub fn scan_tool(
    tool: &ToolDef,
    include_vendored: bool,
    timeout: f64,
) -> Result<ToolResult, String> {
    let timeout = validate_timeout(timeout)?;
    let Some(path) = find_binary(tool, include_vendored) else {
        return Ok(ToolResult::missing(tool));
    };
    let probe = get_version(tool, &path, timeout);
    Ok(ToolResult {
        name: tool.name.clone(),
        binary: tool.binary.clone(),
        category: tool.category.clone(),
        found: true,
        version: probe.version,
        path: Some(path),
        version_source: probe.source_stream,
        version_banner: Some(probe.raw_banner),
        version_banner_truncated: probe.truncated,
    })
}

/// Check if a systemd service is active.
pub fn check_service(name: &str, user: bool, timeout: f64) -> Result<ServiceResult, String> {
    check_service_with(
        name,
        user,
        timeout,
        &sys::uname().sysname,
        |cmd, env, timeout| {
            run_command(cmd, Some(env), timeout, MAX_COMMAND_OUTPUT_BYTES)
                .ok()
                .flatten()
        },
    )
}

/// [`check_service`] with injectable platform name and runner (test seam).
pub fn check_service_with(
    name: &str,
    user: bool,
    timeout: f64,
    system: &str,
    runner: impl Fn(&[String], Vec<(String, String)>, f64) -> Option<CommandResult>,
) -> Result<ServiceResult, String> {
    let timeout = validate_timeout(timeout)?;
    let inactive = ServiceResult {
        name: name.to_string(),
        active: false,
        user_service: user,
    };
    if system != "Linux" || !crate::profile::is_service_name(name) {
        return Ok(inactive);
    }
    let Some(systemctl) = find_binary(&ToolDef::new("systemctl", "systemctl", "System"), false)
    else {
        return Ok(inactive);
    };
    let mut cmd = vec![systemctl.clone()];
    if user {
        cmd.push("--user".to_string());
    }
    cmd.extend(["is-active", "--quiet", "--", name].map(String::from));
    match runner(&cmd, minimal_probe_environment(&systemctl, user), timeout) {
        Some(result) => Ok(ServiceResult {
            active: result.returncode == 0,
            ..inactive
        }),
        None => Ok(inactive),
    }
}

/// UTC timestamp like `datetime.now(UTC).isoformat(timespec="seconds")`.
pub fn utc_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

enum Job<'a> {
    Tool(&'a ToolDef),
    Service(&'a str, bool),
}

enum Outcome {
    Tool(ToolResult),
    Service(ServiceResult),
}

/// Scan all tools and services, returning a [`ScanResult`].
///
/// `tools: None` scans the full registry. Probes run concurrently on up to
/// `max_workers` threads (when `parallel`), but results keep input order.
pub fn scan_tools(
    tools: Option<&[ToolDef]>,
    services: Option<&[(String, bool)]>,
    parallel: bool,
    max_workers: usize,
    include_vendored: bool,
    timeout: f64,
) -> Result<ScanResult, String> {
    let timeout = validate_timeout(timeout)?;
    if !(1..=MAX_SCAN_WORKERS).contains(&max_workers) {
        return Err(format!(
            "max_workers must be between 1 and {MAX_SCAN_WORKERS}"
        ));
    }
    let registry_tools: Vec<ToolDef>;
    let tools = match tools {
        Some(t) => t,
        None => {
            registry_tools = registry().values().cloned().collect();
            &registry_tools
        }
    };
    let services = services.unwrap_or(&[]);

    let jobs: Vec<Job> = tools
        .iter()
        .map(Job::Tool)
        .chain(services.iter().map(|(n, u)| Job::Service(n, *u)))
        .collect();
    let run = |job: &Job| -> Result<Outcome, String> {
        match job {
            Job::Tool(tool) => scan_tool(tool, include_vendored, timeout).map(Outcome::Tool),
            Job::Service(name, user) => check_service(name, *user, timeout).map(Outcome::Service),
        }
    };

    let outcomes: Vec<Result<Outcome, String>> = if parallel && jobs.len() > 1 {
        let workers = max_workers.min(jobs.len()).max(1);
        let slots: Vec<Mutex<Option<Result<Outcome, String>>>> =
            jobs.iter().map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let worker = || {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= jobs.len() {
                            break;
                        }
                        let outcome = run(&jobs[i]);
                        *slots[i].lock().unwrap_or_else(|p| p.into_inner()) = Some(outcome);
                    }
                };
                let spawned = std::thread::Builder::new()
                    .stack_size(WORKER_STACK)
                    .spawn_scoped(scope, worker);
                if spawned.is_err() {
                    // Fall back to running on the current thread.
                    worker();
                }
            }
        });
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(|p| p.into_inner())
                    .unwrap_or_else(|| Err("probe did not run".to_string()))
            })
            .collect()
    } else {
        jobs.iter().map(run).collect()
    };

    let mut results = Vec::with_capacity(tools.len());
    let mut service_results = Vec::with_capacity(services.len());
    for outcome in outcomes {
        match outcome? {
            Outcome::Tool(r) => results.push(r),
            Outcome::Service(s) => service_results.push(s),
        }
    }

    let uts = sys::uname();
    Ok(ScanResult {
        hostname: sys::hostname(),
        timestamp: utc_timestamp(),
        platform: format!("{} {}", uts.sysname, uts.release),
        results,
        services: service_results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex as StdMutex;

    fn env(path: &str, cwd: &Path) -> LookupEnv {
        LookupEnv {
            path: path.to_string(),
            cwd: Some(cwd.to_path_buf()),
        }
    }

    /// A working directory with no project markers (removed right away,
    /// so it can never look like a project checkout).
    fn neutral_dir() -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        dir.path().to_path_buf()
    }

    fn fake_tool(path: &Path) {
        std::fs::write(path, "#!/bin/sh\nprintf 'fake 1.0.0\\n'\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn result(stdout: &str, stderr: &str) -> CommandResult {
        CommandResult {
            args: vec!["/bin/tool".into()],
            returncode: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            output_truncated: false,
        }
    }

    #[test]
    fn test_extract_version_semver() {
        assert_eq!(extract_version("Python 3.12.3").as_deref(), Some("3.12.3"));
    }

    #[test]
    fn test_extract_version_with_v_prefix() {
        assert_eq!(extract_version("v1.2.3").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn test_extract_version_multiline() {
        assert_eq!(
            extract_version("ruff 0.15.0\nsome other line\n").as_deref(),
            Some("0.15.0")
        );
    }

    #[test]
    fn test_extract_version_scans_past_warning() {
        let output = "warning: optional plugin unavailable\ntool version 7.8.9\n";
        assert_eq!(extract_version(output).as_deref(), Some("7.8.9"));
    }

    #[test]
    fn test_extract_version_fallback() {
        assert_eq!(
            extract_version("no version number here").as_deref(),
            Some("no version number here")
        );
    }

    #[test]
    fn test_extract_version_empty() {
        assert_eq!(extract_version(""), None);
        assert_eq!(extract_version("\n\n"), None);
    }

    #[test]
    fn test_extract_version_truncation() {
        let result = extract_version(&"x".repeat(100)).unwrap();
        assert!(result.chars().count() <= 80);
    }

    #[test]
    fn test_extract_version_strips_terminal_controls() {
        assert_eq!(
            extract_version("\x1b[31mtool 1.2.3\x1b[0m\n").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(extract_version("bad\x07line").as_deref(), Some("bad line"));
    }

    #[test]
    fn version_regex_edge_cases() {
        assert_eq!(search_version("1.2rc1").as_deref(), Some("1.2"));
        assert_eq!(search_version("1.2.3abc").as_deref(), Some("1.2.3"));
        assert_eq!(
            search_version("go1.27.0-X linux").as_deref(),
            Some("1.27.0-X")
        );
        assert_eq!(
            search_version("1.2.3.4-rc_1+b.5 x").as_deref(),
            Some("1.2.3.4-rc_1+b.5")
        );
        assert_eq!(search_version("v.1.2").as_deref(), Some("1.2"));
        assert_eq!(search_version("1. 2"), None);
        assert_eq!(
            search_version("\u{663}.\u{665}").as_deref(),
            Some("\u{663}.\u{665}")
        );
        assert_eq!(search_version("1.2-"), Some("1.2".to_string()));
    }

    #[test]
    fn test_get_version_stream_preference_and_fallback() {
        let cases = [
            ("stdout", "warning\ntool 2.3.4\n", "", "2.3.4", "stdout"),
            (
                "stdout",
                "warning only\n",
                "tool 3.4.5\n",
                "3.4.5",
                "stderr",
            ),
            ("stdout", "", "tool 4.5.6\n", "4.5.6", "stderr"),
            ("stderr", "tool 5.6.7\n", "", "5.6.7", "stdout"),
            (
                "stderr",
                "stdout banner\n",
                "stderr banner\n",
                "stderr banner",
                "stderr",
            ),
        ];
        for (preferred, stdout, stderr, expected, source) in cases {
            let tool = ToolDef::new("tool", "tool", "Test").with_version_source(preferred);
            let probe = get_version_with(&tool, "/bin/tool", 5.0, |_, _, _| {
                Some(result(stdout, stderr))
            });
            assert_eq!(probe.version.as_deref(), Some(expected));
            assert_eq!(probe.source_stream.as_deref(), Some(source));
            let raw = if source == "stdout" { stdout } else { stderr };
            assert_eq!(probe.raw_banner, raw);
        }
    }

    #[test]
    fn banner_bounds() {
        let many: String = (0..70).map(|i| format!("line {i}\n")).collect();
        let (banner, truncated) = bounded_banner(&many);
        assert!(truncated);
        assert_eq!(splitlines(&banner, false).len(), MAX_VERSION_LINES);
        let wide = "é".repeat(3000);
        let (banner, truncated) = bounded_banner(&wide);
        assert!(truncated);
        assert!(banner.len() <= MAX_VERSION_BANNER_BYTES + 3);
    }

    #[test]
    fn test_scan_tools_rejects_invalid_worker_bound() {
        // Python also rejected 1.5 and True; usize excludes them by type.
        for max_workers in [0, 65] {
            let err = scan_tools(Some(&[]), None, true, max_workers, false, 5.0).unwrap_err();
            assert!(err.contains("max_workers"), "{err}");
        }
    }

    #[test]
    fn test_find_binary_skips_vendored_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let vendored = root.join("node_modules").join(".bin");
        std::fs::create_dir_all(&vendored).unwrap();
        let fake = vendored.join("fake-tool");
        fake_tool(&fake);
        let lookup = env(vendored.to_str().unwrap(), &neutral_dir());
        let tool = ToolDef::new("fake-tool", "fake-tool", "Test");
        assert_eq!(lookup.find_binary(&tool, false), None);
        assert_eq!(lookup.find_binary(&tool, true).as_deref(), fake.to_str());
    }

    #[test]
    fn test_find_binary_prefers_non_vendored_alternative() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let vendored = root.join("node_modules").join(".bin");
        let normal = root.join("bin");
        std::fs::create_dir_all(&vendored).unwrap();
        std::fs::create_dir_all(&normal).unwrap();
        fake_tool(&vendored.join("fake-tool"));
        fake_tool(&normal.join("fake-tool"));
        let lookup = env(
            &format!("{}:{}", vendored.display(), normal.display()),
            &neutral_dir(),
        );
        let tool = ToolDef::new("fake-tool", "fake-tool", "Test");
        assert_eq!(
            lookup.find_binary(&tool, false).as_deref(),
            normal.join("fake-tool").to_str()
        );
    }

    #[test]
    fn test_find_binary_skips_project_local_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let local_bin = root.join("bin");
        std::fs::create_dir(&local_bin).unwrap();
        let fake = local_bin.join("fake-tool");
        fake_tool(&fake);
        std::fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"example\"\n",
        )
        .unwrap();
        let lookup = env("bin", &root);
        let tool = ToolDef::new("fake-tool", "fake-tool", "Test");
        assert_eq!(lookup.find_binary(&tool, false), None);
        assert_eq!(lookup.find_binary(&tool, true).as_deref(), fake.to_str());
    }

    #[test]
    fn find_binary_uses_aliases_and_dedups() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let bin = root.join("bin");
        std::fs::create_dir(&bin).unwrap();
        fake_tool(&bin.join("fdfind"));
        let lookup = env(&format!("{0}:{0}/:{0}//", bin.display()), &neutral_dir());
        let tool = ToolDef::new("fd", "fd-missing-xyz", "Test").with_aliases(&["fdfind"]);
        assert_eq!(
            lookup.find_binary(&tool, false).as_deref(),
            bin.join("fdfind").to_str()
        );
        assert_eq!(lookup.iter_binary_matches("fdfind").len(), 1);
        // A project checkout as cwd marks binaries inside it untrusted.
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        let inside = env(bin.to_str().unwrap(), &root);
        assert_eq!(inside.find_binary(&tool, false), None);
    }

    #[test]
    fn test_check_service_uses_argument_separator() {
        let captured = StdMutex::new(Vec::new());
        let result = check_service_with("docker.service", false, 5.0, "Linux", |cmd, _, _| {
            *captured.lock().unwrap() = cmd.to_vec();
            Some(CommandResult {
                args: cmd.to_vec(),
                returncode: 0,
                stdout: vec![],
                stderr: vec![],
                output_truncated: false,
            })
        })
        .unwrap();
        let cmd = captured.into_inner().unwrap();
        if cmd.is_empty() {
            // No systemctl on this host: the service is reported inactive.
            assert!(!result.active);
        } else {
            assert!(result.active);
            assert_eq!(&cmd[cmd.len() - 2..], ["--", "docker.service"]);
        }
    }

    #[test]
    fn test_check_service_rejects_option_like_name() {
        let result = check_service_with("--user", false, 5.0, "Linux", |_, _, _| {
            panic!("systemctl should not run for unsafe service names")
        })
        .unwrap();
        assert!(!result.active);
        let other = check_service_with("sshd", false, 5.0, "Darwin", |_, _, _| {
            panic!("systemctl should not run off Linux")
        })
        .unwrap();
        assert!(!other.active);
    }

    #[test]
    fn test_scan_tool_python3_equivalent() {
        // Python probed the host python3; the Rust suite probes `sh` with a
        // fixed banner so it does not depend on host interpreters.
        let tool =
            ToolDef::new("sh", "sh", "Languages").with_version_flag("-c 'echo Fixture 3.12.3'");
        let result = scan_tool(&tool, false, 5.0).unwrap();
        assert!(result.found);
        assert!(result.version.as_deref().unwrap().contains("3."));
        assert!(result.path.is_some());
        assert_eq!(result.version_source.as_deref(), Some("stdout"));
        assert_eq!(result.version_banner.as_deref(), Some("Fixture 3.12.3\n"));
    }

    #[test]
    fn test_scan_tool_missing() {
        let tool = ToolDef::new("nonexistent_xyz", "nonexistent_xyz_binary", "Test");
        let result = scan_tool(&tool, false, 5.0).unwrap();
        assert!(!result.found);
        assert!(result.version.is_none());
        assert!(result.path.is_none());
    }

    #[test]
    fn test_scan_tools_basic() {
        let tools = [ToolDef::new("sh", "sh", "Languages").with_version_flag("-c 'echo sh 1.0.0'")];
        let result = scan_tools(Some(&tools), None, false, 16, false, 5.0).unwrap();
        assert!(!result.hostname.is_empty());
        assert!(!result.timestamp.is_empty());
        assert_eq!(result.results.len(), 1);
        assert!(result.results[0].found);
    }

    #[test]
    fn test_scan_tools_parallel() {
        let tools = [
            ToolDef::new("sh", "sh", "Languages").with_version_flag("-c 'echo sh 1.0.0'"),
            ToolDef::new("missing", "nonexistent_xyz_binary", "Test"),
        ];
        let result = scan_tools(Some(&tools), None, true, 16, false, 5.0).unwrap();
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.results[0].name, "sh");
        assert_eq!(result.results[1].name, "missing");
    }

    #[test]
    fn test_tool_result_to_dict() {
        let tr = ToolResult {
            name: "python3".into(),
            binary: "python3".into(),
            category: "Languages".into(),
            found: true,
            version: Some("3.12.3".into()),
            path: Some("/usr/bin/python3".into()),
            version_source: Some("stdout".into()),
            version_banner: Some("Python 3.12.3\n".into()),
            version_banner_truncated: false,
        };
        let d = tr.to_json();
        assert_eq!(d.get("name"), Some(&Json::str("python3")));
        assert_eq!(d.get("found"), Some(&Json::Bool(true)));
        assert_eq!(d.get("version"), Some(&Json::str("3.12.3")));
        assert_eq!(
            d.get("version_diagnostics"),
            Some(&Json::Object(vec![
                ("source_stream".into(), Json::str("stdout")),
                ("raw_banner".into(), Json::str("Python 3.12.3\n")),
                ("truncated".into(), Json::Bool(false)),
            ]))
        );
    }

    #[test]
    fn test_tool_result_to_dict_missing() {
        let tr = ToolResult::missing(&ToolDef::new("missing", "missing", "Test"));
        let d = tr.to_json();
        assert_eq!(d.get("found"), Some(&Json::Bool(false)));
        assert!(d.get("version").is_none());
        assert!(d.get("path").is_none());
    }

    #[test]
    fn test_scan_result_to_dict() {
        let result = scan_tools(Some(&[]), None, false, 16, false, 5.0).unwrap();
        let d = result.to_json();
        for key in ["hostname", "timestamp", "platform", "tools", "services"] {
            assert!(d.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn test_redact_scan_replaces_hostname_and_paths() {
        let scan = ScanResult {
            hostname: "private-host".into(),
            timestamp: "now".into(),
            platform: "Linux".into(),
            results: vec![ToolResult {
                name: "python3".into(),
                binary: "python3".into(),
                category: "Languages".into(),
                found: true,
                version: Some("3.12.3".into()),
                path: Some("/home/user/.local/bin/python3".into()),
                ..Default::default()
            }],
            services: vec![],
        };
        let redacted = redact_scan(&scan, true, true);
        assert_eq!(redacted.hostname, "[redacted]");
        assert_eq!(redacted.results[0].path.as_deref(), Some("[redacted]"));
    }

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00+00:00");
        assert_eq!(
            format_utc(1_791_158_400 + 3661),
            "2026-10-05T01:01:01+00:00"
        );
    }

    #[test]
    fn vendored_detection() {
        assert!(is_vendored("/a/node_modules/.bin/x"));
        assert!(is_vendored("/a\\.venv\\x"));
        assert!(!is_vendored("/a/venvs/x"));
    }
}
