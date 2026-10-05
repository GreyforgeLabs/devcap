//! Output formatters — text, json, markdown.

use crate::json;
use crate::registry::CATEGORIES;
use crate::safe_text::{clean_text, markdown_cell, markdown_inline_code};
use crate::scanner::{ScanResult, ToolResult};

/// Group tool results by category, preserving `CATEGORIES` order, then any
/// custom categories in first-seen order.
fn group_by_category(results: &[ToolResult]) -> Vec<(&str, Vec<&ToolResult>)> {
    let mut groups: Vec<(&str, Vec<&ToolResult>)> = Vec::new();
    for r in results {
        match groups.iter_mut().find(|(c, _)| *c == r.category) {
            Some((_, items)) => items.push(r),
            None => groups.push((&r.category, vec![r])),
        }
    }
    let mut ordered: Vec<(&str, Vec<&ToolResult>)> = Vec::new();
    for cat in CATEGORIES {
        if let Some(i) = groups.iter().position(|(c, _)| *c == cat) {
            ordered.push(groups.remove(i));
        }
    }
    ordered.extend(groups);
    ordered
}

fn pad(value: &str, width: usize) -> String {
    format!("{value:<width$}")
}

/// Format scan results as human-readable columnar text.
pub fn format_text(scan: &ScanResult) -> String {
    let hostname = clean_text(&scan.hostname, Some(120));
    let timestamp = clean_text(&scan.timestamp, Some(80));
    let mut lines = vec![
        format!("devcap scan — {hostname} — {timestamp}"),
        format!("Platform: {}", clean_text(&scan.platform, Some(160))),
        String::new(),
    ];

    for (category, tools) in group_by_category(&scan.results) {
        let found: Vec<_> = tools.iter().filter(|t| t.found).collect();
        let missing: Vec<_> = tools.iter().filter(|t| !t.found).collect();
        lines.push(format!("=== {} ===", clean_text(category, Some(80))));
        for t in &found {
            let name = clean_text(&t.name, Some(32));
            let version = clean_text(nonempty_or(t.version.as_deref(), "?"), Some(80));
            let path = clean_text(t.path.as_deref().unwrap_or(""), Some(240));
            lines.push(format!(
                "  {} {} {}",
                pad(&name, 16),
                pad(&version, 20),
                path
            ));
        }
        if !missing.is_empty() {
            lines.push("  Missing:".to_string());
            for t in &missing {
                lines.push(format!("    {}", clean_text(&t.name, Some(80))));
            }
        }
        lines.push(String::new());
    }

    if !scan.services.is_empty() {
        lines.push("=== Services ===".to_string());
        for svc in &scan.services {
            let status = if svc.active { "running" } else { "stopped" };
            let suffix = if svc.user_service { " (user)" } else { "" };
            lines.push(format!(
                "  [{status}] {}{suffix}",
                clean_text(&svc.name, Some(120))
            ));
        }
        lines.push(String::new());
    }

    let found_count = scan.results.iter().filter(|r| r.found).count();
    lines.push(format!("Found {found_count}/{} tools", scan.results.len()));
    lines.join("\n")
}

/// Python's `value or default` for an optional string.
fn nonempty_or<'a>(value: Option<&'a str>, default: &'a str) -> &'a str {
    match value {
        Some(v) if !v.is_empty() => v,
        _ => default,
    }
}

/// Format scan results as JSON.
pub fn format_json(scan: &ScanResult) -> String {
    json::dumps(&scan.to_json())
}

/// Format scan results as markdown tables.
pub fn format_markdown(scan: &ScanResult) -> String {
    let timestamp = markdown_cell(&scan.timestamp, Some(80));
    let platform = markdown_cell(&scan.platform, Some(160));
    let mut lines = vec![
        format!(
            "# Development Environment — {}",
            markdown_cell(&scan.hostname, Some(120))
        ),
        String::new(),
        format!("> Scanned: {timestamp} | Platform: {platform}"),
        String::new(),
    ];

    for (category, tools) in group_by_category(&scan.results) {
        let found: Vec<_> = tools.iter().filter(|t| t.found).collect();
        let missing: Vec<_> = tools.iter().filter(|t| !t.found).collect();
        lines.push(format!("## {}", markdown_cell(category, Some(80))));
        lines.push(String::new());
        if !found.is_empty() {
            lines.push("| Tool | Version | Path |".to_string());
            lines.push("|------|---------|------|".to_string());
            for t in &found {
                let name = markdown_cell(&t.name, Some(80));
                let version = markdown_cell(nonempty_or(t.version.as_deref(), "?"), Some(80));
                let path = markdown_cell(t.path.as_deref().unwrap_or(""), Some(240));
                lines.push(format!("| {name} | {version} | {path} |"));
            }
            lines.push(String::new());
        }
        if !missing.is_empty() {
            let names: Vec<String> = missing
                .iter()
                .map(|t| markdown_inline_code(&t.name, Some(120)))
                .collect();
            lines.push(format!("**Not installed**: {}", names.join(", ")));
            lines.push(String::new());
        }
    }

    if !scan.services.is_empty() {
        lines.push("## Services".to_string());
        lines.push(String::new());
        lines.push("| Service | Status |".to_string());
        lines.push("|---------|--------|".to_string());
        for svc in &scan.services {
            let status = if svc.active { "running" } else { "stopped" };
            let suffix = if svc.user_service { " (user)" } else { "" };
            lines.push(format!(
                "| {}{suffix} | {status} |",
                markdown_cell(&svc.name, Some(120))
            ));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Format by name (`text`, `json`, `markdown`).
pub fn format_by_name(name: &str, scan: &ScanResult) -> String {
    match name {
        "json" => format_json(scan),
        "markdown" => format_markdown(scan),
        _ => format_text(scan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Json, parse};
    use crate::scanner::ServiceResult;

    fn tool(name: &str, category: &str, version: Option<&str>, path: Option<&str>) -> ToolResult {
        ToolResult {
            name: name.into(),
            binary: name.into(),
            category: category.into(),
            found: path.is_some(),
            version: version.map(Into::into),
            path: path.map(Into::into),
            ..Default::default()
        }
    }

    fn make_scan() -> ScanResult {
        ScanResult {
            hostname: "testhost".into(),
            timestamp: "2026-01-01T00:00:00+00:00".into(),
            platform: "Linux 6.0.0".into(),
            results: vec![
                tool(
                    "python3",
                    "Languages",
                    Some("3.12.3"),
                    Some("/usr/bin/python3"),
                ),
                tool("rustc", "Languages", None, None),
                tool(
                    "git",
                    "Version Control",
                    Some("2.43.0"),
                    Some("/usr/bin/git"),
                ),
            ],
            services: vec![
                ServiceResult {
                    name: "sshd".into(),
                    active: true,
                    user_service: false,
                },
                ServiceResult {
                    name: "docker".into(),
                    active: false,
                    user_service: false,
                },
            ],
        }
    }

    #[test]
    fn test_format_text() {
        let text = format_text(&make_scan());
        for needle in [
            "testhost",
            "python3",
            "3.12.3",
            "rustc",
            "Missing:",
            "sshd",
            "Found 2/3 tools",
        ] {
            assert!(text.contains(needle), "{needle}");
        }
        assert_eq!(
            text,
            "devcap scan — testhost — 2026-01-01T00:00:00+00:00\nPlatform: Linux 6.0.0\n\n=== Languages ===\n  python3          3.12.3               /usr/bin/python3\n  Missing:\n    rustc\n\n=== Version Control ===\n  git              2.43.0               /usr/bin/git\n\n=== Services ===\n  [running] sshd\n  [stopped] docker\n\nFound 2/3 tools"
        );
    }

    #[test]
    fn test_format_json() {
        let data = parse(&format_json(&make_scan())).unwrap();
        assert_eq!(data.get("hostname"), Some(&Json::str("testhost")));
        let Some(Json::Array(tools)) = data.get("tools") else {
            panic!("tools")
        };
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].get("found"), Some(&Json::Bool(true)));
        assert_eq!(tools[1].get("found"), Some(&Json::Bool(false)));
        let Some(Json::Array(services)) = data.get("services") else {
            panic!("services")
        };
        assert_eq!(services.len(), 2);
    }

    #[test]
    fn test_format_markdown() {
        let md = format_markdown(&make_scan());
        for needle in [
            "# Development Environment",
            "| python3 |",
            "`rustc`",
            "## Services",
            "| sshd |",
        ] {
            assert!(md.contains(needle), "{needle}");
        }
    }

    #[test]
    fn test_format_text_empty() {
        let scan = ScanResult {
            hostname: "empty".into(),
            timestamp: "now".into(),
            platform: "Linux".into(),
            ..Default::default()
        };
        assert!(format_text(&scan).contains("Found 0/0 tools"));
    }

    #[test]
    fn test_format_json_roundtrip() {
        let data = parse(&format_json(&make_scan())).unwrap();
        assert!(matches!(data, Json::Object(_)));
        assert!(data.get("tools").is_some());
    }

    #[test]
    fn test_formatters_strip_control_sequences_and_escape_markdown() {
        let scan = ScanResult {
            hostname: "host\x1b[31m\nspoof".into(),
            timestamp: "2026-01-01T00:00:00+00:00".into(),
            platform: "Linux 6.0.0".into(),
            results: vec![tool(
                "bad|tool",
                "Custom|Category",
                Some("\x1b[31m1.0|spoof\x1b[0m"),
                Some("/tmp/a|b\nnext"),
            )],
            services: vec![ServiceResult {
                name: "svc|name".into(),
                active: true,
                user_service: false,
            }],
        };
        let text = format_text(&scan);
        let markdown = format_markdown(&scan);
        assert!(!text.contains('\x1b'));
        assert!(!markdown.contains('\x1b'));
        assert!(!text.contains("spoof\n"));
        assert!(markdown.contains("bad\\|tool"));
        assert!(markdown.contains("/tmp/a\\|b next"));
    }

    #[test]
    fn custom_categories_follow_known_ones() {
        let scan = ScanResult {
            results: vec![
                tool("z", "Zeta", None, None),
                tool("a", "Testing", None, None),
                tool("y", "Alpha", None, None),
            ],
            ..Default::default()
        };
        let groups: Vec<&str> = group_by_category(&scan.results)
            .iter()
            .map(|g| g.0)
            .collect();
        assert_eq!(groups, vec!["Testing", "Zeta", "Alpha"]);
    }
}
