"""Core scan engine — tool detection, version extraction, parallel scanning."""

from __future__ import annotations

import os
import platform
import re
import shlex
import shutil
import signal
import socket
import subprocess
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

from .registry import REGISTRY, ToolDef
from .safe_text import clean_text

VERSION_RE = re.compile(r"v?(\d+\.\d+(?:\.\d+)?(?:[-+.]\w+)*)")
SERVICE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.@:+-]{0,127}$")
COMMAND_TIMEOUT_SECONDS = 5

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


def _iter_binary_matches(candidate: str) -> list[str]:
    """Return executable matches for candidate in PATH order."""
    if os.path.dirname(candidate):
        path = shutil.which(candidate)
        return [path] if path else []

    matches: list[str] = []
    seen: set[str] = set()
    for directory in os.get_exec_path():
        directory = directory or "."
        path = shutil.which(os.path.join(directory, candidate))
        if path and path not in seen:
            matches.append(path)
            seen.add(path)
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


def extract_version(output: str) -> str | None:
    """Extract a version string from command output."""
    for line in output.splitlines():
        line = clean_text(line.strip(), max_length=80)
        if not line:
            continue
        match = VERSION_RE.search(line)
        if match:
            return match.group(1)
        return line
    return None


def _terminate_process(proc: subprocess.Popen[str]) -> None:
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


def _run_command(
    cmd: list[str],
    *,
    text: bool = True,
    env: dict[str, str] | None = None,
    timeout: int = COMMAND_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[str] | None:
    """Run a bounded command, cleaning up process groups on timeout."""
    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=text,
            env=env,
            start_new_session=os.name == "posix",
        )
        try:
            stdout, stderr = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            _terminate_process(proc)
            proc.communicate()
            return None
        return subprocess.CompletedProcess(cmd, proc.returncode, stdout, stderr)
    except OSError:
        return None


def _get_version(tool: ToolDef, path: str) -> str | None:
    """Run the version command and extract the version string."""
    try:
        flag_parts = shlex.split(tool.version_flag)
    except ValueError:
        return None
    cmd = [path, *flag_parts]
    result = _run_command(cmd, text=True, env={**os.environ, "LC_ALL": "C"})
    if result is None:
        return None
    output = result.stderr if tool.version_source == "stderr" else result.stdout
    if not output and tool.version_source != "stderr":
        output = result.stderr
    return extract_version(output)


def scan_tool(tool: ToolDef, *, include_vendored: bool = False) -> ToolResult:
    """Scan a single tool for presence and version."""
    path = _find_binary(tool, include_vendored=include_vendored)
    if not path:
        return ToolResult(name=tool.name, binary=tool.binary, category=tool.category, found=False)

    version = _get_version(tool, path)
    return ToolResult(
        name=tool.name,
        binary=tool.binary,
        category=tool.category,
        found=True,
        version=version,
        path=path,
    )


def check_service(name: str, user: bool = False) -> ServiceResult:
    """Check if a systemd service is active."""
    if platform.system() != "Linux":
        return ServiceResult(name=name, active=False, user_service=user)
    if not SERVICE_NAME_RE.fullmatch(name):
        return ServiceResult(name=name, active=False, user_service=user)
    cmd = ["systemctl"]
    if user:
        cmd.append("--user")
    cmd.extend(["is-active", "--quiet", "--", name])
    result = _run_command(cmd, text=False)
    if result is None:
        return ServiceResult(name=name, active=False, user_service=user)
    return ServiceResult(name=name, active=result.returncode == 0, user_service=user)


def scan_tools(
    tools: list[ToolDef] | None = None,
    services: list[tuple[str, bool]] | None = None,
    parallel: bool = True,
    max_workers: int = 16,
    include_vendored: bool = False,
) -> ScanResult:
    """Scan all tools and services, returning a ScanResult.

    Args:
        tools: List of ToolDefs to scan. Defaults to full registry.
        services: List of (service_name, is_user_service) tuples. Defaults to empty.
        parallel: Use ThreadPoolExecutor for scanning. Default True.
        max_workers: Thread pool size. Default 16.
        include_vendored: Allow executables from vendored/project-local PATH segments.
    """
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
                pool.map(lambda tool: scan_tool(tool, include_vendored=include_vendored), tools)
            )
    else:
        tool_results = [scan_tool(t, include_vendored=include_vendored) for t in tools]

    service_results = [check_service(name, user) for name, user in services]

    return ScanResult(
        hostname=socket.gethostname(),
        timestamp=datetime.now(UTC).isoformat(timespec="seconds"),
        platform=f"{platform.system()} {platform.release()}",
        results=tool_results,
        services=service_results,
    )
