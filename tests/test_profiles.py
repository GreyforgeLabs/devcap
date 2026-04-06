"""Tests for profile loading."""

from devcap.profile_loader import Profile, list_builtin_profiles, load_builtin_profile


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
