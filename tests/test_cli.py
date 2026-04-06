"""Integration tests for the CLI."""

import json
import subprocess
import sys


def _run_devcap(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, "-m", "devcap", *args],
        capture_output=True,
        text=True,
        timeout=60,
    )


def test_no_args_shows_help():
    result = _run_devcap()
    assert result.returncode == 0
    assert "devcap" in result.stdout.lower()


def test_scan_text():
    result = _run_devcap("scan", "--profile", "python-dev")
    assert result.returncode == 0
    assert "python3" in result.stdout


def test_scan_json():
    result = _run_devcap("scan", "--profile", "python-dev", "--format", "json")
    assert result.returncode == 0
    data = json.loads(result.stdout)
    assert "hostname" in data
    assert "tools" in data


def test_scan_markdown():
    result = _run_devcap("scan", "--profile", "python-dev", "--format", "markdown")
    assert result.returncode == 0
    assert "##" in result.stdout


def test_check_python_dev():
    result = _run_devcap("check", "--profile", "python-dev")
    # Should pass on any machine with python3, pip, git
    assert result.returncode == 0


def test_list_profiles():
    result = _run_devcap("list-profiles")
    assert result.returncode == 0
    assert "full" in result.stdout
    assert "python-dev" in result.stdout


def test_unknown_profile():
    result = _run_devcap("scan", "--profile", "nonexistent")
    assert result.returncode == 2


def test_scan_no_parallel():
    result = _run_devcap("scan", "--profile", "python-dev", "--no-parallel", "--format", "json")
    assert result.returncode == 0
    data = json.loads(result.stdout)
    assert len(data["tools"]) > 0
