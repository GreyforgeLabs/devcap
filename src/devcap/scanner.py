"""Core scan engine — tool detection, version extraction, parallel scanning."""

from __future__ import annotations

import os
import platform
import re
import shutil
import socket
import subprocess
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import UTC, datetime

from .registry import REGISTRY, ToolDef

VERSION_RE = re.compile(r"v?(\d+\.\d+(?:\.\d+)?(?:[-+.]\w+)*)")

# Paths containing these segments are considered vendored and skipped.
VENDORED_SEGMENTS = {"node_modules", ".venv", "venv", "__pypackages__"}


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


def _is_vendored(path: str) -> bool:
    """Return True if the path contains a vendored directory segment."""
    parts = path.replace("\\", "/").split("/")
    return bool(VENDORED_SEGMENTS.intersection(parts))


def _find_binary(tool: ToolDef) -> str | None:
    """Find the binary on PATH, trying aliases and skipping vendored paths."""
    candidates = [tool.binary, *tool.aliases]
    for candidate in candidates:
        path = shutil.which(candidate)
        if path and not _is_vendored(path):
            return path
    # Fallback: accept vendored path if nothing else found
    for candidate in candidates:
        path = shutil.which(candidate)
        if path:
            return path
    return None


def extract_version(output: str) -> str | None:
    """Extract a version string from command output."""
    for line in output.splitlines():
        line = line.strip()
        if not line:
            continue
        match = VERSION_RE.search(line)
        if match:
            return match.group(1)
        # Fallback: return first non-empty line, truncated
        if len(line) > 80:
            return line[:77] + "..."
        return line
    return None


def _get_version(tool: ToolDef, path: str) -> str | None:
    """Run the version command and extract the version string."""
    flag_parts = tool.version_flag.split()
    cmd = [path, *flag_parts]
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=5,
            env={**os.environ, "LC_ALL": "C"},
        )
        output = result.stderr if tool.version_source == "stderr" else result.stdout
        if not output and tool.version_source != "stderr":
            output = result.stderr
        return extract_version(output)
    except (subprocess.TimeoutExpired, OSError):
        return None


def scan_tool(tool: ToolDef) -> ToolResult:
    """Scan a single tool for presence and version."""
    path = _find_binary(tool)
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
    cmd = ["systemctl"]
    if user:
        cmd.append("--user")
    cmd.extend(["is-active", "--quiet", name])
    try:
        result = subprocess.run(cmd, capture_output=True, timeout=5)
        return ServiceResult(name=name, active=result.returncode == 0, user_service=user)
    except (subprocess.TimeoutExpired, OSError):
        return ServiceResult(name=name, active=False, user_service=user)


def scan_tools(
    tools: list[ToolDef] | None = None,
    services: list[tuple[str, bool]] | None = None,
    parallel: bool = True,
    max_workers: int = 16,
) -> ScanResult:
    """Scan all tools and services, returning a ScanResult.

    Args:
        tools: List of ToolDefs to scan. Defaults to full registry.
        services: List of (service_name, is_user_service) tuples. Defaults to empty.
        parallel: Use ThreadPoolExecutor for scanning. Default True.
        max_workers: Thread pool size. Default 16.
    """
    if tools is None:
        tools = list(REGISTRY.values())
    if services is None:
        services = []

    if parallel and len(tools) > 1:
        with ThreadPoolExecutor(max_workers=max_workers) as pool:
            tool_results = list(pool.map(scan_tool, tools))
    else:
        tool_results = [scan_tool(t) for t in tools]

    service_results = [check_service(name, user) for name, user in services]

    return ScanResult(
        hostname=socket.gethostname(),
        timestamp=datetime.now(UTC).isoformat(timespec="seconds"),
        platform=f"{platform.system()} {platform.release()}",
        results=tool_results,
        services=service_results,
    )
