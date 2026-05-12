"""Tests for the scanner engine."""

import os
import subprocess

from devcap import scanner
from devcap.registry import ToolDef
from devcap.scanner import (
    ScanResult,
    ToolResult,
    _find_binary,
    check_service,
    extract_version,
    redact_scan,
    scan_tool,
    scan_tools,
)


def test_extract_version_semver():
    assert extract_version("Python 3.12.3") == "3.12.3"


def test_extract_version_with_v_prefix():
    assert extract_version("v1.2.3") == "1.2.3"


def test_extract_version_multiline():
    output = "ruff 0.15.0\nsome other line\n"
    assert extract_version(output) == "0.15.0"


def test_extract_version_fallback():
    output = "no version number here"
    result = extract_version(output)
    assert result == "no version number here"


def test_extract_version_empty():
    assert extract_version("") is None
    assert extract_version("\n\n") is None


def test_extract_version_truncation():
    long_line = "x" * 100
    result = extract_version(long_line)
    assert result is not None
    assert len(result) <= 80


def test_extract_version_strips_terminal_controls():
    assert extract_version("\x1b[31mtool 1.2.3\x1b[0m\n") == "1.2.3"
    assert extract_version("bad\x07line") == "bad line"


def test_find_binary_skips_vendored_fallback(tmp_path, monkeypatch):
    vendored = tmp_path / "node_modules" / ".bin"
    vendored.mkdir(parents=True)
    fake = vendored / "fake-tool"
    fake.write_text("#!/bin/sh\nprintf 'fake 1.0.0\\n'\n", encoding="utf-8")
    fake.chmod(0o755)
    monkeypatch.setenv("PATH", str(vendored))

    tool = ToolDef(name="fake-tool", binary="fake-tool", category="Test")

    assert _find_binary(tool) is None
    assert _find_binary(tool, include_vendored=True) == str(fake)


def test_find_binary_prefers_non_vendored_alternative(tmp_path, monkeypatch):
    vendored = tmp_path / "node_modules" / ".bin"
    normal = tmp_path / "bin"
    vendored.mkdir(parents=True)
    normal.mkdir()
    vendored_fake = vendored / "fake-tool"
    normal_fake = normal / "fake-tool"
    for path in (vendored_fake, normal_fake):
        path.write_text("#!/bin/sh\nprintf 'fake 1.0.0\\n'\n", encoding="utf-8")
        path.chmod(0o755)
    monkeypatch.setenv("PATH", os.pathsep.join([str(vendored), str(normal)]))

    tool = ToolDef(name="fake-tool", binary="fake-tool", category="Test")

    assert _find_binary(tool) == str(normal_fake)


def test_find_binary_skips_project_local_relative_path(tmp_path, monkeypatch):
    local_bin = tmp_path / "bin"
    local_bin.mkdir()
    fake = local_bin / "fake-tool"
    fake.write_text("#!/bin/sh\nprintf 'fake 1.0.0\\n'\n", encoding="utf-8")
    fake.chmod(0o755)
    (tmp_path / "pyproject.toml").write_text('[project]\nname = "example"\n', encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("PATH", "bin")

    tool = ToolDef(name="fake-tool", binary="fake-tool", category="Test")

    assert _find_binary(tool) is None
    assert _find_binary(tool, include_vendored=True) == os.path.join("bin", "fake-tool")


def test_check_service_uses_argument_separator(monkeypatch):
    captured = {}

    def fake_run_command(cmd, **_kwargs):
        captured["cmd"] = cmd
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(scanner.platform, "system", lambda: "Linux")
    monkeypatch.setattr(scanner, "_run_command", fake_run_command)

    result = check_service("docker.service")

    assert result.active is True
    assert captured["cmd"][-2:] == ["--", "docker.service"]


def test_check_service_rejects_option_like_name(monkeypatch):
    monkeypatch.setattr(scanner.platform, "system", lambda: "Linux")

    def fail_run_command(*_args, **_kwargs):
        raise AssertionError("systemctl should not run for unsafe service names")

    monkeypatch.setattr(scanner, "_run_command", fail_run_command)

    result = check_service("--user")

    assert result.active is False


def test_scan_tool_python3():
    tool = ToolDef(
        name="python3",
        binary="python3",
        category="Languages",
        version_flag="--version",
        version_source="stdout",
    )
    result = scan_tool(tool)
    assert result.found is True
    assert result.version is not None
    assert "3." in result.version
    assert result.path is not None


def test_scan_tool_missing():
    tool = ToolDef(
        name="nonexistent_xyz",
        binary="nonexistent_xyz_binary",
        category="Test",
        version_flag="--version",
        version_source="stdout",
    )
    result = scan_tool(tool)
    assert result.found is False
    assert result.version is None
    assert result.path is None


def test_scan_tools_basic():
    tools = [
        ToolDef(
            name="python3",
            binary="python3",
            category="Languages",
            version_flag="--version",
            version_source="stdout",
        ),
    ]
    result = scan_tools(tools=tools, parallel=False)
    assert isinstance(result, ScanResult)
    assert result.hostname
    assert result.timestamp
    assert len(result.results) == 1
    assert result.results[0].found is True


def test_scan_tools_parallel():
    tools = [
        ToolDef(
            name="python3",
            binary="python3",
            category="Languages",
            version_flag="--version",
            version_source="stdout",
        ),
        ToolDef(
            name="git",
            binary="git",
            category="Version Control",
            version_flag="--version",
            version_source="stdout",
        ),
    ]
    result = scan_tools(tools=tools, parallel=True)
    assert len(result.results) == 2


def test_tool_result_to_dict():
    tr = ToolResult(
        name="python3",
        binary="python3",
        category="Languages",
        found=True,
        version="3.12.3",
        path="/usr/bin/python3",
    )
    d = tr.to_dict()
    assert d["name"] == "python3"
    assert d["found"] is True
    assert d["version"] == "3.12.3"


def test_tool_result_to_dict_missing():
    tr = ToolResult(name="missing", binary="missing", category="Test", found=False)
    d = tr.to_dict()
    assert d["found"] is False
    assert "version" not in d
    assert "path" not in d


def test_scan_result_to_dict():
    result = scan_tools(tools=[], parallel=False)
    d = result.to_dict()
    assert "hostname" in d
    assert "timestamp" in d
    assert "platform" in d
    assert "tools" in d
    assert "services" in d


def test_redact_scan_replaces_hostname_and_paths():
    scan = ScanResult(
        hostname="private-host",
        timestamp="now",
        platform="Linux",
        results=[
            ToolResult(
                name="python3",
                binary="python3",
                category="Languages",
                found=True,
                version="3.12.3",
                path="/home/user/.local/bin/python3",
            )
        ],
    )

    redacted = redact_scan(scan)

    assert redacted.hostname == "[redacted]"
    assert redacted.results[0].path == "[redacted]"
