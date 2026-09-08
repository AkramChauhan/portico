//! Version reporting and updates, for the app and for the tools it manages.

use crate::paths;
use serde::Serialize;
use std::process::Command;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
pub struct ToolVersion {
    pub name: String,
    pub installed: Option<String>,
    pub latest: Option<String>,
    /// True when a newer release exists *and* we are allowed to replace it.
    pub can_update: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateReport {
    pub app_version: String,
    pub app_latest: Option<String>,
    pub app_update_available: bool,
    pub app_detail: String,
    /// False when there is no published release feed to compare against yet.
    pub app_channel: bool,
    pub tools: Vec<ToolVersion>,
}

fn run_capture(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let combined = if text.trim().is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        text.into_owned()
    };
    combined.lines().next().map(str::to_string)
}

/// Pull a dotted version out of a `--version` line.
pub fn extract_version(line: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for token in line.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        let t = token.trim_start_matches('v');
        if t.contains('.') && t.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            best.get_or_insert_with(|| t.to_string());
        }
    }
    best
}

/// Latest release tag for a GitHub repository.
fn github_latest(repo: &str) -> Option<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let out = Command::new("/usr/bin/curl")
        .args([
            "-fsSL",
            "--max-time",
            "12",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: Portico",
            &url,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).ok()?;
    json.get("tag_name")?
        .as_str()
        .map(|t| t.trim_start_matches('v').to_string())
}

fn tool_version(name: &str, arg: &str) -> Option<String> {
    let bin = paths::find_bin(name)?;
    extract_version(&run_capture(&bin, &[arg])?)
}

pub fn check(app_repo: Option<&str>) -> UpdateReport {
    let tools = [("caddy", "version"), ("cloudflared", "--version")]
        .into_iter()
        .map(|(name, arg)| {
            let installed = tool_version(name, arg);
            let detail = match (&installed, paths::find_bin(name)) {
                (Some(v), Some(path)) => format!("{v} — {path}"),
                (Some(v), None) => v.clone(),
                (None, _) => "Not installed".to_string(),
            };
            ToolVersion { name: name.to_string(), installed, latest: None, can_update: false, detail }
        })
        .collect();

    let app_latest = app_repo.and_then(github_latest);
    let app_channel = app_latest.is_some();
    let app_detail = match &app_latest {
        Some(l) if l != APP_VERSION => format!("{APP_VERSION} → {l} available"),
        _ => APP_VERSION.to_string(),
    };

    UpdateReport {
        app_version: APP_VERSION.to_string(),
        app_latest,
        app_update_available: app_channel && app_detail.contains('→'),
        app_detail,
        app_channel,
        tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulls_versions_out_of_real_version_lines() {
        assert_eq!(
            extract_version("v2.11.4 h1:XKxkMTgNSizEvKG6QHue6cAsFOteU2qA61w2tKkCWi0=").as_deref(),
            Some("2.11.4")
        );
        assert_eq!(
            extract_version("cloudflared version 2026.8.3 (built 2026-08-31-10:06 UTC)").as_deref(),
            Some("2026.8.3")
        );
    }

    #[test]
    fn ignores_lines_without_a_version() {
        assert!(extract_version("no version here").is_none());
        assert!(extract_version("").is_none());
    }

    #[test]
    fn does_not_mistake_a_hash_or_date_for_a_version() {
        // Build hashes and dates must not be picked up as the version.
        assert_eq!(extract_version("built 2026-08-31-10:06 v1.2.3").as_deref(), Some("1.2.3"));
    }
}
