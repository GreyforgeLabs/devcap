//! Tool registry — definitions for all scannable tools.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Definition of a scannable tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDef {
    pub name: String,
    pub binary: String,
    pub category: String,
    pub version_flag: String,
    pub version_source: String,
    pub aliases: Vec<String>,
}

impl ToolDef {
    /// A tool with the default `--version` flag read from stdout.
    pub fn new(name: &str, binary: &str, category: &str) -> Self {
        ToolDef {
            name: name.to_string(),
            binary: binary.to_string(),
            category: category.to_string(),
            version_flag: "--version".to_string(),
            version_source: "stdout".to_string(),
            aliases: Vec::new(),
        }
    }

    /// Builder: override the version flag.
    pub fn with_version_flag(mut self, flag: &str) -> Self {
        self.version_flag = flag.to_string();
        self
    }

    /// Builder: override the preferred version stream.
    pub fn with_version_source(mut self, source: &str) -> Self {
        self.version_source = source.to_string();
        self
    }

    /// Builder: set aliases.
    pub fn with_aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|a| a.to_string()).collect();
        self
    }
}

pub const LANGUAGES: &str = "Languages";
pub const PACKAGE_MANAGERS: &str = "Package Managers";
pub const BUILD_TOOLS: &str = "Build Tools";
pub const VERSION_CONTROL: &str = "Version Control";
pub const CONTAINERS: &str = "Containers";
pub const EDITORS: &str = "Editors";
pub const LINTING: &str = "Linting & Formatting";
pub const TESTING: &str = "Testing";
pub const DEBUGGING: &str = "Debugging & Profiling";
pub const NETWORK: &str = "Network";
pub const DATABASE: &str = "Database";
pub const SEARCH: &str = "Search & Files";
pub const AI_TOOLS: &str = "AI Tools";
pub const MISC: &str = "Miscellaneous";

/// Category display order.
pub const CATEGORIES: [&str; 14] = [
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
];

/// Insertion-ordered registry keyed by tool name.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    tools: Vec<ToolDef>,
    index: HashMap<String, usize>,
}

impl Registry {
    /// Look up a tool by exact name.
    pub fn get(&self, name: &str) -> Option<&ToolDef> {
        self.index.get(name).map(|&i| &self.tools[i])
    }

    /// All tools in registration order.
    pub fn values(&self) -> impl Iterator<Item = &ToolDef> {
        self.tools.iter()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Build a registry without silently overwriting normalized names.
pub fn build_registry(tools: Vec<ToolDef>) -> Result<Registry, String> {
    let mut registry = Registry::default();
    let mut normalized_names: HashMap<String, String> = HashMap::new();
    for tool in tools {
        let normalized = casefold(&tool.name);
        if let Some(first) = normalized_names.get(&normalized) {
            return Err(format!(
                "duplicate normalized tool name: {} conflicts with {}",
                crate::pycompat::repr(&tool.name),
                crate::pycompat::repr(first)
            ));
        }
        normalized_names.insert(normalized, tool.name.clone());
        if let Some(&existing) = registry.index.get(&tool.name) {
            registry.tools[existing] = tool;
        } else {
            registry
                .index
                .insert(tool.name.clone(), registry.tools.len());
            registry.tools.push(tool);
        }
    }
    Ok(registry)
}

/// `str.casefold()` for the names devcap accepts (validated tokens are ASCII;
/// non-ASCII falls back to Unicode lowercase).
pub fn casefold(name: &str) -> String {
    if name.is_ascii() {
        name.to_ascii_lowercase()
    } else {
        name.to_lowercase()
    }
}

fn t(name: &str, category: &str) -> ToolDef {
    ToolDef::new(name, name, category)
}

fn registry_tools() -> Vec<ToolDef> {
    vec![
        // --- Languages ---
        t("python3", LANGUAGES),
        t("node", LANGUAGES),
        t("bun", LANGUAGES),
        t("rustc", LANGUAGES),
        t("go", LANGUAGES).with_version_flag("version"),
        t("java", LANGUAGES).with_version_source("stderr"),
        t("ruby", LANGUAGES),
        t("php", LANGUAGES),
        t("perl", LANGUAGES).with_version_source("stderr"),
        t("dotnet", LANGUAGES),
        t("deno", LANGUAGES),
        t("zig", LANGUAGES).with_version_flag("version"),
        t("elixir", LANGUAGES),
        t("swift", LANGUAGES),
        // --- Package Managers ---
        t("pip", PACKAGE_MANAGERS),
        t("uv", PACKAGE_MANAGERS),
        t("npm", PACKAGE_MANAGERS),
        t("yarn", PACKAGE_MANAGERS),
        t("pnpm", PACKAGE_MANAGERS),
        t("cargo", PACKAGE_MANAGERS),
        t("gem", PACKAGE_MANAGERS),
        t("apt", PACKAGE_MANAGERS),
        t("brew", PACKAGE_MANAGERS),
        t("flatpak", PACKAGE_MANAGERS),
        t("snap", PACKAGE_MANAGERS),
        // --- Build Tools ---
        t("make", BUILD_TOOLS),
        t("cmake", BUILD_TOOLS),
        t("meson", BUILD_TOOLS),
        t("ninja", BUILD_TOOLS),
        t("autoconf", BUILD_TOOLS),
        t("automake", BUILD_TOOLS),
        t("just", BUILD_TOOLS),
        // --- Version Control ---
        t("git", VERSION_CONTROL),
        t("gh", VERSION_CONTROL),
        t("hg", VERSION_CONTROL),
        t("svn", VERSION_CONTROL),
        // --- Containers ---
        t("docker", CONTAINERS),
        t("docker-compose", CONTAINERS).with_version_flag("version"),
        t("podman", CONTAINERS),
        t("kubectl", CONTAINERS).with_version_flag("version --client --short"),
        t("helm", CONTAINERS).with_version_flag("version --short"),
        t("terraform", CONTAINERS),
        t("ansible", CONTAINERS),
        // --- Editors ---
        t("vim", EDITORS),
        t("nvim", EDITORS),
        t("nano", EDITORS),
        t("emacs", EDITORS),
        t("code", EDITORS),
        // --- Linting & Formatting ---
        t("ruff", LINTING),
        t("shellcheck", LINTING),
        t("shfmt", LINTING),
        t("eslint", LINTING),
        t("prettier", LINTING),
        t("clang-format", LINTING),
        t("rustfmt", LINTING),
        t("mypy", LINTING),
        t("pyright", LINTING),
        t("yamllint", LINTING),
        t("black", LINTING),
        t("isort", LINTING),
        // --- Testing ---
        t("pytest", TESTING),
        t("jest", TESTING),
        t("vitest", TESTING),
        t("playwright", TESTING).with_version_flag("--version"),
        // --- Debugging & Profiling ---
        t("gdb", DEBUGGING),
        t("lldb", DEBUGGING),
        t("strace", DEBUGGING)
            .with_version_flag("-V")
            .with_version_source("stderr"),
        t("ltrace", DEBUGGING)
            .with_version_flag("-V")
            .with_version_source("stderr"),
        t("valgrind", DEBUGGING),
        t("perf", DEBUGGING),
        t("htop", DEBUGGING),
        t("btop", DEBUGGING),
        // --- Network ---
        t("curl", NETWORK),
        t("wget", NETWORK),
        t("jq", NETWORK),
        t("yq", NETWORK),
        t("nmap", NETWORK),
        t("nc", NETWORK)
            .with_version_flag("-h")
            .with_version_source("stderr"),
        t("websocat", NETWORK),
        t("xh", NETWORK),
        t("socat", NETWORK).with_version_flag("-V"),
        t("openssl", NETWORK).with_version_flag("version"),
        // --- Database ---
        t("sqlite3", DATABASE),
        t("psql", DATABASE),
        t("mysql", DATABASE),
        t("redis-cli", DATABASE),
        t("mongosh", DATABASE),
        t("duckdb", DATABASE),
        // --- Search & Files ---
        t("rg", SEARCH).with_aliases(&["ripgrep"]),
        t("fd", SEARCH).with_aliases(&["fdfind"]),
        t("fzf", SEARCH),
        t("bat", SEARCH).with_aliases(&["batcat"]),
        t("eza", SEARCH),
        t("tree", SEARCH),
        t("tokei", SEARCH),
        t("delta", SEARCH),
        t("sd", SEARCH),
        // --- AI Tools ---
        t("ollama", AI_TOOLS),
        t("aider", AI_TOOLS),
        t("claude", AI_TOOLS),
        // --- Miscellaneous ---
        t("tmux", MISC),
        t("direnv", MISC),
        t("hyperfine", MISC),
    ]
}

/// The built-in tool registry.
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| build_registry(registry_tools()).expect("registry names are unique"))
}

/// Return all tools in a given category.
pub fn get_tools_by_category(category: &str) -> Vec<&'static ToolDef> {
    registry()
        .values()
        .filter(|t| t.category == category)
        .collect()
}

/// Look up a tool by name.
pub fn get_tool(name: &str) -> Option<&'static ToolDef> {
    registry().get(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_not_empty() {
        assert!(registry().len() > 50);
        assert_eq!(registry().len(), 103);
    }

    #[test]
    fn test_all_entries_are_tooldefs() {
        for tool in registry().values() {
            assert_eq!(registry().get(&tool.name), Some(tool));
        }
    }

    #[test]
    fn test_categories_list() {
        assert_eq!(CATEGORIES.len(), 14);
    }

    #[test]
    fn test_every_tool_has_known_category() {
        for tool in registry().values() {
            assert!(
                CATEGORIES.contains(&tool.category.as_str()),
                "{} has unknown category {}",
                tool.name,
                tool.category
            );
        }
    }

    #[test]
    fn test_get_tool_existing() {
        let tool = get_tool("python3").expect("python3");
        assert_eq!(tool.name, "python3");
        assert_eq!(tool.category, "Languages");
    }

    #[test]
    fn test_get_tool_missing() {
        assert!(get_tool("nonexistent_tool_xyz").is_none());
    }

    #[test]
    fn test_get_tools_by_category() {
        let langs = get_tools_by_category("Languages");
        assert!(langs.len() >= 5);
        assert!(langs.iter().all(|t| t.category == "Languages"));
    }

    #[test]
    fn test_aliases() {
        assert!(
            get_tool("fd")
                .unwrap()
                .aliases
                .contains(&"fdfind".to_string())
        );
        assert!(
            get_tool("bat")
                .unwrap()
                .aliases
                .contains(&"batcat".to_string())
        );
    }

    #[test]
    fn test_version_flag_overrides() {
        assert_eq!(get_tool("go").unwrap().version_flag, "version");
        assert_eq!(get_tool("java").unwrap().version_source, "stderr");
    }

    #[test]
    fn test_no_duplicate_names() {
        let mut names: Vec<&str> = registry().values().map(|t| t.name.as_str()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names.len());
    }

    #[test]
    fn test_registry_builder_rejects_normalized_duplicate_names() {
        let tools = vec![
            ToolDef::new("Example", "example", "Test"),
            ToolDef::new("example", "example-2", "Test"),
        ];
        let err = build_registry(tools).unwrap_err();
        assert!(err.contains("duplicate normalized tool name"), "{err}");
        assert_eq!(
            err,
            "duplicate normalized tool name: 'example' conflicts with 'Example'"
        );
    }
}
