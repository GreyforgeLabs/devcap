//! Thin wrappers over the few POSIX calls std does not expose.
//!
//! This is the only module with `unsafe`. Each block is a direct libc call
//! with valid, NUL-terminated arguments; see the SAFETY notes.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// `os.getuid()`.
pub fn getuid() -> u32 {
    // SAFETY: getuid(2) has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// `os.uname()` fields devcap reports.
pub struct Uname {
    pub sysname: String,
    pub nodename: String,
    pub release: String,
}

fn field(raw: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = raw
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    crate::pycompat::fsdecode(&bytes)
}

/// `os.uname()`.
pub fn uname() -> Uname {
    // SAFETY: utsname is a plain C struct of char arrays; zeroed is valid.
    let mut buf: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `buf` is a valid, writable utsname.
    let rc = unsafe { libc::uname(&mut buf) };
    if rc != 0 {
        return Uname {
            sysname: String::new(),
            nodename: String::new(),
            release: String::new(),
        };
    }
    Uname {
        sysname: field(&buf.sysname),
        nodename: field(&buf.nodename),
        release: field(&buf.release),
    }
}

/// `socket.gethostname()`.
pub fn hostname() -> String {
    let mut buf = [0 as libc::c_char; 256];
    // SAFETY: the buffer is writable for `len - 1` bytes, leaving a NUL.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr(), buf.len() - 1) };
    if rc != 0 {
        return uname().nodename;
    }
    field(&buf)
}

/// `os.killpg(pgid, SIGKILL)`; returns the raw errno on failure.
pub fn killpg_sigkill(pgid: u32) -> Result<(), i32> {
    // SAFETY: killpg(2) takes plain integers.
    let rc = unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGKILL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}

/// `os.kill(pid, SIGKILL)` (Popen.kill fallback).
pub fn kill_sigkill(pid: u32) {
    // SAFETY: kill(2) takes plain integers.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// `os.access(path, X_OK)`.
pub fn access_x(path: &Path) -> bool {
    access(path, libc::X_OK)
}

/// `os.access(path, F_OK | X_OK)`.
pub fn access_fx(path: &Path) -> bool {
    access(path, libc::F_OK | libc::X_OK)
}

fn access(path: &Path, mode: libc::c_int) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string for the call.
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

/// CPython's `OSError.strerror` text for an I/O error.
pub fn strerror(err: &io::Error) -> String {
    let text = err.to_string();
    match text.rfind(" (os error ") {
        Some(i) => text[..i].to_string(),
        None => text,
    }
}

/// Pre-built `execve` arguments so the post-fork child never allocates.
pub struct ExecSpec {
    /// Upper bound for the fallback close-on-exec sweep (see
    /// [`mark_inherited_cloexec`]), computed before the fork.
    fd_sweep_limit: libc::c_int,
    path: CString,
    _argv: Vec<CString>,
    _envp: Vec<CString>,
    argv_ptrs: Vec<*const libc::c_char>,
    envp_ptrs: Vec<*const libc::c_char>,
}

// SAFETY: the raw pointers point into the owned CStrings of the same struct,
// which are never mutated after construction.
unsafe impl Send for ExecSpec {}
unsafe impl Sync for ExecSpec {}

impl ExecSpec {
    /// Build from an argv (argv[0] is the executable path) and env pairs.
    pub fn new(argv: &[String], env: &[(String, String)]) -> Option<ExecSpec> {
        use crate::pycompat::fsencode;
        let path = CString::new(fsencode(argv.first()?)).ok()?;
        let argv_c: Vec<CString> = argv
            .iter()
            .map(|a| CString::new(fsencode(a)).ok())
            .collect::<Option<_>>()?;
        let envp_c: Vec<CString> = env
            .iter()
            .map(|(k, v)| CString::new(fsencode(&format!("{k}={v}"))).ok())
            .collect::<Option<_>>()?;
        let mut argv_ptrs: Vec<*const libc::c_char> = argv_c.iter().map(|c| c.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());
        let mut envp_ptrs: Vec<*const libc::c_char> = envp_c.iter().map(|c| c.as_ptr()).collect();
        envp_ptrs.push(std::ptr::null());
        // SAFETY: sysconf(3) takes a plain integer name.
        let open_max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        let fd_sweep_limit = if open_max <= 0 {
            FD_SWEEP_CAP
        } else {
            open_max.min(libc::c_long::from(FD_SWEEP_CAP)) as libc::c_int
        };
        Some(ExecSpec {
            fd_sweep_limit,
            path,
            _argv: argv_c,
            _envp: envp_c,
            argv_ptrs,
            envp_ptrs,
        })
    }
}

/// Largest descriptor number the fallback sweep visits.
const FD_SWEEP_CAP: libc::c_int = 65_536;

/// Mark every descriptor above stderr close-on-exec, so the probe inherits
/// only stdin/stdout/stderr (CPython's `close_fds=True`). Marking instead of
/// closing keeps std's own exec-error pipe working until `execve` succeeds.
/// Runs post-fork: only async-signal-safe syscalls, no allocation.
fn mark_inherited_cloexec(sweep_limit: libc::c_int) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: close_range(2) with CLOSE_RANGE_CLOEXEC only changes
        // descriptor flags; the arguments are plain integers.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3 as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        };
        if rc == 0 {
            return;
        }
    }
    // Older kernels and other Unix systems: set FD_CLOEXEC one by one
    // (EBADF for unused numbers is ignored).
    for fd in 3..sweep_limit {
        // SAFETY: fcntl(2) F_SETFD on an integer descriptor.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// Configure `cmd` to behave like `subprocess.Popen(..., start_new_session=True)`
/// with CPython's `execve` and `close_fds=True` semantics: the child calls
/// `setsid()`, marks inherited descriptors close-on-exec, and then
/// `execve()`s the absolute path directly (no `/bin/sh` fallback on ENOEXEC,
/// which glibc's `execvp` would otherwise perform).
pub fn exec_in_new_session(cmd: &mut std::process::Command, spec: std::sync::Arc<ExecSpec>) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs in the forked child before exec. It only makes
    // async-signal-safe syscalls (setsid, close_range/fcntl, execve) on memory
    // allocated before the fork, and returns the errno so std reports a spawn
    // failure.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            mark_inherited_cloexec(spec.fd_sweep_limit);
            libc::execve(
                spec.path.as_ptr(),
                spec.argv_ptrs.as_ptr(),
                spec.envp_ptrs.as_ptr(),
            );
            Err(io::Error::last_os_error())
        });
    }
}
