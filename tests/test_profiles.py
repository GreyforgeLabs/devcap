"""Tests for profile loading."""

import pytest

from devcap.profile_loader import (
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
