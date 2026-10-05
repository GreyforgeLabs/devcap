//! `devcap` command-line entry point.

use std::io::Write;

fn main() {
    use std::os::unix::ffi::OsStrExt;
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| devcap::pycompat::fsdecode(a.as_bytes()))
        .collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut err = stderr.lock();
    let code = devcap::cli::main_with(&argv, &mut out, &mut err);
    let code = if out.flush().is_err() { 120 } else { code };
    drop(out);
    std::process::exit(code);
}
