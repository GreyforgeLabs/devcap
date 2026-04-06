"""Output formatters — text, json, markdown."""

from __future__ import annotations

import json

from .registry import CATEGORIES
from .scanner import ScanResult, ToolResult


def _group_by_category(results: list[ToolResult]) -> dict[str, list[ToolResult]]:
    """Group tool results by category, preserving CATEGORIES order."""
    groups: dict[str, list[ToolResult]] = {}
    for r in results:
        groups.setdefault(r.category, []).append(r)
    ordered: dict[str, list[ToolResult]] = {}
    for cat in CATEGORIES:
        if cat in groups:
            ordered[cat] = groups.pop(cat)
    # Any remaining custom categories
    for cat, tools in groups.items():
        ordered[cat] = tools
    return ordered


def format_text(scan: ScanResult) -> str:
    """Format scan results as human-readable columnar text."""
    lines = [
        f"devcap scan — {scan.hostname} — {scan.timestamp}",
        f"Platform: {scan.platform}",
        "",
    ]

    groups = _group_by_category(scan.results)
    for category, tools in groups.items():
        found = [t for t in tools if t.found]
        missing = [t for t in tools if not t.found]

        lines.append(f"=== {category} ===")
        if found:
            for t in found:
                version = t.version or "?"
                lines.append(f"  {t.name:<16} {version:<20} {t.path}")
        if missing:
            lines.append("  Missing:")
            for t in missing:
                lines.append(f"    {t.name}")
        lines.append("")

    if scan.services:
        lines.append("=== Services ===")
        for svc in scan.services:
            status = "running" if svc.active else "stopped"
            suffix = " (user)" if svc.user_service else ""
            lines.append(f"  [{status}] {svc.name}{suffix}")
        lines.append("")

    found_count = sum(1 for r in scan.results if r.found)
    total_count = len(scan.results)
    lines.append(f"Found {found_count}/{total_count} tools")

    return "\n".join(lines)


def format_json(scan: ScanResult) -> str:
    """Format scan results as JSON."""
    return json.dumps(scan.to_dict(), indent=2)


def format_markdown(scan: ScanResult) -> str:
    """Format scan results as markdown tables."""
    lines = [
        f"# Development Environment — {scan.hostname}",
        "",
        f"> Scanned: {scan.timestamp} | Platform: {scan.platform}",
        "",
    ]

    groups = _group_by_category(scan.results)
    for category, tools in groups.items():
        found = [t for t in tools if t.found]
        missing = [t for t in tools if not t.found]

        lines.append(f"## {category}")
        lines.append("")
        if found:
            lines.append("| Tool | Version | Path |")
            lines.append("|------|---------|------|")
            for t in found:
                version = t.version or "?"
                lines.append(f"| {t.name} | {version} | {t.path} |")
            lines.append("")
        if missing:
            missing_names = ", ".join(f"`{t.name}`" for t in missing)
            lines.append(f"**Not installed**: {missing_names}")
            lines.append("")

    if scan.services:
        lines.append("## Services")
        lines.append("")
        lines.append("| Service | Status |")
        lines.append("|---------|--------|")
        for svc in scan.services:
            status = "running" if svc.active else "stopped"
            suffix = " (user)" if svc.user_service else ""
            lines.append(f"| {svc.name}{suffix} | {status} |")
        lines.append("")

    return "\n".join(lines)


FORMATTERS = {
    "text": format_text,
    "json": format_json,
    "markdown": format_markdown,
}
