"""Core scan engine — tool detection, version extraction, parallel scanning."""

from __future__ import annotations

import math
import os
import platform
import re
import shlex
import shutil
import signal
import socket
import subprocess
import threading
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import UTC, datetime
from numbers import Real
from pathlib import Path
from typing import BinaryIO

from .registry import REGISTRY, ToolDef
from .safe_text import clean_text

VERSION_RE = re.compile(r"v?(\d+\.\d+(?:\.\d+)?(?:[-+.]\w+)*)")
SERVICE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.@:+-]{0,127}$")
COMMAND_TIMEOUT_SECONDS = 5.0
MAX_COMMAND_TIMEOUT_SECONDS = 60.0
MAX_COMMAND_OUTPUT_BYTES = 64 * 1024
MAX_VERSION_BANNER_BYTES = 4096
MAX_VERSION_LINES = 64
MAX_SCAN_WORKERS = 64

# Paths containing these segments are considered vendored and skipped.
VENDORED_SEGMENTS = {"node_modules", ".venv", "venv", "__pypackages__", ".tox", ".nox"}
PROJECT_MARKERS = {".git", "pyproject.toml", "package.json", "Cargo.toml", "go.mod"}


@dataclass(slots=True, frozen=True)
class ToolResult:
    """Result of scanning a single tool."""

    name: str
    binary: str
    category: str
    found: bool
    version: str | None = None
    path: str | None = None
    version_source: str | None = None
    version_banner: str | None = None
    version_banner_truncated: bool = False

    def to_dict(self) -> dict:
        d: dict = {
            "name": self.name,
            "binary": self.binary,
            "category": self.category,
            "found": self.found,
        }
        if self.found:
            d["version"] = self.version
            d["path"] = self.path
            if self.version_source is not None:
                d["version_diagnostics"] = {
                    "source_stream": self.version_source,
                    "raw_banner": self.version_banner or "",
                    "truncated": self.version_banner_truncated,
                }
        return d


@dataclass(slots=True, frozen=True)
class ServiceResult:
    """Result of checking a systemd service."""

    name: str
    active: bool
    user_service: bool = False

    def to_dict(self) -> dict:
        return {"name": self.name, "active": self.active, "user_service": self.user_service}


@dataclass(slots=True, frozen=True)
class CommandResult:
    """Bounded subprocess result with explicit truncation state."""

    args: list[str]
    returncode: int
    stdout: str | bytes
    stderr: str | bytes
    output_truncated: bool = False


@dataclass(slots=True, frozen=True)
class VersionProbe:
    """Parsed version plus the bounded stream evidence used to derive it."""

    version: str | None
    source_stream: str | None = None
    raw_banner: str = ""
    truncated: bool = False


@dataclass(slots=True, frozen=True)
class ScanResult:
    """Top-level scan output."""

    hostname: str
    timestamp: str
    platform: str
    results: list[ToolResult] = field(default_factory=list)
    services: list[ServiceResult] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "hostname": self.hostname,
            "timestamp": self.timestamp,
            "platform": self.platform,
            "tools": [r.to_dict() for r in self.results],
            "services": [s.to_dict() for s in self.services],
        }


def redact_scan(scan: ScanResult, *, hostname: bool = True, paths: bool = True) -> ScanResult:
    """Return a copy with sensitive local metadata replaced."""
    results = [
        ToolResult(
            name=result.name,
            binary=result.binary,
            category=result.category,
            found=result.found,
            version=result.version,
            path="[redacted]" if paths and result.path else result.path,
            version_source=result.version_source,
            version_banner="[redacted]" if result.version_banner else result.version_banner,
            version_banner_truncated=result.version_banner_truncated,
        )
        for result in scan.results
    ]
    return ScanResult(
        hostname="[redacted]" if hostname else scan.hostname,
        timestamp=scan.timestamp,
        platform=scan.platform,
        results=results,
        services=scan.services,
    )


def _is_vendored(path: str) -> bool:
    """Return True if the path contains a vendored directory segment."""
    parts = path.replace("\\", "/").split("/")
    return bool(VENDORED_SEGMENTS.intersection(parts))


def _cwd_looks_like_project() -> bool:
    cwd = Path.cwd()
    return any((cwd / marker).exists() for marker in PROJECT_MARKERS)


def _is_project_local(path: str) -> bool:
    """Return True if a PATH match is relative or inside the current project."""
    if not os.path.isabs(path):
        return True
    if not _cwd_looks_like_project():
        return False
    try:
        resolved_path = Path(path).resolve()
        resolved_cwd = Path.cwd().resolve()
    except OSError:
        return False
    return resolved_path == resolved_cwd or resolved_cwd in resolved_path.parents


def _is_untrusted_path(path: str) -> bool:
    return _is_vendored(path) or _is_project_local(path)


def _canonical_executable(path: str) -> str | None:
    """Return an absolute executable path after validating its resolved target."""
    try:
        absolute = Path(os.path.abspath(path))
        resolved = absolute.resolve(strict=True)
    except OSError:
        return None
    if not resolved.is_file() or not os.access(absolute, os.X_OK):
        return None
    return str(absolute)


def _iter_binary_matches(candidate: str) -> list[str]:
    """Return executable matches for candidate in PATH order."""
    if os.path.dirname(candidate):
        path = shutil.which(candidate)
        canonical = _canonical_executable(path) if path else None
        return [canonical] if canonical else []

    matches: list[str] = []
    seen: set[str] = set()
    for directory in os.get_exec_path():
        directory = directory or "."
        path = shutil.which(os.path.join(directory, candidate))
        canonical = _canonical_executable(path) if path else None
        if canonical and canonical not in seen:
            matches.append(canonical)
            seen.add(canonical)
    return matches


def _find_binary(tool: ToolDef, *, include_vendored: bool = False) -> str | None:
    """Find the binary on PATH, trying aliases and skipping vendored paths by default."""
    candidates = [tool.binary, *tool.aliases]
    fallback: str | None = None

    for candidate in candidates:
        for path in _iter_binary_matches(candidate):
            if _is_untrusted_path(path):
                if include_vendored and fallback is None:
                    fallback = path
                continue
            if fallback is None:
                fallback = path
            return path

    return fallback


def _bounded_banner(output: str | bytes) -> tuple[str, bool]:
    """Return bounded decoded banner evidence and whether it was truncated."""
    if isinstance(output, bytes):
        text = output.decode("utf-8", errors="replace")
    else:
        text = output
    lines = text.splitlines(keepends=True)
    truncated = len(lines) > MAX_VERSION_LINES
    selected = "".join(lines[:MAX_VERSION_LINES])
    encoded = selected.encode("utf-8", errors="replace")
    if len(encoded) > MAX_VERSION_BANNER_BYTES:
        encoded = encoded[:MAX_VERSION_BANNER_BYTES]
        truncated = True
    return encoded.decode("utf-8", errors="replace"), truncated


def _analyze_banner(output: str | bytes) -> tuple[str | None, str | None, str, bool]:
    """Find a version anywhere in a bounded banner before descriptive fallback."""
    banner, truncated = _bounded_banner(output)
    descriptive: str | None = None
    for raw_line in banner.splitlines():
        line = clean_text(raw_line.strip())
        if not line:
            continue
        if descriptive is None:
            descriptive = clean_text(line, max_length=80)
        match = VERSION_RE.search(line)
        if match:
            return match.group(1), descriptive, banner, truncated
    return None, descriptive, banner, truncated


def extract_version(output: str | bytes) -> str | None:
    """Extract the first valid version from bounded output, then use a banner fallback."""
    version, descriptive, _banner, _truncated = _analyze_banner(output)
    return version or descriptive


def _terminate_process(proc: subprocess.Popen[bytes]) -> None:
    """Terminate a process and its process group when the platform supports it."""
    try:
        if os.name == "posix":
            os.killpg(proc.pid, signal.SIGKILL)
        else:
            proc.kill()
    except ProcessLookupError:
        return
    except OSError:
        proc.kill()


def _validate_timeout(timeout: object) -> float:
    if isinstance(timeout, bool) or not isinstance(timeout, Real):
        raise ValueError("timeout must be a finite positive number")
    value = float(timeout)
    if not math.isfinite(value) or value <= 0 or value > MAX_COMMAND_TIMEOUT_SECONDS:
        raise ValueError(
            f"timeout must be greater than 0 and at most {MAX_COMMAND_TIMEOUT_SECONDS:g} seconds"
        )
    return value


def _validate_output_limit(value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise ValueError("max_output_bytes must be a positive integer")
    if value > MAX_COMMAND_OUTPUT_BYTES:
        raise ValueError(f"max_output_bytes must be at most {MAX_COMMAND_OUTPUT_BYTES}")
    return value


def _minimal_probe_environment(executable: str, *, user_service: bool = False) -> dict[str, str]:
    """Build a minimal deterministic environment for external probes."""
    path_entries = [str(Path(executable).parent), *os.defpath.split(os.pathsep)]
    environment = {
        "LANG": "C",
        "LC_ALL": "C",
        "PATH": os.pathsep.join(dict.fromkeys(path_entries)),
    }
    if os.name == "nt":
        for name in ("SYSTEMROOT", "WINDIR"):
            if value := os.environ.get(name):
                environment[name] = value
    if user_service:
        for name in ("DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR"):
            if value := os.environ.get(name):
                environment[name] = value
    return environment


def _run_command(
    cmd: list[str],
    *,
    text: bool = True,
    env: dict[str, str] | None = None,
    timeout: float = COMMAND_TIMEOUT_SECONDS,
    max_output_bytes: int = MAX_COMMAND_OUTPUT_BYTES,
) -> CommandResult | None:
    """Run a command with bounded time/output and process-group cleanup."""
    timeout_value = _validate_timeout(timeout)
    output_limit = _validate_output_limit(max_output_bytes)
    if not cmd or not os.path.isabs(cmd[0]):
        raise ValueError("command executable must be an absolute path")
    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=False,
            env=env or _minimal_probe_environment(cmd[0]),
            start_new_session=os.name == "posix",
        )
    except OSError:
        return None

    stdout = bytearray()
    stderr = bytearray()
    output_truncated = threading.Event()

    def read_stream(stream: BinaryIO, target: bytearray) -> None:
        try:
            while chunk := stream.read(4096):
                remaining = output_limit - len(target)
                if remaining > 0:
                    target.extend(chunk[:remaining])
                if len(chunk) > remaining:
                    output_truncated.set()
                    _terminate_process(proc)
                    break
        except (OSError, ValueError):
            return

    readers = [
        threading.Thread(target=read_stream, args=(proc.stdout, stdout), daemon=True),
        threading.Thread(target=read_stream, args=(proc.stderr, stderr), daemon=True),
    ]
    for reader in readers:
        reader.start()
    try:
        returncode = proc.wait(timeout=timeout_value)
    except subprocess.TimeoutExpired:
        _terminate_process(proc)
        proc.wait()
        return None
    finally:
        for reader in readers:
            reader.join(timeout=0.25)
        if any(reader.is_alive() for reader in readers):
            _terminate_process(proc)
            for stream in (proc.stdout, proc.stderr):
                try:
                    stream.close()
                except OSError:
                    pass
            for reader in readers:
                reader.join(timeout=0.25)

    stdout_value: str | bytes
    stderr_value: str | bytes
    if text:
        stdout_value = bytes(stdout).decode("utf-8", errors="replace")
        stderr_value = bytes(stderr).decode("utf-8", errors="replace")
    else:
        stdout_value = bytes(stdout)
        stderr_value = bytes(stderr)
    return CommandResult(
        args=cmd,
        returncode=returncode,
        stdout=stdout_value,
        stderr=stderr_value,
        output_truncated=output_truncated.is_set(),
    )


def _get_version(
    tool: ToolDef, path: str, *, timeout: float = COMMAND_TIMEOUT_SECONDS
) -> VersionProbe:
    """Run a version probe and honor the configured stream with a true fallback."""
    try:
        flag_parts = shlex.split(tool.version_flag)
    except ValueError:
        return VersionProbe(None)
    cmd = [path, *flag_parts]
    result = _run_command(
        cmd,
        text=True,
        env=_minimal_probe_environment(path),
        timeout=timeout,
    )
    if result is None:
        return VersionProbe(None)

    preferred = tool.version_source
    fallback = "stdout" if preferred == "stderr" else "stderr"
    analyzed = {
        "stdout": _analyze_banner(result.stdout),
        "stderr": _analyze_banner(result.stderr),
    }
    for source in (preferred, fallback):
        version, _descriptive, banner, truncated = analyzed[source]
        if version is not None:
            return VersionProbe(
                version,
                source,
                banner,
                truncated or result.output_truncated,
            )
    for source in (preferred, fallback):
        _version, descriptive, banner, truncated = analyzed[source]
        if descriptive is not None:
            return VersionProbe(
                descriptive,
                source,
                banner,
                truncated or result.output_truncated,
            )
    return VersionProbe(None, truncated=result.output_truncated)


def scan_tool(
    tool: ToolDef,
    *,
    include_vendored: bool = False,
    timeout: float = COMMAND_TIMEOUT_SECONDS,
) -> ToolResult:
    """Scan a single tool for presence and version."""
    timeout_value = _validate_timeout(timeout)
    path = _find_binary(tool, include_vendored=include_vendored)
    if not path:
        return ToolResult(name=tool.name, binary=tool.binary, category=tool.category, found=False)

    probe = _get_version(tool, path, timeout=timeout_value)
    return ToolResult(
        name=tool.name,
        binary=tool.binary,
        category=tool.category,
        found=True,
        version=probe.version,
        path=path,
        version_source=probe.source_stream,
        version_banner=probe.raw_banner,
        version_banner_truncated=probe.truncated,
    )


def check_service(
    name: str, user: bool = False, *, timeout: float = COMMAND_TIMEOUT_SECONDS
) -> ServiceResult:
    """Check if a systemd service is active."""
    timeout_value = _validate_timeout(timeout)
    if platform.system() != "Linux":
        return ServiceResult(name=name, active=False, user_service=user)
    if not SERVICE_NAME_RE.fullmatch(name):
        return ServiceResult(name=name, active=False, user_service=user)
    systemctl = _find_binary(ToolDef("systemctl", "systemctl", "System"))
    if systemctl is None:
        return ServiceResult(name=name, active=False, user_service=user)
    cmd = [systemctl]
    if user:
        cmd.append("--user")
    cmd.extend(["is-active", "--quiet", "--", name])
    result = _run_command(
        cmd,
        text=False,
        env=_minimal_probe_environment(systemctl, user_service=user),
        timeout=timeout_value,
    )
    if result is None:
        return ServiceResult(name=name, active=False, user_service=user)
    return ServiceResult(name=name, active=result.returncode == 0, user_service=user)


def scan_tools(
    tools: list[ToolDef] | None = None,
    services: list[tuple[str, bool]] | None = None,
    parallel: bool = True,
    max_workers: int = 16,
    include_vendored: bool = False,
    timeout: float = COMMAND_TIMEOUT_SECONDS,
) -> ScanResult:
    """Scan all tools and services, returning a ScanResult.

    Args:
        tools: List of ToolDefs to scan. Defaults to full registry.
        services: List of (service_name, is_user_service) tuples. Defaults to empty.
        parallel: Use ThreadPoolExecutor for scanning. Default True.
        max_workers: Thread pool size. Default 16.
        include_vendored: Allow executables from vendored/project-local PATH segments.
        timeout: Maximum seconds for each external probe.
    """
    timeout_value = _validate_timeout(timeout)
    if isinstance(max_workers, bool) or not isinstance(max_workers, int):
        raise ValueError("max_workers must be a positive integer")
    if max_workers < 1 or max_workers > MAX_SCAN_WORKERS:
        raise ValueError(f"max_workers must be between 1 and {MAX_SCAN_WORKERS}")
    if tools is None:
        tools = list(REGISTRY.values())
    if services is None:
        services = []

    if parallel and len(tools) > 1:
        workers = min(max_workers, len(tools))
        if workers < 1:
            workers = 1
        with ThreadPoolExecutor(max_workers=workers) as pool:
            tool_results = list(
                pool.map(
                    lambda tool: scan_tool(
                        tool,
                        include_vendored=include_vendored,
                        timeout=timeout_value,
                    ),
                    tools,
                )
            )
    else:
        tool_results = [
            scan_tool(t, include_vendored=include_vendored, timeout=timeout_value) for t in tools
        ]

    service_results = [check_service(name, user, timeout=timeout_value) for name, user in services]

    return ScanResult(
        hostname=socket.gethostname(),
        timestamp=datetime.now(UTC).isoformat(timespec="seconds"),
        platform=f"{platform.system()} {platform.release()}",
        results=tool_results,
        services=service_results,
    )
