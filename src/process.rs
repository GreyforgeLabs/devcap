//! Bounded subprocess execution with process-group cleanup
//! (port of `scanner._run_command`).

use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::sys;

pub const COMMAND_TIMEOUT_SECONDS: f64 = 5.0;
pub const MAX_COMMAND_TIMEOUT_SECONDS: f64 = 60.0;
pub const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

const READ_CHUNK: usize = 4096;
/// Grace period for output pipes after the probe exits.
const READER_JOIN: Duration = Duration::from_millis(250);
/// Exit-check interval when no pidfd is available (non-Linux, old kernels).
const EXIT_POLL_MS: u128 = 10;

/// Bounded subprocess result with explicit truncation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub args: Vec<String>,
    pub returncode: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub output_truncated: bool,
}

impl CommandResult {
    /// `stdout` decoded like `bytes.decode("utf-8", errors="replace")`.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// `stderr` decoded like `bytes.decode("utf-8", errors="replace")`.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Validate a probe timeout (finite, > 0, <= 60).
pub fn validate_timeout(timeout: f64) -> Result<f64, String> {
    if !timeout.is_finite() || timeout <= 0.0 || timeout > MAX_COMMAND_TIMEOUT_SECONDS {
        return Err(format!(
            "timeout must be greater than 0 and at most {} seconds",
            crate::pycompat::format_g(MAX_COMMAND_TIMEOUT_SECONDS)
        ));
    }
    Ok(timeout)
}

/// Validate a per-stream output cap.
pub fn validate_output_limit(value: usize) -> Result<usize, String> {
    if value < 1 {
        return Err("max_output_bytes must be a positive integer".to_string());
    }
    if value > MAX_COMMAND_OUTPUT_BYTES {
        return Err(format!(
            "max_output_bytes must be at most {MAX_COMMAND_OUTPUT_BYTES}"
        ));
    }
    Ok(value)
}

/// `os.defpath` on POSIX.
pub const DEFPATH: &str = "/bin:/usr/bin";

fn pathlib_parent(executable: &str) -> String {
    match executable.rfind('/') {
        None => ".".to_string(),
        Some(idx) => {
            let parent = executable[..idx].trim_end_matches('/');
            if !parent.is_empty() {
                parent.to_string()
            } else if executable.starts_with("//") && !executable.starts_with("///") {
                "//".to_string()
            } else {
                "/".to_string()
            }
        }
    }
}

/// Non-secret locator variables passed through to probes when they hold an
/// absolute path. Version-manager shims (mise, asdf, pyenv, rbenv, nodenv,
/// Volta, rustup) need them to find the toolchain the user's shell would run;
/// without `HOME` they print an error banner instead of the tool's version.
pub const PROBE_LOCATOR_VARS: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "ASDF_DIR",
    "ASDF_DATA_DIR",
    "MISE_DATA_DIR",
    "MISE_CONFIG_DIR",
    "PYENV_ROOT",
    "RBENV_ROOT",
    "NODENV_ROOT",
    "VOLTA_HOME",
    "RUSTUP_HOME",
    "CARGO_HOME",
];

/// Build a minimal deterministic environment for external probes.
pub fn minimal_probe_environment(executable: &str, user_service: bool) -> Vec<(String, String)> {
    probe_environment_from(executable, user_service, |name| {
        use std::os::unix::ffi::OsStrExt;
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(|value| crate::pycompat::fsdecode(value.as_bytes()))
    })
}

/// [`minimal_probe_environment`] with an injectable variable lookup.
pub fn probe_environment_from(
    executable: &str,
    user_service: bool,
    getenv: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut path_entries: Vec<String> = vec![pathlib_parent(executable)];
    for entry in DEFPATH.split(':') {
        if !path_entries.iter().any(|e| e == entry) {
            path_entries.push(entry.to_string());
        }
    }
    let mut env = vec![
        ("LANG".to_string(), "C".to_string()),
        ("LC_ALL".to_string(), "C".to_string()),
        ("PATH".to_string(), path_entries.join(":")),
    ];
    for &name in PROBE_LOCATOR_VARS {
        if let Some(value) = getenv(name).filter(|v| v.starts_with('/')) {
            env.push((name.to_string(), value));
        }
    }
    if user_service {
        for name in ["DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR"] {
            if let Some(value) = getenv(name) {
                env.push((name.to_string(), value));
            }
        }
    }
    env
}

/// Kill a process group (falling back to the process) like `_terminate_process`.
fn terminate_process(pid: u32) {
    match sys::killpg_sigkill(pid) {
        Ok(()) => {}
        Err(errno) if errno == libc::ESRCH => {}
        Err(_) => sys::kill_sigkill(pid),
    }
}

/// One captured output pipe.
struct Capture {
    pipe: Option<File>,
    data: Vec<u8>,
}

impl Capture {
    fn new(pipe: Option<impl Into<OwnedFd>>) -> Self {
        Capture {
            pipe: pipe.map(|p| File::from(p.into())),
            data: Vec::new(),
        }
    }

    /// Read what is available; returns `true` when the cap was exceeded.
    fn read_available(&mut self, limit: usize) -> bool {
        let Some(pipe) = self.pipe.as_mut() else {
            return false;
        };
        let mut buf = [0u8; READ_CHUNK];
        match pipe.read(&mut buf) {
            Ok(0) => self.pipe = None,
            Ok(n) => {
                let remaining = limit.saturating_sub(self.data.len());
                self.data.extend_from_slice(&buf[..n.min(remaining)]);
                if n > remaining {
                    self.pipe = None;
                    return true;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => self.pipe = None,
        }
        false
    }
}

/// Run a command with bounded time/output and process-group cleanup.
///
/// The probe's stdin is `/dev/null`. Both output pipes and (on Linux) a
/// pidfd for the child are multiplexed with `poll(2)` on the calling thread,
/// so a probe costs no helper threads. The timeout covers the probe until it
/// exits; afterwards the pipes get a short grace period (for descendants that
/// still hold them) before the process group is killed.
///
/// `Err` mirrors the Python `ValueError`s for invalid arguments; `Ok(None)`
/// means the command could not be started or timed out.
pub fn run_command(
    cmd: &[String],
    env: Option<Vec<(String, String)>>,
    timeout: f64,
    max_output_bytes: usize,
) -> Result<Option<CommandResult>, String> {
    let timeout = validate_timeout(timeout)?;
    let limit = validate_output_limit(max_output_bytes)?;
    if cmd.is_empty() || !cmd[0].starts_with('/') {
        return Err("command executable must be an absolute path".to_string());
    }
    let env = env.unwrap_or_else(|| minimal_probe_environment(&cmd[0], false));
    let Some(spec) = sys::ExecSpec::new(cmd, &env) else {
        return Err("embedded null byte".to_string());
    };

    let os = |s: &str| crate::pycompat::os_path(s).into_os_string();
    let mut command = Command::new(os(&cmd[0]));
    command
        .args(cmd[1..].iter().map(|a| os(a)))
        .env_clear()
        .envs(env.iter().map(|(k, v)| (os(k), os(v))))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sys::exec_in_new_session(&mut command, spec);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(None),
    };
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    let pidfd = sys::pidfd_open(pid);
    let mut streams = [
        Capture::new(child.stdout.take()),
        Capture::new(child.stderr.take()),
    ];
    let mut truncated = false;
    let mut status = None;
    // Grace deadline for open pipes after exit, and whether the group was
    // already killed because of them.
    let mut drain: Option<(Instant, bool)> = None;

    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    drain = Some((Instant::now() + READER_JOIN, false));
                }
                Ok(None) => {}
                Err(_) => {
                    terminate_process(pid);
                    let _ = child.wait();
                    return Ok(None);
                }
            }
        }
        let open = streams.iter().any(|s| s.pipe.is_some());
        if status.is_some() && !open {
            break;
        }
        let now = Instant::now();
        let wake = match drain {
            None if now >= deadline => {
                terminate_process(pid);
                let _ = child.wait();
                return Ok(None);
            }
            None => deadline,
            Some((until, killed)) if now >= until => {
                if killed {
                    break;
                }
                // Descendants still hold the pipes: kill the group and
                // give the pipes one more grace period to report EOF.
                terminate_process(pid);
                drain = Some((now + READER_JOIN, true));
                continue;
            }
            Some((until, _)) => until,
        };

        let mut fds: Vec<libc::pollfd> = Vec::with_capacity(3);
        let mut owners: Vec<usize> = Vec::with_capacity(2);
        for (i, stream) in streams.iter().enumerate() {
            if let Some(pipe) = &stream.pipe {
                fds.push(sys::pollfd(pipe.as_raw_fd()));
                owners.push(i);
            }
        }
        let mut wait_ms = wake.saturating_duration_since(now).as_millis() + 1;
        match (&pidfd, status.is_none()) {
            (Some(fd), true) => fds.push(sys::pollfd(fd.as_raw_fd())),
            // No pidfd: notice the exit with short sleeps.
            (None, true) => wait_ms = wait_ms.min(EXIT_POLL_MS),
            _ => {}
        }
        sys::poll(&mut fds, wait_ms.min(i32::MAX as u128) as i32);
        for (fd, &i) in fds.iter().zip(&owners) {
            if fd.revents != 0 && streams[i].read_available(limit) {
                truncated = true;
                terminate_process(pid);
            }
        }
    }

    let Some(status) = status else {
        return Ok(None);
    };
    let returncode = status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0));
    let [stdout, stderr] = streams;
    Ok(Some(CommandResult {
        args: cmd.to_vec(),
        returncode,
        stdout: stdout.data,
        stderr: stderr.data,
        output_truncated: truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    #[test]
    fn test_run_command_replaces_invalid_utf8_and_bounds_output() {
        // Python injected PATH/DEVCAP_PRIVATE_TEST into its own environment;
        // the probe must see only the minimal environment.
        let result = run_command(
            &sh("printf '\\377tool 1.2.3\\n'; printf '%s|%s' \"${DEVCAP_PRIVATE_TEST:-clean}\" \"$PATH\" >&2"),
            None,
            COMMAND_TIMEOUT_SECONDS,
            1024,
        )
        .unwrap()
        .unwrap();
        assert!(result.stdout_text().contains("\u{fffd}tool 1.2.3"));
        assert!(result.stderr_text().starts_with("clean|"));
        assert_eq!(result.stderr_text(), "clean|/bin:/usr/bin");

        // No variable of this (test) process may leak into the probe: the
        // child environment holds only the minimal variables plus what the
        // shell itself sets.
        let dumped = run_command(&sh("export -p"), None, COMMAND_TIMEOUT_SECONDS, 65536)
            .unwrap()
            .unwrap()
            .stdout_text();
        let shell_own = ["LANG", "LC_ALL", "PATH", "PWD", "OLDPWD", "SHLVL", "_"];
        let mut checked = 0;
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if shell_own.contains(&name.as_ref()) || PROBE_LOCATOR_VARS.contains(&name.as_ref()) {
                continue;
            }
            checked += 1;
            assert!(
                !dumped.contains(&format!(" {name}=")),
                "{name} leaked into the probe environment: {dumped}"
            );
        }
        assert!(checked > 0, "test process has no extra variables to check");

        let flooded = run_command(
            &sh("i=0; while [ $i -lt 100 ]; do printf '%0100d' 0; i=$((i+1)); done"),
            None,
            COMMAND_TIMEOUT_SECONDS,
            512,
        )
        .unwrap()
        .unwrap();
        assert!(flooded.output_truncated);
        assert!(flooded.stdout.len() <= 512);
    }

    #[test]
    fn test_run_command_rejects_invalid_timeout() {
        // Python also rejected `True`; a bool cannot be passed as f64 here.
        for timeout in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            let err = run_command(&sh("true"), None, timeout, 1024).unwrap_err();
            assert!(err.contains("timeout"), "{err}");
        }
    }

    #[test]
    fn run_command_times_out_and_kills_group() {
        let started = std::time::Instant::now();
        let result = run_command(&sh("sleep 30 & sleep 30"), None, 0.3, 1024).unwrap();
        assert!(result.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn run_command_reports_exit_codes_and_rejects_relative() {
        let result = run_command(&sh("exit 3"), None, 5.0, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(result.returncode, 3);
        assert!(run_command(&["sh".to_string()], None, 5.0, 1024).is_err());
        assert!(run_command(&sh("true"), None, 5.0, 0).is_err());
        assert!(run_command(&sh("true"), None, 5.0, MAX_COMMAND_OUTPUT_BYTES + 1).is_err());
        let missing = run_command(&["/nonexistent/devcap-probe".to_string()], None, 5.0, 1024);
        assert_eq!(missing, Ok(None));
    }

    #[test]
    fn exec_does_not_fall_back_to_shell_on_enoexec() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("noshebang");
        std::fs::write(&script, "echo ran\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = run_command(&[script.to_string_lossy().into_owned()], None, 5.0, 1024);
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn probe_runs_in_new_session() {
        let result = run_command(&sh("ps -o sid= -p $$; echo $$"), None, 5.0, 1024)
            .unwrap()
            .unwrap();
        let text = result.stdout_text();
        let mut lines = text.split_whitespace();
        if let (Some(sid), Some(pid)) = (lines.next(), lines.next()) {
            assert_eq!(sid, pid);
        }
    }

    #[test]
    fn minimal_environment_shape() {
        let none = |_: &str| None;
        let env = probe_environment_from("/usr/bin/git", false, none);
        assert_eq!(
            env,
            vec![
                ("LANG".to_string(), "C".to_string()),
                ("LC_ALL".to_string(), "C".to_string()),
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ]
        );
        assert_eq!(
            probe_environment_from("/bin/sh", false, none)[2].1,
            "/bin:/usr/bin"
        );
        assert_eq!(
            probe_environment_from("/x", false, none)[2].1,
            "/:/bin:/usr/bin"
        );
    }

    #[test]
    fn probe_environment_passes_absolute_locators_only() {
        let lookup = |name: &str| match name {
            "HOME" => Some("/home/dev".to_string()),
            "MISE_DATA_DIR" => Some("/opt/mise".to_string()),
            "PYENV_ROOT" => Some("relative/pyenv".to_string()),
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_string()),
            "GITHUB_TOKEN" | "LD_PRELOAD" | "NODE_OPTIONS" => Some("/secret".to_string()),
            _ => None,
        };
        let env = probe_environment_from("/usr/bin/git", false, lookup);
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["LANG", "LC_ALL", "PATH", "HOME", "MISE_DATA_DIR"]);
        assert_eq!(env[3].1, "/home/dev");
        let user = probe_environment_from("/usr/bin/systemctl", true, lookup);
        assert_eq!(user.last().unwrap().0, "XDG_RUNTIME_DIR");
    }

    #[test]
    fn probe_stdin_is_dev_null() {
        let result = run_command(
            &sh("readlink /proc/$$/fd/0; read line; echo \"read=$?\""),
            None,
            5.0,
            1024,
        )
        .unwrap()
        .unwrap();
        let text = result.stdout_text();
        if std::path::Path::new("/proc/self/fd/0").exists() {
            assert!(text.starts_with("/dev/null\n"), "{text}");
        }
        assert!(text.ends_with("read=1\n"), "{text}");
    }

    #[test]
    fn descendant_holding_pipes_does_not_hang_the_probe() {
        let started = std::time::Instant::now();
        let result = run_command(&sh("sleep 30 & echo tool 1.2.3"), None, 10.0, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(result.stdout_text(), "tool 1.2.3\n");
        assert_eq!(result.returncode, 0);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn both_streams_are_captured_and_signals_reported() {
        let result = run_command(
            &sh("echo out; echo err >&2; i=0; while [ $i -lt 50 ]; do printf '%0100d' 0 >&2; i=$((i+1)); done; sleep 5; echo done"),
            None,
            5.0,
            1024,
        )
        .unwrap()
        .unwrap();
        // stderr overflows its 1 KiB cap: the group is killed, stdout keeps
        // what it had, and the result is marked truncated.
        assert!(result.output_truncated);
        assert_eq!(result.stdout_text(), "out\n");
        assert_eq!(result.stderr.len(), 1024);
        assert!(result.stderr_text().starts_with("err\n"));
        // Killed by the group SIGKILL, or by SIGPIPE if its next write to
        // the closed stderr pipe comes first.
        assert!(
            [-libc::SIGKILL, -libc::SIGPIPE].contains(&result.returncode),
            "{}",
            result.returncode
        );

        let killed = run_command(&sh("echo partial; kill -TERM $$"), None, 5.0, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(killed.returncode, -libc::SIGTERM);
        assert_eq!(killed.stdout_text(), "partial\n");
    }
}
