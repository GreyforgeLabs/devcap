"""Output formatters — text, json, markdown."""

from __future__ import annotations

import json

from .registry import CATEGORIES
from .safe_text import clean_text, markdown_cell, markdown_inline_code
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
    hostname = clean_text(scan.hostname, max_length=120)
    timestamp = clean_text(scan.timestamp, max_length=80)
    lines = [
        f"devcap scan — {hostname} — {timestamp}",
        f"Platform: {clean_text(scan.platform, max_length=160)}",
        "",
    ]

    groups = _group_by_category(scan.results)
    for category, tools in groups.items():
        found = [t for t in tools if t.found]
        missing = [t for t in tools if not t.found]

        lines.append(f"=== {clean_text(category, max_length=80)} ===")
        if found:
            for t in found:
                name = clean_text(t.name, max_length=32)
                version = clean_text(t.version or "?", max_length=80)
                path = clean_text(t.path or "", max_length=240)
                lines.append(f"  {name:<16} {version:<20} {path}")
        if missing:
            lines.append("  Missing:")
            for t in missing:
                lines.append(f"    {clean_text(t.name, max_length=80)}")
        lines.append("")

    if scan.services:
        lines.append("=== Services ===")
        for svc in scan.services:
            status = "running" if svc.active else "stopped"
            suffix = " (user)" if svc.user_service else ""
            lines.append(f"  [{status}] {clean_text(svc.name, max_length=120)}{suffix}")
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
    timestamp = markdown_cell(scan.timestamp, max_length=80)
    platform = markdown_cell(scan.platform, max_length=160)
    lines = [
        f"# Development Environment — {markdown_cell(scan.hostname, max_length=120)}",
        "",
        f"> Scanned: {timestamp} | Platform: {platform}",
        "",
    ]

    groups = _group_by_category(scan.results)
    for category, tools in groups.items():
        found = [t for t in tools if t.found]
        missing = [t for t in tools if not t.found]

        lines.append(f"## {markdown_cell(category, max_length=80)}")
        lines.append("")
        if found:
            lines.append("| Tool | Version | Path |")
            lines.append("|------|---------|------|")
            for t in found:
                name = markdown_cell(t.name, max_length=80)
                version = markdown_cell(t.version or "?", max_length=80)
                path = markdown_cell(t.path or "")
                lines.append(f"| {name} | {version} | {path} |")
            lines.append("")
        if missing:
            missing_names = ", ".join(markdown_inline_code(t.name) for t in missing)
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
            lines.append(f"| {markdown_cell(svc.name, max_length=120)}{suffix} | {status} |")
        lines.append("")

    return "\n".join(lines)


FORMATTERS = {
    "text": format_text,
    "json": format_json,
    "markdown": format_markdown,
}
