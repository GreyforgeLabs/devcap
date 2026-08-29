"""Profile loader — built-in TOML profiles and custom config files."""

from __future__ import annotations

import importlib.resources
import os
import re
import shlex
import stat
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from .registry import REGISTRY, ToolDef

MAX_PROFILE_BYTES = 1_000_000
MAX_TOOLS = 256
MAX_SERVICES = 128
MAX_ALIASES = 16
MAX_VERSION_ARGS = 8
MAX_STRING_LENGTH = 256
MAX_PROFILE_DEPTH = 16

TOP_LEVEL_KEYS = frozenset({"profile", "tools", "services"})
PROFILE_KEYS = frozenset({"name", "description"})
TOOL_KEYS = frozenset(
    {"name", "binary", "category", "version_flag", "version_source", "aliases", "required"}
)
SERVICE_KEYS = frozenset({"system", "user"})

TOKEN_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.+@:-]{0,127}$")
SERVICE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.@:+-]{0,127}$")
CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")
UNSAFE_FLAG_CHARS = frozenset(";&|`$<>")
UNSAFE_CUSTOM_BINARIES = frozenset(
    {
        "bash",
        "cmd",
        "cmd.exe",
        "cscript",
        "dash",
        "deno",
        "env",
        "fish",
        "ksh",
        "node",
        "osascript",
        "perl",
        "php",
        "powershell",
        "pwsh",
        "python",
        "python3",
        "ruby",
        "sh",
        "sudo",
        "su",
        "wscript",
        "xargs",
        "zsh",
    }
)


@dataclass(slots=True, frozen=True)
class Profile:
    """A loaded scan profile."""

    name: str
    description: str
    tools: list[ToolDef]
    required_tools: set[str]
    services: list[tuple[str, bool]] = field(default_factory=list)


def _reject_unknown_keys(value: dict, allowed: frozenset[str], field: str) -> None:
    unknown = sorted(set(value) - allowed)
    if unknown:
        raise ValueError(f"{field} contains unknown key(s): {', '.join(unknown)}")


def _validate_max_depth(value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError("max_depth must be a positive integer")
    if value < 1 or value > MAX_PROFILE_DEPTH:
        raise ValueError(f"max_depth must be between 1 and {MAX_PROFILE_DEPTH}")
    return value


def _assert_profile_depth(value: object, max_depth: int) -> None:
    """Reject unexpectedly deep TOML structures after a bounded read."""
    limit = _validate_max_depth(max_depth)
    stack = [(value, 1)]
    while stack:
        current, depth = stack.pop()
        if depth > limit:
            raise ValueError(f"profile nesting exceeds max_depth {limit}")
        if isinstance(current, dict):
            stack.extend((child, depth + 1) for child in current.values())
        elif isinstance(current, list | tuple):
            stack.extend((child, depth + 1) for child in current)


def _string(value: object, field: str, *, default: str | None = None) -> str:
    if value is None:
        if default is not None:
            return default
        raise ValueError(f"{field} is required")
    if not isinstance(value, str):
        raise ValueError(f"{field} must be a string")
    if len(value) > MAX_STRING_LENGTH:
        raise ValueError(f"{field} is too long")
    if CONTROL_RE.search(value):
        raise ValueError(f"{field} contains control characters")
    return value


def _bool(value: object, field: str, *, default: bool = False) -> bool:
    if value is None:
        return default
    if not isinstance(value, bool):
        raise ValueError(f"{field} must be a boolean")
    return value


def _string_list(
    value: object,
    field: str,
    *,
    default: tuple[str, ...] = (),
    max_items: int = MAX_ALIASES,
) -> tuple[str, ...]:
    if value is None:
        return default
    if not isinstance(value, list | tuple):
        raise ValueError(f"{field} must be an array of strings")
    if len(value) > max_items:
        raise ValueError(f"{field} has too many entries")
    return tuple(_string(item, f"{field}[]") for item in value)


def _validate_token(value: str, field: str) -> None:
    if not TOKEN_RE.fullmatch(value):
        raise ValueError(f"{field} must be a command name, not a path or shell expression")


def _validate_service_name(value: str, field: str) -> None:
    if not SERVICE_RE.fullmatch(value):
        raise ValueError(f"{field} must be a systemd unit name, not an option or path")


def _validate_version_flag(value: str, *, trusted_registry_command: bool) -> tuple[str, ...]:
    try:
        parts = tuple(shlex.split(value))
    except ValueError as exc:
        raise ValueError(f"version_flag is invalid: {exc}") from exc
    if not parts:
        raise ValueError("version_flag must not be empty")
    if len(parts) > MAX_VERSION_ARGS:
        raise ValueError("version_flag has too many arguments")
    for part in parts:
        _string(part, "version_flag[]")
        if not trusted_registry_command and any(char in part for char in UNSAFE_FLAG_CHARS):
            raise ValueError("custom version_flag contains shell-control characters")
    if not trusted_registry_command and any(part in {"-c", "--command"} for part in parts):
        raise ValueError("custom profile commands cannot pass interpreter command flags")
    return parts


def _resolve_tool(entry: dict) -> ToolDef | None:
    """Resolve a TOML tool entry against the registry, applying overrides."""
    if not isinstance(entry, dict):
        raise ValueError("tools entries must be TOML tables")
    _reject_unknown_keys(entry, TOOL_KEYS, "tools[]")

    name = _string(entry.get("name"), "tools[].name")
    _validate_token(name, "tools[].name")
    registry_def = REGISTRY.get(name)

    category = _string(
        entry.get("category"),
        "tools[].category",
        default=registry_def.category if registry_def else "Custom",
    )
    binary = _string(
        entry.get("binary"), "tools[].binary", default=registry_def.binary if registry_def else name
    )
    _validate_token(binary, "tools[].binary")
    default_vflag = registry_def.version_flag if registry_def else "--version"
    version_flag = _string(entry.get("version_flag"), "tools[].version_flag", default=default_vflag)
    default_vsrc = registry_def.version_source if registry_def else "stdout"
    version_source = _string(
        entry.get("version_source"), "tools[].version_source", default=default_vsrc
    )
    if version_source not in {"stdout", "stderr"}:
        raise ValueError("tools[].version_source must be stdout or stderr")
    aliases = _string_list(
        entry.get("aliases"),
        "tools[].aliases",
        default=registry_def.aliases if registry_def else (),
    )
    for alias in aliases:
        _validate_token(alias, "tools[].aliases[]")

    trusted_registry_command = bool(
        registry_def
        and binary == registry_def.binary
        and version_flag == registry_def.version_flag
        and aliases == registry_def.aliases
    )
    _validate_version_flag(version_flag, trusted_registry_command=trusted_registry_command)
    if not trusted_registry_command and binary in UNSAFE_CUSTOM_BINARIES:
        raise ValueError(f"custom profile binary '{binary}' is not allowed")

    return ToolDef(
        name=name,
        binary=binary,
        category=category,
        version_flag=version_flag,
        version_source=version_source,
        aliases=aliases,
    )


def _parse_profile(data: dict, *, max_depth: int = MAX_PROFILE_DEPTH) -> Profile:
    """Parse a TOML profile dict into a Profile."""
    if not isinstance(data, dict):
        raise ValueError("profile document must be a TOML table")
    _assert_profile_depth(data, max_depth)
    _reject_unknown_keys(data, TOP_LEVEL_KEYS, "profile document")

    meta = data.get("profile", {})
    if not isinstance(meta, dict):
        raise ValueError("[profile] must be a TOML table")
    _reject_unknown_keys(meta, PROFILE_KEYS, "[profile]")
    name = _string(meta.get("name"), "profile.name", default="custom")
    description = _string(meta.get("description"), "profile.description", default="")

    tools = []
    required = set()
    tool_entries = data.get("tools", [])
    if not isinstance(tool_entries, list):
        raise ValueError("tools must be an array of TOML tables")
    if len(tool_entries) > MAX_TOOLS:
        raise ValueError("profile has too many tools")
    normalized_tool_names: dict[str, str] = {}
    for entry in tool_entries:
        tool = _resolve_tool(entry)
        if tool:
            normalized = tool.name.casefold()
            if normalized in normalized_tool_names:
                first = normalized_tool_names[normalized]
                raise ValueError(
                    f"duplicate normalized tool name: {tool.name!r} conflicts with {first!r}"
                )
            normalized_tool_names[normalized] = tool.name
            tools.append(tool)
            if _bool(entry.get("required"), "tools[].required"):
                required.add(tool.name)

    services_section = data.get("services", {})
    if not isinstance(services_section, dict):
        raise ValueError("[services] must be a TOML table")
    _reject_unknown_keys(services_section, SERVICE_KEYS, "[services]")
    services: list[tuple[str, bool]] = []
    system_services = _string_list(
        services_section.get("system"), "services.system", max_items=MAX_SERVICES
    )
    user_services = _string_list(
        services_section.get("user"), "services.user", max_items=MAX_SERVICES
    )
    if len(system_services) + len(user_services) > MAX_SERVICES:
        raise ValueError("profile has too many services")
    for svc in system_services:
        _validate_service_name(svc, "services.system[]")
        services.append((svc, False))
    for svc in user_services:
        _validate_service_name(svc, "services.user[]")
        services.append((svc, True))

    return Profile(
        name=name,
        description=description,
        tools=tools,
        required_tools=required,
        services=services,
    )


def load_builtin_profile(name: str, *, max_depth: int = MAX_PROFILE_DEPTH) -> Profile:
    """Load a built-in TOML profile by name."""
    _validate_token(name, "profile name")
    filename = f"{name}.toml"
    files = importlib.resources.files("devcap") / "profiles"
    resource = files / filename
    text = resource.read_text(encoding="utf-8")
    data = tomllib.loads(text)
    return _parse_profile(data, max_depth=max_depth)


def load_custom_profile(path: str | Path, *, max_depth: int = MAX_PROFILE_DEPTH) -> Profile:
    """Load a custom TOML profile from a file path."""
    resolved = Path(path).expanduser().absolute()
    if resolved.suffix != ".toml":
        raise ValueError(f"Profile must be a .toml file: {resolved}")
    _validate_max_depth(max_depth)
    try:
        handle = open(resolved, "rb")
    except FileNotFoundError as exc:
        raise FileNotFoundError(f"Profile not found: {resolved}") from exc
    except OSError as exc:
        raise ValueError(f"Profile cannot be opened: {resolved}: {exc.strerror}") from exc

    with handle:
        metadata = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(f"Profile must be a regular file: {resolved}")
        if metadata.st_size > MAX_PROFILE_BYTES:
            raise ValueError(f"Profile is too large: {resolved}")
        raw = handle.read(MAX_PROFILE_BYTES + 1)
        if len(raw) > MAX_PROFILE_BYTES:
            raise ValueError(f"Profile is too large: {resolved}")

    try:
        data = tomllib.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        raise ValueError(f"Profile must be UTF-8: {resolved}") from exc
    return _parse_profile(data, max_depth=max_depth)


def list_builtin_profiles() -> list[str]:
    """Return names of all built-in profiles."""
    files = importlib.resources.files("devcap") / "profiles"
    names = []
    for item in files.iterdir():
        if item.name.endswith(".toml"):
            names.append(item.name.removesuffix(".toml"))
    return sorted(names)
