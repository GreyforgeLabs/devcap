# Benchmarks: Python 0.2.1 vs Rust 0.3.0

Measured on 2026-10-05 on the reference host: Linux 7.2.5 x86_64, 12 cores, 16 GB RAM.
Python 3.14.7 ran the 0.2.1 sources (`PYTHONPATH=src python3 -m devcap`).
Rust 1.98.1 built the 0.3.0 release profile (`cargo build --release --locked`:
`opt-level = 3`, fat LTO, one codegen unit, stripped, `panic = "abort"`).
Both versions scanned the same host with the same `PATH`, and the
differential suite confirmed byte-identical output for every scan below.

## Method

Wall time is the median of 21 runs. `hyperfine` is not installed on this
host, so each run was timed with `date +%s%N` around the command and stdout
and stderr went to `/dev/null`:

```bash
for i in $(seq 21); do
  s=$(date +%s%N); "$@" >/dev/null 2>&1; e=$(date +%s%N)
  echo $(( (e - s) / 1000 ))
done | sort -n | sed -n 11p   # median, microseconds
```

GNU `time` (`/usr/bin/time`) is not installed either. Peak RSS was therefore
measured with a 30-line C wrapper that does what `time -v` does: it forks,
execs the command, and reports `ru_maxrss` from `wait4`. While the command
runs, the wrapper also polls the target's own `VmHWM` from `/proc/<pid>/status`
after exec. Each RSS figure is the median of 7 runs.

## Startup and command latency

| Command | Python 0.2.1 | Rust 0.3.0 | Speed-up |
|---|---:|---:|---:|
| `devcap --help` (cold start) | 76.4 ms | 2.2 ms | 35x |
| `devcap list-profiles` | 89.9 ms | 2.8 ms | 32x |
| `devcap scan --profile python-dev --format json` (12 tools) | 101.6 ms | 15.6 ms | 6.5x |
| `devcap scan --format json` (full profile, 103 tools + 2 services) | 755.2 ms | 623.0 ms | 1.2x |
| `devcap scan --format json --max-workers 64` | 751.8 ms | 617.6 ms | 1.2x |
| `devcap scan --format json --no-parallel` | 1564.7 ms | 1196.6 ms | 1.3x |

Most of a full scan is spent waiting on external version probes. On this
host `pnpm --version` alone takes about 580 ms, so no scheduler can bring a
full scan below that. With the default 16 workers, Rust finishes about 40 ms
after its slowest probe. Python takes about 175 ms longer than that probe,
mostly interpreter startup plus thread and GIL overhead. Smaller profiles
show the startup savings more clearly.

## Peak memory

| Command | Python own peak (`VmHWM`) | Rust own peak (`VmHWM`) | `time -v`-style `ru_maxrss` (Python / Rust) |
|---|---:|---:|---:|
| `devcap --help` | 19.7 MiB | 2.0 MiB | 19.1 / 2.4 MiB |
| `devcap list-profiles` | 20.3 MiB | 2.8 MiB | 20.0 / 2.8 MiB |
| `devcap scan --profile python-dev --format json` | 21.1 MiB | 3.4 MiB | 24.7 / 24.5 MiB |
| `devcap scan --format json` | 22.4 MiB | 4.3 MiB | 156.4 / 156.3 MiB |

For scans, the `wait4` figure is set by the largest probe child, not by
devcap itself, because `ru_maxrss` covers waited-for descendants.
That is why the two versions match in that column. The own-process `VmHWM`
column measures devcap itself.

## Verification re-measurement

An independent check on 2026-10-05 (01:04 to 01:19 local) rebuilt the release
binary and re-ran these commands with a small C wrapper (fork, exec,
`clock_gettime`, `wait4`, optional `VmHWM` polling). The host was heavily
loaded by unrelated jobs (load average 11 to 27 on 12 cores), so wall times
were inflated and noisy, especially for Python and for the full scans. The
results:

- Startup: Rust took 1.4 ms for `--help` and 2.1 to 2.4 ms for
  `list-profiles`, consistent with the table above. Python took 101 to 202 ms
  for `--help` because of the load.
- Full scans: Rust was faster than Python in every run, but the absolute times
  varied too much to replace the table's figures.
- Peak memory matched the table. devcap's own `VmHWM` was 3.4 MiB for the
  python-dev scan and 4.4 MiB for the full scan, against 21.1 and 22.3 MiB for
  Python. `ru_maxrss` was 2.4 MiB for Rust `--help`, and for a full scan both
  versions reported about 156 MiB, set by probe children.
- Binary size after the verification fixes is 750,944 B. These fixes are
  `surrogateescape`-style handling of non-UTF-8 paths and close-on-exec for
  inherited descriptors.

## Installed footprint

| | Python 0.2.1 | Rust 0.3.0 |
|---|---:|---:|
| devcap itself | 141,200 B package directory (60,330 B across 15 `.py`/`.toml` files) | 750,944 B single stripped PIE binary (dynamically linked to glibc and libgcc_s only), profiles embedded |
| Runtime dependency | CPython 3.11+ (here: 6.4 MB `libpython3.14.so` + 844 MB `/usr/lib/python3.14` tree on this host, stdlib plus distro site-packages) | none beyond the system C library |

The Rust binary is larger than the Python sources on their own. Unlike the
Python package, it needs no interpreter. It also bundles the Unicode property
tables that make version parsing and text cleanup match CPython exactly.

## 0.3.1 probe runner (2026-10-14)

Measured on the reference host (Linux 7.2.5 x86_64, 12 cores, load average 16 to 20
from unrelated jobs) with Rust 1.98.1 and the same release profile. Both
binaries ran interleaved (A, B, A, B, ...) inside
`systemd-run --user --scope -p MemoryMax=3G -p CPUQuota=300% nice -n 10`.
A small C wrapper forked, exec'd and timed each run. It polled the target's
`VmHWM` and read devcap's own `utime + stime` from `/proc/<pid>/stat` while
the exited process was still a zombie (`waitid(WNOWAIT)`), so probe children
are excluded. Wall time and `VmHWM` are medians; own CPU is a mean, because
the kernel reports it in 10 ms ticks. Thread and `execve` counts come from a
ptrace wrapper that follows the whole process tree.

| Command | Runs | 0.3.0 wall | 0.3.1 wall | 0.3.0 own CPU | 0.3.1 own CPU | 0.3.0 own `VmHWM` | 0.3.1 own `VmHWM` |
|---|---:|---:|---:|---:|---:|---:|---:|
| 100 fast probes (`true --version`), 16 workers | 21 | 125.3 ms | 79.2 ms | 135.7 ms | 62.4 ms | 4.35 MiB | 3.83 MiB |
| 100 fast probes, `--no-parallel` | 21 | 157.5 ms | 133.8 ms | 59.5 ms | 30.5 ms | 3.38 MiB | 3.13 MiB |
| `devcap --help` | 21 | 2.5 ms | 2.1 ms | — | — | 2.14 MiB | 1.96 MiB |
| `scan` of 100 tools not on `PATH` (lookup only) | 21 | 12.6 ms | 10.1 ms | — | — | 3.40 MiB | 3.33 MiB |
| `scan --profile python-dev --format json` | 15 | 33.1 ms | 31.6 ms | — | — | 3.39 MiB | 3.23 MiB |
| `scan --format json` (full, `HOME` unset so both run identical probes) | 11 | 1251.3 ms | 1311.8 ms | 68.2 ms | 38.2 ms | 4.41 MiB | 3.75 MiB |

Threads started by devcap during a full scan: 172 in 0.3.0 (16 workers plus
three helpers for each of 52 probes), 16 in 0.3.1. A full-scan wall time is
still set by the slowest probe (`pnpm --version`, 0.6 to 0.85 s here), so the
two full-scan medians fall within this host's run-to-run noise. On this host,
three `~/.local/bin` wrappers run `mise`, which needs `HOME`. With `HOME`
passed through, they report their real versions (`gh` 2.101.0 and
`claude` 2.1.294, instead of mise's own 2026.9.14) after doing their actual
work. That adds 8 `execve` calls (61 to 69), and the full-scan median with
`HOME` set rose from 1068.8 ms to 1258.5 ms.

The stripped release binary is 707,472 B (0.3.0: 750,944 B).
