"""Tests for output formatters."""

import json

from devcap.formatters import format_json, format_markdown, format_text
from devcap.scanner import ScanResult, ServiceResult, ToolResult


def _make_scan() -> ScanResult:
    return ScanResult(
        hostname="testhost",
        timestamp="2026-01-01T00:00:00+00:00",
        platform="Linux 6.0.0",
        results=[
            ToolResult(
                name="python3",
                binary="python3",
                category="Languages",
                found=True,
                version="3.12.3",
                path="/usr/bin/python3",
            ),
            ToolResult(
                name="rustc",
                binary="rustc",
                category="Languages",
                found=False,
            ),
            ToolResult(
                name="git",
                binary="git",
                category="Version Control",
                found=True,
                version="2.43.0",
                path="/usr/bin/git",
            ),
        ],
        services=[
            ServiceResult(name="sshd", active=True),
            ServiceResult(name="docker", active=False),
        ],
    )


def test_format_text():
    text = format_text(_make_scan())
    assert "testhost" in text
    assert "python3" in text
    assert "3.12.3" in text
    assert "rustc" in text
    assert "Missing:" in text
    assert "sshd" in text
    assert "Found 2/3 tools" in text


def test_format_json():
    output = format_json(_make_scan())
    data = json.loads(output)
    assert data["hostname"] == "testhost"
    assert len(data["tools"]) == 3
    assert data["tools"][0]["found"] is True
    assert data["tools"][1]["found"] is False
    assert len(data["services"]) == 2


def test_format_markdown():
    md = format_markdown(_make_scan())
    assert "# Development Environment" in md
    assert "| python3 |" in md
    assert "`rustc`" in md
    assert "## Services" in md
    assert "| sshd |" in md


def test_format_text_empty():
    scan = ScanResult(
        hostname="empty",
        timestamp="now",
        platform="Linux",
        results=[],
        services=[],
    )
    text = format_text(scan)
    assert "Found 0/0 tools" in text


def test_format_json_roundtrip():
    scan = _make_scan()
    output = format_json(scan)
    data = json.loads(output)
    assert isinstance(data, dict)
    assert "tools" in data


def test_formatters_strip_control_sequences_and_escape_markdown():
    scan = ScanResult(
        hostname="host\x1b[31m\nspoof",
        timestamp="2026-01-01T00:00:00+00:00",
        platform="Linux 6.0.0",
        results=[
            ToolResult(
                name="bad|tool",
                binary="bad",
                category="Custom|Category",
                found=True,
                version="\x1b[31m1.0|spoof\x1b[0m",
                path="/tmp/a|b\nnext",
            )
        ],
        services=[ServiceResult(name="svc|name", active=True)],
    )

    text = format_text(scan)
    markdown = format_markdown(scan)

    assert "\x1b" not in text
    assert "\x1b" not in markdown
    assert "spoof\n" not in text
    assert "bad\\|tool" in markdown
    assert "/tmp/a\\|b next" in markdown
