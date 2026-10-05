//! Bounded subprocess execution with process-group cleanup
//! (port of `scanner._run_command`).

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::sys;

pub const COMMAND_TIMEOUT_SECONDS: f64 = 5.0;
pub const MAX_COMMAND_TIMEOUT_SECONDS: f64 = 60.0;
pub const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

const READ_CHUNK: usize = 4096;
const READER_JOIN: Duration = Duration::from_millis(250);
const HELPER_STACK: usize = 64 * 1024;

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

/// Build a minimal deterministic environment for external probes.
pub fn minimal_probe_environment(executable: &str, user_service: bool) -> Vec<(String, String)> {
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
    if user_service {
        for name in ["DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR"] {
            if let Some(value) = std::env::var_os(name)
                && !value.is_empty()
            {
                use std::os::unix::ffi::OsStrExt;
                env.push((
                    name.to_string(),
                    crate::pycompat::fsdecode(value.as_bytes()),
                ));
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

/// `BufferedReader.read(4096)`: block until the chunk is full or EOF.
fn read_chunk(stream: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                if filled == 0 {
                    return Err(e);
                }
                break;
            }
        }
    }
    Ok(filled)
}

fn spawn_reader(
    mut stream: impl Read + Send + 'static,
    target: Arc<Mutex<Vec<u8>>>,
    truncated: Arc<AtomicBool>,
    limit: usize,
    pid: u32,
) -> mpsc::Receiver<()> {
    let (done_tx, done_rx) = mpsc::channel();
    let body = move || {
        let mut buf = [0u8; READ_CHUNK];
        loop {
            let n = match read_chunk(&mut stream, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut data = target.lock().unwrap_or_else(|p| p.into_inner());
            let remaining = limit.saturating_sub(data.len());
            if remaining > 0 {
                data.extend_from_slice(&buf[..n.min(remaining)]);
            }
            drop(data);
            if n > remaining {
                truncated.store(true, Ordering::SeqCst);
                terminate_process(pid);
                break;
            }
        }
        let _ = done_tx.send(());
    };
    if thread::Builder::new()
        .stack_size(HELPER_STACK)
        .spawn(body)
        .is_err()
    {
        // Could not start a reader: report it as already finished.
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(());
        return rx;
    }
    done_rx
}

/// Run a command with bounded time/output and process-group cleanup.
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
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sys::exec_in_new_session(&mut command, Arc::new(spec));
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(None),
    };
    let pid = child.id();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let truncated = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::with_capacity(2);
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(
            out,
            stdout.clone(),
            truncated.clone(),
            limit,
            pid,
        ));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(
            err,
            stderr.clone(),
            truncated.clone(),
            limit,
            pid,
        ));
    }

    let (wait_tx, wait_rx) = mpsc::channel();
    let waiter = thread::Builder::new()
        .stack_size(HELPER_STACK)
        .spawn(move || {
            let _ = wait_tx.send(child.wait());
        });
    if waiter.is_err() {
        terminate_process(pid);
        return Ok(None);
    }

    let status = match wait_rx.recv_timeout(Duration::from_secs_f64(timeout)) {
        Ok(Ok(status)) => status,
        Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return Ok(None),
        Err(RecvTimeoutError::Timeout) => {
            terminate_process(pid);
            let _ = wait_rx.recv();
            return Ok(None);
        }
    };

    let stuck: Vec<&mpsc::Receiver<()>> = readers
        .iter()
        .filter(|rx| rx.recv_timeout(READER_JOIN).is_err())
        .collect();
    if !stuck.is_empty() {
        terminate_process(pid);
        for rx in stuck {
            let _ = rx.recv_timeout(READER_JOIN);
        }
    }

    use std::os::unix::process::ExitStatusExt;
    let returncode = status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0));
    let take = |buf: &Arc<Mutex<Vec<u8>>>| buf.lock().unwrap_or_else(|p| p.into_inner()).clone();
    Ok(Some(CommandResult {
        args: cmd.to_vec(),
        returncode,
        stdout: take(&stdout),
        stderr: take(&stderr),
        output_truncated: truncated.load(Ordering::SeqCst),
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
            if shell_own.contains(&name.as_ref()) {
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
        let env = minimal_probe_environment("/usr/bin/git", false);
        assert_eq!(
            env,
            vec![
                ("LANG".to_string(), "C".to_string()),
                ("LC_ALL".to_string(), "C".to_string()),
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ]
        );
        assert_eq!(
            minimal_probe_environment("/bin/sh", false)[2].1,
            "/bin:/usr/bin"
        );
        assert_eq!(
            minimal_probe_environment("/x", false)[2].1,
            "/:/bin:/usr/bin"
        );
    }
}
