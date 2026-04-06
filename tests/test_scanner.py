"""Tests for the scanner engine."""

from devcap.registry import ToolDef
from devcap.scanner import (
    ScanResult,
    ToolResult,
    extract_version,
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


def test_scan_tool_python3():
    tool = ToolDef(
        name="python3", binary="python3", category="Languages",
        version_flag="--version", version_source="stdout",
    )
    result = scan_tool(tool)
    assert result.found is True
    assert result.version is not None
    assert "3." in result.version
    assert result.path is not None


def test_scan_tool_missing():
    tool = ToolDef(
        name="nonexistent_xyz", binary="nonexistent_xyz_binary",
        category="Test", version_flag="--version", version_source="stdout",
    )
    result = scan_tool(tool)
    assert result.found is False
    assert result.version is None
    assert result.path is None


def test_scan_tools_basic():
    tools = [
        ToolDef(name="python3", binary="python3", category="Languages",
                version_flag="--version", version_source="stdout"),
    ]
    result = scan_tools(tools=tools, parallel=False)
    assert isinstance(result, ScanResult)
    assert result.hostname
    assert result.timestamp
    assert len(result.results) == 1
    assert result.results[0].found is True


def test_scan_tools_parallel():
    tools = [
        ToolDef(name="python3", binary="python3", category="Languages",
                version_flag="--version", version_source="stdout"),
        ToolDef(name="git", binary="git", category="Version Control",
                version_flag="--version", version_source="stdout"),
    ]
    result = scan_tools(tools=tools, parallel=True)
    assert len(result.results) == 2


def test_tool_result_to_dict():
    tr = ToolResult(
        name="python3", binary="python3", category="Languages",
        found=True, version="3.12.3", path="/usr/bin/python3",
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
