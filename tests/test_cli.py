"""Integration tests for the CLI."""

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest


def _run_devcap(*args: str) -> subprocess.CompletedProcess:
    env = os.environ.copy()
    src = str(Path(__file__).resolve().parents[1] / "src")
    env["PYTHONPATH"] = os.pathsep.join([src, env["PYTHONPATH"]]) if env.get("PYTHONPATH") else src
    return subprocess.run(
        [sys.executable, "-m", "devcap", *args],
        capture_output=True,
        env=env,
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


def test_scan_redact_json():
    result = _run_devcap("scan", "--profile", "python-dev", "--format", "json", "--redact")
    assert result.returncode == 0
    data = json.loads(result.stdout)
    assert data["hostname"] == "[redacted]"
    assert all(tool.get("path") == "[redacted]" for tool in data["tools"] if tool["found"])


def test_rejects_unsafe_custom_profile(tmp_path):
    profile_path = tmp_path / "unsafe.toml"
    profile_path.write_text(
        """
        [[tools]]
        name = "owned"
        binary = "sh"
        version_flag = "-c id"
        """,
        encoding="utf-8",
    )

    result = _run_devcap("scan", "--config", str(profile_path))

    assert result.returncode == 2
    assert "invalid profile" in result.stderr


@pytest.mark.parametrize(
    "args",
    [
        ("--timeout", "0"),
        ("--timeout", "-1"),
        ("--timeout", "nan"),
        ("--timeout", "inf"),
        ("--timeout", "61"),
        ("--max-depth", "0"),
        ("--max-depth", "17"),
        ("--max-workers", "0"),
        ("--max-workers", "65"),
    ],
)
def test_rejects_invalid_numeric_options(args):
    result = _run_devcap("scan", "--profile", "python-dev", *args)

    assert result.returncode == 2
    assert "error" in result.stderr.lower()


def test_profile_and_config_are_mutually_exclusive(tmp_path):
    profile_path = tmp_path / "profile.toml"
    profile_path.write_text("[profile]\nname = 'custom'\n", encoding="utf-8")

    result = _run_devcap(
        "scan",
        "--profile",
        "python-dev",
        "--config",
        str(profile_path),
    )

    assert result.returncode == 2
