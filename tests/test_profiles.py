"""Tests for profile loading."""

import builtins
import os
from pathlib import Path

import pytest

from devcap.profile_loader import (
    MAX_PROFILE_BYTES,
    MAX_PROFILE_DEPTH,
    Profile,
    list_builtin_profiles,
    load_builtin_profile,
    load_custom_profile,
)


def test_list_builtin_profiles():
    profiles = list_builtin_profiles()
    assert "full" in profiles
    assert "python-dev" in profiles
    assert "node-dev" in profiles
    assert "rust-dev" in profiles
    assert "devops" in profiles
    assert "sysadmin" in profiles


def test_load_full_profile():
    profile = load_builtin_profile("full")
    assert isinstance(profile, Profile)
    assert profile.name == "full"
    assert len(profile.tools) > 50


def test_load_python_dev_profile():
    profile = load_builtin_profile("python-dev")
    assert profile.name == "python-dev"
    tool_names = {t.name for t in profile.tools}
    assert "python3" in tool_names
    assert "pip" in tool_names
    assert "python3" in profile.required_tools


def test_load_devops_profile():
    profile = load_builtin_profile("devops")
    assert profile.name == "devops"
    assert "docker" in profile.required_tools
    assert len(profile.services) > 0


def test_profile_resolves_registry_defaults():
    profile = load_builtin_profile("python-dev")
    python_tool = next(t for t in profile.tools if t.name == "python3")
    assert python_tool.binary == "python3"
    assert python_tool.version_flag == "--version"


def test_profile_services():
    profile = load_builtin_profile("devops")
    svc_names = [name for name, _ in profile.services]
    assert "sshd" in svc_names or "docker" in svc_names


def test_custom_profile_rejects_interpreter_command(tmp_path):
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

    with pytest.raises(ValueError, match="not allowed|interpreter"):
        load_custom_profile(profile_path)


def test_custom_profile_rejects_option_like_service(tmp_path):
    profile_path = tmp_path / "unsafe-service.toml"
    profile_path.write_text(
        """
        [services]
        system = ["--user"]
        """,
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="systemd unit name"):
        load_custom_profile(profile_path)


def test_custom_profile_validates_schema_types(tmp_path):
    profile_path = tmp_path / "bad-schema.toml"
    profile_path.write_text('tools = "python3"\n', encoding="utf-8")

    with pytest.raises(ValueError, match="tools must be an array"):
        load_custom_profile(profile_path)


def test_custom_profile_loads_safe_custom_tool(tmp_path):
    profile_path = tmp_path / "safe.toml"
    profile_path.write_text(
        """
        [profile]
        name = "safe"

        [[tools]]
        name = "custom-tool"
        binary = "custom-tool"
        category = "Custom"
        version_flag = "--version"
        aliases = ["custom-tool2"]
        required = true
        """,
        encoding="utf-8",
    )

    profile = load_custom_profile(profile_path)

    assert profile.name == "safe"
    assert profile.tools[0].binary == "custom-tool"
    assert profile.tools[0].aliases == ("custom-tool2",)
    assert profile.required_tools == {"custom-tool"}


@pytest.mark.parametrize(
    ("document", "field"),
    [
        ("mystery = true\n", "profile document"),
        ("[profile]\nname = 'x'\nmystery = true\n", "profile"),
        ("[[tools]]\nname = 'x'\nmystery = true\n", "tools"),
        ("[services]\nsystem = []\nmystery = []\n", "services"),
    ],
)
def test_custom_profile_rejects_unknown_keys(tmp_path, document, field):
    profile_path = tmp_path / "unknown.toml"
    profile_path.write_text(document, encoding="utf-8")

    with pytest.raises(ValueError, match=rf"{field}.*unknown key"):
        load_custom_profile(profile_path)


def test_custom_profile_rejects_duplicate_normalized_names(tmp_path):
    profile_path = tmp_path / "duplicate.toml"
    profile_path.write_text(
        """
        [[tools]]
        name = "Custom-Tool"

        [[tools]]
        name = "custom-tool"
        """,
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="duplicate normalized tool name"):
        load_custom_profile(profile_path)


def test_custom_profile_rejects_oversized_file(tmp_path):
    profile_path = tmp_path / "oversized.toml"
    profile_path.write_bytes(b"#" * (MAX_PROFILE_BYTES + 1))

    with pytest.raises(ValueError, match="too large"):
        load_custom_profile(profile_path)


def test_custom_profile_detects_growth_after_fstat(tmp_path, monkeypatch):
    profile_path = tmp_path / "growing.toml"
    profile_path.write_text("[profile]\nname = 'original'\n", encoding="utf-8")
    real_open = builtins.open

    class GrowingHandle:
        def __init__(self, handle):
            self.handle = handle

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return self.handle.__exit__(*args)

        def fileno(self):
            return self.handle.fileno()

        def read(self, limit):
            with real_open(profile_path, "ab") as writer:
                writer.write(b"#" * (MAX_PROFILE_BYTES + 1))
            return self.handle.read(limit)

    def growing_open(file, *args, **kwargs):
        handle = real_open(file, *args, **kwargs)
        if Path(file) == profile_path:
            return GrowingHandle(handle)
        return handle

    monkeypatch.setattr(builtins, "open", growing_open)

    with pytest.raises(ValueError, match="too large"):
        load_custom_profile(profile_path)


def test_custom_profile_parses_the_exact_opened_file_when_path_is_replaced(tmp_path, monkeypatch):
    profile_path = tmp_path / "replace.toml"
    replacement = tmp_path / "replacement.toml"
    profile_path.write_text("[profile]\nname = 'opened'\n", encoding="utf-8")
    replacement.write_text("[profile]\nname = 'replacement'\n", encoding="utf-8")
    real_open = builtins.open

    def replacing_open(file, *args, **kwargs):
        handle = real_open(file, *args, **kwargs)
        if Path(file) == profile_path:
            os.replace(replacement, profile_path)
        return handle

    monkeypatch.setattr(builtins, "open", replacing_open)

    profile = load_custom_profile(profile_path)

    assert profile.name == "opened"
    assert "replacement" in profile_path.read_text(encoding="utf-8")


@pytest.mark.parametrize("max_depth", [0, -1, MAX_PROFILE_DEPTH + 1, 1.5, True])
def test_custom_profile_rejects_invalid_max_depth(tmp_path, max_depth):
    profile_path = tmp_path / "depth.toml"
    profile_path.write_text("[profile]\nname = 'depth'\n", encoding="utf-8")

    with pytest.raises(ValueError, match="max_depth"):
        load_custom_profile(profile_path, max_depth=max_depth)


def test_custom_profile_enforces_max_depth(tmp_path):
    profile_path = tmp_path / "depth.toml"
    profile_path.write_text("[[tools]]\nname = 'tool'\n", encoding="utf-8")

    with pytest.raises(ValueError, match="nesting exceeds"):
        load_custom_profile(profile_path, max_depth=2)
