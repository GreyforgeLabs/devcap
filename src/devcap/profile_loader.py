"""Profile loader — built-in TOML profiles and custom config files."""

from __future__ import annotations

import importlib.resources
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from .registry import REGISTRY, ToolDef


@dataclass(slots=True, frozen=True)
class Profile:
    """A loaded scan profile."""

    name: str
    description: str
    tools: list[ToolDef]
    required_tools: set[str]
    services: list[tuple[str, bool]] = field(default_factory=list)


def _resolve_tool(entry: dict) -> ToolDef | None:
    """Resolve a TOML tool entry against the registry, applying overrides."""
    name = entry.get("name", "")
    registry_def = REGISTRY.get(name)

    category = entry.get("category", registry_def.category if registry_def else "Custom")
    binary = entry.get("binary", registry_def.binary if registry_def else name)
    default_vflag = registry_def.version_flag if registry_def else "--version"
    version_flag = entry.get("version_flag", default_vflag)
    default_vsrc = registry_def.version_source if registry_def else "stdout"
    version_source = entry.get("version_source", default_vsrc)
    aliases = tuple(entry.get("aliases", registry_def.aliases if registry_def else ()))

    return ToolDef(
        name=name,
        binary=binary,
        category=category,
        version_flag=version_flag,
        version_source=version_source,
        aliases=aliases,
    )


def _parse_profile(data: dict) -> Profile:
    """Parse a TOML profile dict into a Profile."""
    meta = data.get("profile", {})
    name = meta.get("name", "custom")
    description = meta.get("description", "")

    tools = []
    required = set()
    for entry in data.get("tools", []):
        tool = _resolve_tool(entry)
        if tool:
            tools.append(tool)
            if entry.get("required", False):
                required.add(tool.name)

    services_section = data.get("services", {})
    services: list[tuple[str, bool]] = []
    for svc in services_section.get("system", []):
        services.append((svc, False))
    for svc in services_section.get("user", []):
        services.append((svc, True))

    return Profile(
        name=name,
        description=description,
        tools=tools,
        required_tools=required,
        services=services,
    )


def load_builtin_profile(name: str) -> Profile:
    """Load a built-in TOML profile by name."""
    filename = f"{name}.toml"
    files = importlib.resources.files("devcap") / "profiles"
    resource = files / filename
    text = resource.read_text(encoding="utf-8")
    data = tomllib.loads(text)
    return _parse_profile(data)


def load_custom_profile(path: str | Path) -> Profile:
    """Load a custom TOML profile from a file path."""
    resolved = Path(path).resolve()
    if not resolved.is_file():
        raise FileNotFoundError(f"Profile not found: {resolved}")
    if resolved.suffix != ".toml":
        raise ValueError(f"Profile must be a .toml file: {resolved}")
    with open(resolved, "rb") as f:
        data = tomllib.load(f)
    return _parse_profile(data)


def list_builtin_profiles() -> list[str]:
    """Return names of all built-in profiles."""
    files = importlib.resources.files("devcap") / "profiles"
    names = []
    for item in files.iterdir():
        if item.name.endswith(".toml"):
            names.append(item.name.removesuffix(".toml"))
    return sorted(names)
