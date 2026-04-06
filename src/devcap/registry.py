"""Tool registry — definitions for all scannable tools."""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(slots=True, frozen=True)
class ToolDef:
    """Definition of a scannable tool."""

    name: str
    binary: str
    category: str
    version_flag: str = "--version"
    version_source: str = "stdout"
    aliases: tuple[str, ...] = field(default_factory=tuple)


# Category constants
LANGUAGES = "Languages"
PACKAGE_MANAGERS = "Package Managers"
BUILD_TOOLS = "Build Tools"
VERSION_CONTROL = "Version Control"
CONTAINERS = "Containers"
EDITORS = "Editors"
LINTING = "Linting & Formatting"
TESTING = "Testing"
DEBUGGING = "Debugging & Profiling"
NETWORK = "Network"
DATABASE = "Database"
SEARCH = "Search & Files"
AI_TOOLS = "AI Tools"
MISC = "Miscellaneous"

CATEGORIES = [
    LANGUAGES,
    PACKAGE_MANAGERS,
    BUILD_TOOLS,
    VERSION_CONTROL,
    CONTAINERS,
    EDITORS,
    LINTING,
    TESTING,
    DEBUGGING,
    NETWORK,
    DATABASE,
    SEARCH,
    AI_TOOLS,
    MISC,
]


def _tool(name: str, category: str, **kwargs) -> ToolDef:
    return ToolDef(name=name, binary=kwargs.pop("binary", name), category=category, **kwargs)


# fmt: off
REGISTRY: dict[str, ToolDef] = {t.name: t for t in [
    # --- Languages ---
    _tool("python3",  LANGUAGES),
    _tool("node",     LANGUAGES),
    _tool("bun",      LANGUAGES),
    _tool("rustc",    LANGUAGES),
    _tool("go",       LANGUAGES, version_flag="version"),
    _tool("java",     LANGUAGES, version_source="stderr"),
    _tool("ruby",     LANGUAGES),
    _tool("php",      LANGUAGES),
    _tool("perl",     LANGUAGES, version_source="stderr"),
    _tool("dotnet",   LANGUAGES),
    _tool("deno",     LANGUAGES),
    _tool("zig",      LANGUAGES, version_flag="version"),
    _tool("elixir",   LANGUAGES),
    _tool("swift",    LANGUAGES),

    # --- Package Managers ---
    _tool("pip",      PACKAGE_MANAGERS),
    _tool("uv",       PACKAGE_MANAGERS),
    _tool("npm",      PACKAGE_MANAGERS),
    _tool("yarn",     PACKAGE_MANAGERS),
    _tool("pnpm",     PACKAGE_MANAGERS),
    _tool("cargo",    PACKAGE_MANAGERS),
    _tool("gem",      PACKAGE_MANAGERS),
    _tool("apt",      PACKAGE_MANAGERS),
    _tool("brew",     PACKAGE_MANAGERS, binary="brew"),
    _tool("flatpak",  PACKAGE_MANAGERS),
    _tool("snap",     PACKAGE_MANAGERS),

    # --- Build Tools ---
    _tool("make",     BUILD_TOOLS),
    _tool("cmake",    BUILD_TOOLS),
    _tool("meson",    BUILD_TOOLS),
    _tool("ninja",    BUILD_TOOLS),
    _tool("autoconf", BUILD_TOOLS),
    _tool("automake", BUILD_TOOLS),
    _tool("just",     BUILD_TOOLS),

    # --- Version Control ---
    _tool("git",      VERSION_CONTROL),
    _tool("gh",       VERSION_CONTROL),
    _tool("hg",       VERSION_CONTROL),
    _tool("svn",      VERSION_CONTROL),

    # --- Containers ---
    _tool("docker",           CONTAINERS),
    _tool("docker-compose",   CONTAINERS, version_flag="version"),
    _tool("podman",           CONTAINERS),
    _tool("kubectl",          CONTAINERS, version_flag="version --client --short"),
    _tool("helm",             CONTAINERS, version_flag="version --short"),
    _tool("terraform",        CONTAINERS),
    _tool("ansible",          CONTAINERS),

    # --- Editors ---
    _tool("vim",   EDITORS),
    _tool("nvim",  EDITORS),
    _tool("nano",  EDITORS),
    _tool("emacs", EDITORS),
    _tool("code",  EDITORS),

    # --- Linting & Formatting ---
    _tool("ruff",         LINTING),
    _tool("shellcheck",   LINTING),
    _tool("shfmt",        LINTING),
    _tool("eslint",       LINTING),
    _tool("prettier",     LINTING),
    _tool("clang-format", LINTING),
    _tool("rustfmt",      LINTING),
    _tool("mypy",         LINTING),
    _tool("pyright",      LINTING),
    _tool("yamllint",     LINTING),
    _tool("black",        LINTING),
    _tool("isort",        LINTING),

    # --- Testing ---
    _tool("pytest",     TESTING),
    _tool("jest",       TESTING),
    _tool("vitest",     TESTING),
    _tool("playwright", TESTING, version_flag="--version"),

    # --- Debugging & Profiling ---
    _tool("gdb",      DEBUGGING),
    _tool("lldb",     DEBUGGING),
    _tool("strace",   DEBUGGING, version_flag="-V", version_source="stderr"),
    _tool("ltrace",   DEBUGGING, version_flag="-V", version_source="stderr"),
    _tool("valgrind", DEBUGGING),
    _tool("perf",     DEBUGGING),
    _tool("htop",     DEBUGGING),
    _tool("btop",     DEBUGGING),

    # --- Network ---
    _tool("curl",        NETWORK),
    _tool("wget",        NETWORK),
    _tool("jq",          NETWORK),
    _tool("yq",          NETWORK),
    _tool("nmap",        NETWORK),
    _tool("nc",          NETWORK, version_flag="-h", version_source="stderr"),
    _tool("websocat",    NETWORK),
    _tool("xh",          NETWORK),
    _tool("socat",       NETWORK, version_flag="-V"),
    _tool("openssl",     NETWORK, version_flag="version"),

    # --- Database ---
    _tool("sqlite3", DATABASE),
    _tool("psql",    DATABASE),
    _tool("mysql",   DATABASE),
    _tool("redis-cli", DATABASE, binary="redis-cli"),
    _tool("mongosh", DATABASE),
    _tool("duckdb",  DATABASE),

    # --- Search & Files ---
    _tool("rg",    SEARCH, aliases=("ripgrep",)),
    _tool("fd",    SEARCH, aliases=("fdfind",)),
    _tool("fzf",   SEARCH),
    _tool("bat",   SEARCH, aliases=("batcat",)),
    _tool("eza",   SEARCH),
    _tool("tree",  SEARCH),
    _tool("tokei", SEARCH),
    _tool("delta", SEARCH),
    _tool("sd",    SEARCH),

    # --- AI Tools ---
    _tool("ollama", AI_TOOLS),
    _tool("aider",  AI_TOOLS),
    _tool("claude", AI_TOOLS),

    # --- Miscellaneous ---
    _tool("tmux",      MISC),
    _tool("direnv",    MISC),
    _tool("hyperfine", MISC),
]}
# fmt: on


def get_tools_by_category(category: str) -> list[ToolDef]:
    """Return all tools in a given category."""
    return [t for t in REGISTRY.values() if t.category == category]


def get_tool(name: str) -> ToolDef | None:
    """Look up a tool by name."""
    return REGISTRY.get(name)
