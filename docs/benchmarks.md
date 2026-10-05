# Benchmarks: Python 0.2.1 vs Rust 0.3.0

Measured on 2026-10-05 on `greyarch`: Linux 7.2.5 x86_64, 12 cores, 16 GB RAM.
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
