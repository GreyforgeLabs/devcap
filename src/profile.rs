//! Profile loader — built-in TOML profiles and custom config files.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::pycompat::{char_len, repr};
use crate::registry::{ToolDef, casefold, registry};
use crate::shlex;
use crate::toml::{self, Table, TomlError, Value};

pub const MAX_PROFILE_BYTES: u64 = 1_000_000;
pub const MAX_TOOLS: usize = 256;
pub const MAX_SERVICES: usize = 128;
pub const MAX_ALIASES: usize = 16;
pub const MAX_VERSION_ARGS: usize = 8;
pub const MAX_STRING_LENGTH: usize = 256;
pub const MAX_PROFILE_DEPTH: i64 = 16;

const TOP_LEVEL_KEYS: &[&str] = &["profile", "tools", "services"];
const PROFILE_KEYS: &[&str] = &["name", "description"];
const TOOL_KEYS: &[&str] = &[
    "name",
    "binary",
    "category",
    "version_flag",
    "version_source",
    "aliases",
    "required",
];
const SERVICE_KEYS: &[&str] = &["system", "user"];
const UNSAFE_FLAG_CHARS: &[char] = &[';', '&', '|', '`', '$', '<', '>'];
const UNSAFE_CUSTOM_BINARIES: &[&str] = &[
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
];

/// Built-in profiles embedded at compile time, sorted by name.
pub const BUILTIN_PROFILES: &[(&str, &str)] = &[
    ("devops", include_str!("profiles/devops.toml")),
    ("full", include_str!("profiles/full.toml")),
    ("node-dev", include_str!("profiles/node-dev.toml")),
    ("python-dev", include_str!("profiles/python-dev.toml")),
    ("rust-dev", include_str!("profiles/rust-dev.toml")),
    ("sysadmin", include_str!("profiles/sysadmin.toml")),
];

/// A loaded scan profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub tools: Vec<ToolDef>,
    pub required_tools: BTreeSet<String>,
    /// `(unit name, is_user_service)` in profile order.
    pub services: Vec<(String, bool)>,
}

/// Profile loading failure, mirroring the Python exception classes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// `FileNotFoundError`.
    NotFound(String),
    /// `ValueError` (including `tomllib.TOMLDecodeError`).
    Invalid(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::NotFound(msg) | ProfileError::Invalid(msg) => f.write_str(msg),
        }
    }
}

type PResult<T> = Result<T, ProfileError>;

fn invalid<T>(msg: impl Into<String>) -> PResult<T> {
    Err(ProfileError::Invalid(msg.into()))
}

fn reject_unknown_keys(table: &Table, allowed: &[&str], field: &str) -> PResult<()> {
    let mut unknown: Vec<&String> = table
        .keys()
        .filter(|k| !allowed.contains(&k.as_str()))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort();
    let list = unknown
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    invalid(format!("{field} contains unknown key(s): {list}"))
}

/// Validate a `max_depth` value (1..=16).
pub fn validate_max_depth(value: i64) -> PResult<usize> {
    if !(1..=MAX_PROFILE_DEPTH).contains(&value) {
        return invalid(format!(
            "max_depth must be between 1 and {MAX_PROFILE_DEPTH}"
        ));
    }
    Ok(value as usize)
}

fn assert_profile_depth(data: &Table, max_depth: i64) -> PResult<()> {
    let limit = validate_max_depth(max_depth)?;
    enum Node<'a> {
        Table(&'a Table),
        Value(&'a Value),
    }
    let mut stack: Vec<(Node, usize)> = vec![(Node::Table(data), 1)];
    while let Some((node, depth)) = stack.pop() {
        if depth > limit {
            return invalid(format!("profile nesting exceeds max_depth {limit}"));
        }
        match node {
            Node::Table(t) | Node::Value(Value::Table(t)) => {
                stack.extend(t.values().map(|v| (Node::Value(v), depth + 1)));
            }
            Node::Value(Value::Array(items)) => {
                stack.extend(items.iter().map(|v| (Node::Value(v), depth + 1)));
            }
            Node::Value(_) => {}
        }
    }
    Ok(())
}

fn has_control(value: &str) -> bool {
    value.chars().any(|c| (c as u32) < 0x20 || c == '\x7f')
}

fn check_string(value: &str, field: &str) -> PResult<()> {
    if char_len(value) > MAX_STRING_LENGTH {
        return invalid(format!("{field} is too long"));
    }
    if has_control(value) {
        return invalid(format!("{field} contains control characters"));
    }
    Ok(())
}

fn string_field(value: Option<&Value>, field: &str, default: Option<&str>) -> PResult<String> {
    match value {
        None => match default {
            Some(d) => Ok(d.to_string()),
            None => invalid(format!("{field} is required")),
        },
        Some(Value::Str(s)) => {
            check_string(s, field)?;
            Ok(s.clone())
        }
        Some(_) => invalid(format!("{field} must be a string")),
    }
}

fn bool_field(value: Option<&Value>, field: &str) -> PResult<bool> {
    match value {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => invalid(format!("{field} must be a boolean")),
    }
}

fn string_list(
    value: Option<&Value>,
    field: &str,
    default: &[String],
    max_items: usize,
) -> PResult<Vec<String>> {
    let Some(value) = value else {
        return Ok(default.to_vec());
    };
    let Value::Array(items) = value else {
        return invalid(format!("{field} must be an array of strings"));
    };
    if items.len() > max_items {
        return invalid(format!("{field} has too many entries"));
    }
    let item_field = format!("{field}[]");
    items
        .iter()
        .map(|item| string_field(Some(item), &item_field, None))
        .collect()
}

fn is_token(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b"_.+@:-".contains(&b))
}

/// `SERVICE_RE` / `SERVICE_NAME_RE`.
pub fn is_service_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b"_.@:+-".contains(&b))
}

fn validate_token(value: &str, field: &str) -> PResult<()> {
    if !is_token(value) {
        return invalid(format!(
            "{field} must be a command name, not a path or shell expression"
        ));
    }
    Ok(())
}

fn validate_service_name(value: &str, field: &str) -> PResult<()> {
    if !is_service_name(value) {
        return invalid(format!(
            "{field} must be a systemd unit name, not an option or path"
        ));
    }
    Ok(())
}

fn validate_version_flag(value: &str, trusted_registry_command: bool) -> PResult<Vec<String>> {
    let parts = match shlex::split(value) {
        Ok(parts) => parts,
        Err(exc) => return invalid(format!("version_flag is invalid: {exc}")),
    };
    if parts.is_empty() {
        return invalid("version_flag must not be empty");
    }
    if parts.len() > MAX_VERSION_ARGS {
        return invalid("version_flag has too many arguments");
    }
    for part in &parts {
        check_string(part, "version_flag[]")?;
        if !trusted_registry_command && part.contains(UNSAFE_FLAG_CHARS) {
            return invalid("custom version_flag contains shell-control characters");
        }
    }
    if !trusted_registry_command && parts.iter().any(|p| p == "-c" || p == "--command") {
        return invalid("custom profile commands cannot pass interpreter command flags");
    }
    Ok(parts)
}

fn resolve_tool(entry: &Value) -> PResult<ToolDef> {
    let Value::Table(entry) = entry else {
        return invalid("tools entries must be TOML tables");
    };
    reject_unknown_keys(entry, TOOL_KEYS, "tools[]")?;

    let name = string_field(entry.get("name"), "tools[].name", None)?;
    validate_token(&name, "tools[].name")?;
    let registry_def = registry().get(&name);

    let category = string_field(
        entry.get("category"),
        "tools[].category",
        Some(registry_def.map_or("Custom", |d| d.category.as_str())),
    )?;
    let binary = string_field(
        entry.get("binary"),
        "tools[].binary",
        Some(registry_def.map_or(name.as_str(), |d| d.binary.as_str())),
    )?;
    validate_token(&binary, "tools[].binary")?;
    let version_flag = string_field(
        entry.get("version_flag"),
        "tools[].version_flag",
        Some(registry_def.map_or("--version", |d| d.version_flag.as_str())),
    )?;
    let version_source = string_field(
        entry.get("version_source"),
        "tools[].version_source",
        Some(registry_def.map_or("stdout", |d| d.version_source.as_str())),
    )?;
    if version_source != "stdout" && version_source != "stderr" {
        return invalid("tools[].version_source must be stdout or stderr");
    }
    let default_aliases: &[String] = registry_def.map_or(&[], |d| d.aliases.as_slice());
    let aliases = string_list(
        entry.get("aliases"),
        "tools[].aliases",
        default_aliases,
        MAX_ALIASES,
    )?;
    for alias in &aliases {
        validate_token(alias, "tools[].aliases[]")?;
    }

    let trusted = registry_def.is_some_and(|d| {
        binary == d.binary && version_flag == d.version_flag && aliases == d.aliases
    });
    validate_version_flag(&version_flag, trusted)?;
    if !trusted && UNSAFE_CUSTOM_BINARIES.contains(&binary.as_str()) {
        return invalid(format!("custom profile binary '{binary}' is not allowed"));
    }

    Ok(ToolDef {
        name,
        binary,
        category,
        version_flag,
        version_source,
        aliases,
    })
}

/// Parse a TOML profile document into a [`Profile`].
pub fn parse_profile(data: &Table, max_depth: i64) -> PResult<Profile> {
    assert_profile_depth(data, max_depth)?;
    reject_unknown_keys(data, TOP_LEVEL_KEYS, "profile document")?;

    let empty = Table::new();
    let meta = match data.get("profile") {
        None => &empty,
        Some(Value::Table(t)) => t,
        Some(_) => return invalid("[profile] must be a TOML table"),
    };
    reject_unknown_keys(meta, PROFILE_KEYS, "[profile]")?;
    let name = string_field(meta.get("name"), "profile.name", Some("custom"))?;
    let description = string_field(meta.get("description"), "profile.description", Some(""))?;

    let no_tools = Vec::new();
    let tool_entries = match data.get("tools") {
        None => &no_tools,
        Some(Value::Array(items)) => items,
        Some(_) => return invalid("tools must be an array of TOML tables"),
    };
    if tool_entries.len() > MAX_TOOLS {
        return invalid("profile has too many tools");
    }
    let mut tools = Vec::new();
    let mut required = BTreeSet::new();
    let mut normalized_names: HashMap<String, String> = HashMap::new();
    for entry in tool_entries {
        let tool = resolve_tool(entry)?;
        let normalized = casefold(&tool.name);
        if let Some(first) = normalized_names.get(&normalized) {
            return invalid(format!(
                "duplicate normalized tool name: {} conflicts with {}",
                repr(&tool.name),
                repr(first)
            ));
        }
        normalized_names.insert(normalized, tool.name.clone());
        let is_required = match entry {
            Value::Table(t) => bool_field(t.get("required"), "tools[].required")?,
            _ => false,
        };
        if is_required {
            required.insert(tool.name.clone());
        }
        tools.push(tool);
    }

    let services_section = match data.get("services") {
        None => &empty,
        Some(Value::Table(t)) => t,
        Some(_) => return invalid("[services] must be a TOML table"),
    };
    reject_unknown_keys(services_section, SERVICE_KEYS, "[services]")?;
    let system = string_list(
        services_section.get("system"),
        "services.system",
        &[],
        MAX_SERVICES,
    )?;
    let user = string_list(
        services_section.get("user"),
        "services.user",
        &[],
        MAX_SERVICES,
    )?;
    if system.len() + user.len() > MAX_SERVICES {
        return invalid("profile has too many services");
    }
    let mut services = Vec::new();
    for svc in system {
        validate_service_name(&svc, "services.system[]")?;
        services.push((svc, false));
    }
    for svc in user {
        validate_service_name(&svc, "services.user[]")?;
        services.push((svc, true));
    }

    Ok(Profile {
        name,
        description,
        tools,
        required_tools: required,
        services,
    })
}

fn parse_text(text: &str, max_depth: i64) -> PResult<Profile> {
    let data = toml::loads(text).map_err(|e| match e {
        TomlError::Decode(msg) | TomlError::Recursion(msg) => ProfileError::Invalid(msg),
    })?;
    parse_profile(&data, max_depth)
}

/// Load a built-in TOML profile by name.
pub fn load_builtin_profile(name: &str, max_depth: i64) -> PResult<Profile> {
    validate_token(name, "profile name")?;
    let Some((_, text)) = BUILTIN_PROFILES.iter().find(|(n, _)| *n == name) else {
        return Err(ProfileError::NotFound(format!("{name}.toml")));
    };
    parse_text(text, max_depth)
}

/// Return names of all built-in profiles (sorted).
pub fn list_builtin_profiles() -> Vec<&'static str> {
    BUILTIN_PROFILES.iter().map(|(n, _)| *n).collect()
}

fn home_from_passwd(user: Option<&str>) -> Option<String> {
    let passwd = crate::pycompat::fsdecode(&std::fs::read("/etc/passwd").ok()?);
    let uid = crate::sys::getuid().to_string();
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 7 {
            return None;
        }
        let matches = match user {
            Some(u) => fields[0] == u,
            None => fields[2] == uid,
        };
        matches.then(|| fields[5].to_string())
    })
}

/// `pathlib.Path(path).expanduser().absolute()` rendered as a string.
fn expand_and_absolute(path: &str) -> PResult<String> {
    // pathlib normalization: collapse duplicate slashes and "." parts,
    // keep a leading "//", drop trailing slashes. ".." is preserved.
    fn pathlib_normalize(path: &str) -> String {
        if path.is_empty() {
            return ".".to_string();
        }
        let root = if path.starts_with("//") && !path.starts_with("///") {
            "//"
        } else if path.starts_with('/') {
            "/"
        } else {
            ""
        };
        let parts: Vec<&str> = path
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect();
        let joined = format!("{root}{}", parts.join("/"));
        if joined.is_empty() {
            ".".to_string()
        } else {
            joined
        }
    }

    let mut normalized = pathlib_normalize(path);
    if !normalized.starts_with('/') && normalized.starts_with('~') {
        let (first, rest) = match normalized.find('/') {
            Some(i) => (&normalized[..i], &normalized[i..]),
            None => (normalized.as_str(), ""),
        };
        let home = if first == "~" {
            match std::env::var_os("HOME") {
                Some(h) => {
                    use std::os::unix::ffi::OsStrExt;
                    Some(crate::pycompat::fsdecode(h.as_bytes()))
                }
                None => home_from_passwd(None),
            }
        } else {
            home_from_passwd(Some(&first[1..]))
        };
        let Some(home) = home else {
            return invalid("Could not determine home directory.");
        };
        let home = home.trim_end_matches('/');
        let expanded = format!("{home}{rest}");
        let expanded = if expanded.is_empty() {
            "/".to_string()
        } else {
            expanded
        };
        normalized = pathlib_normalize(&expanded);
    }
    if normalized.starts_with('/') {
        return Ok(normalized);
    }
    // `os.getcwd()` raising FileNotFoundError (deleted working directory) is
    // reported by the CLI as "profile not found".
    let cwd = std::env::current_dir().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ProfileError::NotFound(crate::sys::strerror(&e))
        } else {
            ProfileError::Invalid(crate::sys::strerror(&e))
        }
    })?;
    let cwd = {
        use std::os::unix::ffi::OsStrExt;
        crate::pycompat::fsdecode(cwd.as_os_str().as_bytes())
    };
    if normalized == "." {
        return Ok(cwd);
    }
    Ok(pathlib_normalize(&format!("{cwd}/{normalized}")))
}

fn path_suffix(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or("");
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    }
}

/// Read at most `MAX_PROFILE_BYTES + 1` bytes and reject oversized input.
fn read_bounded(reader: &mut impl Read, resolved: &str) -> PResult<Vec<u8>> {
    let mut raw = Vec::new();
    reader
        .take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|e| {
            ProfileError::Invalid(format!(
                "Profile cannot be opened: {resolved}: {}",
                crate::sys::strerror(&e)
            ))
        })?;
    if raw.len() as u64 > MAX_PROFILE_BYTES {
        return invalid(format!("Profile is too large: {resolved}"));
    }
    Ok(raw)
}

/// Load a custom TOML profile from a file path.
pub fn load_custom_profile(path: &str, max_depth: i64) -> PResult<Profile> {
    load_custom_profile_with(path, max_depth, |_| {}, |file| Box::new(file))
}

/// Test seam: `after_open` runs once the handle is open (to simulate path
/// replacement) and `wrap` can substitute the reader (to simulate growth).
pub(crate) fn load_custom_profile_with(
    path: &str,
    max_depth: i64,
    after_open: impl FnOnce(&Path),
    wrap: impl FnOnce(File) -> Box<dyn Read>,
) -> PResult<Profile> {
    let resolved = expand_and_absolute(path)?;
    if path_suffix(&resolved) != ".toml" {
        return invalid(format!("Profile must be a .toml file: {resolved}"));
    }
    validate_max_depth(max_depth)?;
    let resolved_path = crate::pycompat::os_path(&resolved);
    let file = match File::open(&resolved_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProfileError::NotFound(format!(
                "Profile not found: {resolved}"
            )));
        }
        Err(e) => {
            return invalid(format!(
                "Profile cannot be opened: {resolved}: {}",
                crate::sys::strerror(&e)
            ));
        }
    };
    let metadata = file.metadata().map_err(|e| {
        ProfileError::Invalid(format!(
            "Profile cannot be opened: {resolved}: {}",
            crate::sys::strerror(&e)
        ))
    })?;
    if metadata.is_dir() {
        // CPython's open() raises IsADirectoryError for directories.
        return invalid(format!(
            "Profile cannot be opened: {resolved}: Is a directory"
        ));
    }
    after_open(&resolved_path);
    if !metadata.file_type().is_file() {
        return invalid(format!("Profile must be a regular file: {resolved}"));
    }
    if metadata.len() > MAX_PROFILE_BYTES {
        return invalid(format!("Profile is too large: {resolved}"));
    }
    let mut reader = wrap(file);
    let raw = read_bounded(&mut reader, &resolved)?;
    let Ok(text) = String::from_utf8(raw) else {
        return invalid(format!("Profile must be UTF-8: {resolved}"));
    };
    parse_text(&text, max_depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn write(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn invalid_msg(result: PResult<Profile>) -> String {
        match result {
            Err(ProfileError::Invalid(msg)) => msg,
            other => panic!("expected invalid profile, got {other:?}"),
        }
    }

    #[test]
    fn test_list_builtin_profiles() {
        let profiles = list_builtin_profiles();
        for name in [
            "full",
            "python-dev",
            "node-dev",
            "rust-dev",
            "devops",
            "sysadmin",
        ] {
            assert!(profiles.contains(&name), "{name}");
        }
        let mut sorted = profiles.clone();
        sorted.sort_unstable();
        assert_eq!(profiles, sorted);
    }

    #[test]
    fn test_load_full_profile() {
        let profile = load_builtin_profile("full", MAX_PROFILE_DEPTH).unwrap();
        assert_eq!(profile.name, "full");
        assert!(profile.tools.len() > 50);
        assert_eq!(profile.tools.len(), 103);
    }

    #[test]
    fn test_load_python_dev_profile() {
        let profile = load_builtin_profile("python-dev", MAX_PROFILE_DEPTH).unwrap();
        assert_eq!(profile.name, "python-dev");
        let names: Vec<&str> = profile.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"python3"));
        assert!(names.contains(&"pip"));
        assert!(profile.required_tools.contains("python3"));
    }

    #[test]
    fn test_load_devops_profile() {
        let profile = load_builtin_profile("devops", MAX_PROFILE_DEPTH).unwrap();
        assert_eq!(profile.name, "devops");
        assert!(profile.required_tools.contains("docker"));
        assert!(!profile.services.is_empty());
    }

    #[test]
    fn test_profile_resolves_registry_defaults() {
        let profile = load_builtin_profile("python-dev", MAX_PROFILE_DEPTH).unwrap();
        let python = profile.tools.iter().find(|t| t.name == "python3").unwrap();
        assert_eq!(python.binary, "python3");
        assert_eq!(python.version_flag, "--version");
    }

    #[test]
    fn test_profile_services() {
        let profile = load_builtin_profile("devops", MAX_PROFILE_DEPTH).unwrap();
        let names: Vec<&str> = profile.services.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"sshd") || names.contains(&"docker"));
    }

    #[test]
    fn builtin_profile_tool_counts() {
        let counts: Vec<(&str, usize)> = list_builtin_profiles()
            .into_iter()
            .map(|n| {
                (
                    n,
                    load_builtin_profile(n, MAX_PROFILE_DEPTH)
                        .unwrap()
                        .tools
                        .len(),
                )
            })
            .collect();
        assert_eq!(
            counts,
            vec![
                ("devops", 20),
                ("full", 103),
                ("node-dev", 13),
                ("python-dev", 12),
                ("rust-dev", 11),
                ("sysadmin", 22)
            ]
        );
    }

    #[test]
    fn builtin_profile_errors() {
        assert_eq!(
            load_builtin_profile("nonexistent", 16),
            Err(ProfileError::NotFound("nonexistent.toml".into()))
        );
        assert_eq!(
            invalid_msg(load_builtin_profile("../x", 16)),
            "profile name must be a command name, not a path or shell expression"
        );
    }

    #[test]
    fn test_custom_profile_rejects_interpreter_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "unsafe.toml",
            "\n        [[tools]]\n        name = \"owned\"\n        binary = \"sh\"\n        version_flag = \"-c id\"\n        ",
        );
        let msg = invalid_msg(load_custom_profile(&path, 16));
        assert!(
            msg.contains("not allowed") || msg.contains("interpreter"),
            "{msg}"
        );
        assert_eq!(
            msg,
            "custom profile commands cannot pass interpreter command flags"
        );
    }

    #[test]
    fn test_custom_profile_rejects_option_like_service() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "unsafe-service.toml",
            "[services]\nsystem = [\"--user\"]\n",
        );
        let msg = invalid_msg(load_custom_profile(&path, 16));
        assert!(msg.contains("systemd unit name"), "{msg}");
    }

    #[test]
    fn test_custom_profile_validates_schema_types() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "bad-schema.toml", "tools = \"python3\"\n");
        let msg = invalid_msg(load_custom_profile(&path, 16));
        assert!(msg.contains("tools must be an array"), "{msg}");
    }

    #[test]
    fn test_custom_profile_loads_safe_custom_tool() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "safe.toml",
            "[profile]\nname = \"safe\"\n\n[[tools]]\nname = \"custom-tool\"\nbinary = \"custom-tool\"\ncategory = \"Custom\"\nversion_flag = \"--version\"\naliases = [\"custom-tool2\"]\nrequired = true\n",
        );
        let profile = load_custom_profile(&path, 16).unwrap();
        assert_eq!(profile.name, "safe");
        assert_eq!(profile.tools[0].binary, "custom-tool");
        assert_eq!(profile.tools[0].aliases, vec!["custom-tool2".to_string()]);
        assert_eq!(
            profile.required_tools,
            BTreeSet::from(["custom-tool".to_string()])
        );
    }

    #[test]
    fn test_custom_profile_rejects_unknown_keys() {
        let cases = [
            ("mystery = true\n", "profile document"),
            ("[profile]\nname = 'x'\nmystery = true\n", "profile"),
            ("[[tools]]\nname = 'x'\nmystery = true\n", "tools"),
            ("[services]\nsystem = []\nmystery = []\n", "services"),
        ];
        for (document, field) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = write(dir.path(), "unknown.toml", document);
            let msg = invalid_msg(load_custom_profile(&path, 16));
            let field_at = msg.find(field).unwrap_or_else(|| panic!("{msg}"));
            assert!(msg[field_at..].contains("unknown key"), "{msg}");
        }
    }

    #[test]
    fn test_custom_profile_rejects_duplicate_normalized_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "duplicate.toml",
            "[[tools]]\nname = \"Custom-Tool\"\n\n[[tools]]\nname = \"custom-tool\"\n",
        );
        let msg = invalid_msg(load_custom_profile(&path, 16));
        assert_eq!(
            msg,
            "duplicate normalized tool name: 'custom-tool' conflicts with 'Custom-Tool'"
        );
    }

    #[test]
    fn test_custom_profile_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.toml");
        std::fs::write(&path, vec![b'#'; MAX_PROFILE_BYTES as usize + 1]).unwrap();
        let msg = invalid_msg(load_custom_profile(path.to_str().unwrap(), 16));
        assert!(msg.contains("too large"), "{msg}");
    }

    #[test]
    fn test_custom_profile_detects_growth_after_fstat() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "growing.toml", "[profile]\nname = 'original'\n");
        let grow_path = PathBuf::from(&path);
        let result = load_custom_profile_with(
            &path,
            16,
            |_| {},
            move |file| {
                let mut writer = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&grow_path)
                    .unwrap();
                writer
                    .write_all(&vec![b'#'; MAX_PROFILE_BYTES as usize + 1])
                    .unwrap();
                Box::new(file)
            },
        );
        assert!(invalid_msg(result).contains("too large"));
    }

    #[test]
    fn test_custom_profile_parses_the_exact_opened_file_when_path_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "replace.toml", "[profile]\nname = 'opened'\n");
        let replacement = write(
            dir.path(),
            "replacement.toml",
            "[profile]\nname = 'replacement'\n",
        );
        let profile = load_custom_profile_with(
            &path,
            16,
            |opened| std::fs::rename(&replacement, opened).unwrap(),
            |file| Box::new(file),
        )
        .unwrap();
        assert_eq!(profile.name, "opened");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("replacement")
        );
    }

    #[test]
    fn test_custom_profile_rejects_invalid_max_depth() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "depth.toml", "[profile]\nname = 'depth'\n");
        // Python also rejected 1.5 and True; the Rust API takes an integer,
        // so those cases are excluded by the type system.
        for max_depth in [0, -1, MAX_PROFILE_DEPTH + 1] {
            let msg = invalid_msg(load_custom_profile(&path, max_depth));
            assert!(msg.contains("max_depth"), "{msg}");
        }
    }

    #[test]
    fn test_custom_profile_enforces_max_depth() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "depth.toml", "[[tools]]\nname = 'tool'\n");
        let msg = invalid_msg(load_custom_profile(&path, 2));
        assert!(msg.contains("nesting exceeds"), "{msg}");
    }

    #[test]
    fn custom_profile_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.toml");
        assert!(matches!(
            load_custom_profile(missing.to_str().unwrap(), 16),
            Err(ProfileError::NotFound(_))
        ));
        let msg = invalid_msg(load_custom_profile("/etc/passwd", 16));
        assert_eq!(msg, "Profile must be a .toml file: /etc/passwd");
        let sub = dir.path().join("dir.toml");
        std::fs::create_dir(&sub).unwrap();
        let msg = invalid_msg(load_custom_profile(sub.to_str().unwrap(), 16));
        assert!(msg.ends_with(": Is a directory"), "{msg}");
        let bad = write(dir.path(), "bad.toml", "");
        std::fs::write(&bad, b"\xff").unwrap();
        assert!(invalid_msg(load_custom_profile(&bad, 16)).starts_with("Profile must be UTF-8: "));
    }

    #[test]
    fn suffix_and_normalization() {
        assert_eq!(path_suffix("/a/b.toml"), ".toml");
        assert_eq!(path_suffix("/a/.toml"), "");
        assert_eq!(path_suffix("/a/b.toml."), "");
        assert_eq!(
            expand_and_absolute("/a//b/./c.toml/").unwrap(),
            "/a/b/c.toml"
        );
        assert_eq!(expand_and_absolute("/a/../c.toml").unwrap(), "/a/../c.toml");
    }
}
