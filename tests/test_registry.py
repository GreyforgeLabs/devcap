"""Tests for the tool registry."""

from devcap.registry import CATEGORIES, REGISTRY, ToolDef, get_tool, get_tools_by_category


def test_registry_not_empty():
    assert len(REGISTRY) > 50


def test_all_entries_are_tooldefs():
    for name, tool in REGISTRY.items():
        assert isinstance(tool, ToolDef)
        assert tool.name == name


def test_categories_list():
    assert len(CATEGORIES) == 14


def test_every_tool_has_known_category():
    for tool in REGISTRY.values():
        assert tool.category in CATEGORIES, f"{tool.name} has unknown category {tool.category}"


def test_get_tool_existing():
    tool = get_tool("python3")
    assert tool is not None
    assert tool.name == "python3"
    assert tool.category == "Languages"


def test_get_tool_missing():
    assert get_tool("nonexistent_tool_xyz") is None


def test_get_tools_by_category():
    langs = get_tools_by_category("Languages")
    assert len(langs) >= 5
    assert all(t.category == "Languages" for t in langs)


def test_aliases():
    fd = get_tool("fd")
    assert fd is not None
    assert "fdfind" in fd.aliases

    bat = get_tool("bat")
    assert bat is not None
    assert "batcat" in bat.aliases


def test_version_flag_overrides():
    go = get_tool("go")
    assert go is not None
    assert go.version_flag == "version"

    java = get_tool("java")
    assert java is not None
    assert java.version_source == "stderr"


def test_no_duplicate_names():
    names = [t.name for t in REGISTRY.values()]
    assert len(names) == len(set(names))
