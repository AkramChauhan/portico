//! ngrok support, as an alternative to cloudflared.
//!
//! Unlike a Cloudflare quick tunnel, ngrok **requires an account**: without an
//! authtoken the agent refuses to start (`ERR_NGROK_4018`). That is why it is
//! never the default and never appears during onboarding — it would put a
//! sign-up wall in front of the first thing a new user tries.
//!
//! It earns its place for people who already pay for it: a paid plan gives a
//! stable hostname, which fixes the one real weakness of quick tunnels — the
//! URL changing on every restart.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::process::Command;

/// What we can tell about the local ngrok installation.
#[derive(Debug, Clone, Serialize)]
pub struct NgrokStatus {
    pub installed: bool,
    pub path: Option<String>,
    /// True when an authtoken is present, which is what the agent requires.
    pub configured: bool,
    pub config_path: Option<String>,
    /// "free", "paid" or None when no tunnel has been observed yet.
    pub plan: Option<String>,
    /// Custom hostnames need a paid plan.
    pub custom_domain_supported: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Cloudflare,
    Ngrok,
}

impl Default for Provider {
    fn default() -> Self {
        // cloudflared needs no account, so it stays the default.
        Provider::Cloudflare
    }
}

pub fn bin() -> Option<String> {
    paths::find_bin("ngrok")
}

/// Where ngrok keeps its configuration, asked of ngrok itself rather than
/// guessed — the location differs across versions and platforms.
fn config_path() -> Option<String> {
    let out = Command::new(bin()?).args(["config", "check"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "Valid configuration file at /Users/x/Library/.../ngrok.yml"
    let path = text.split(" at ").nth(1)?.trim().to_string();
    (!path.is_empty()).then_some(path)
}

fn has_authtoken(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|c| {
            c.lines()
                .any(|l| l.trim_start().starts_with("authtoken:") && l.split(':').nth(1).is_some_and(|v| !v.trim().is_empty()))
        })
        .unwrap_or(false)
}

/// A hostname tells you the plan: the free tier is always on ngrok-free.app.
pub fn plan_from_url(url: &str) -> &'static str {
    if url.contains("ngrok-free.") {
        "free"
    } else {
        "paid"
    }
}

/// Pull the public URL out of an ngrok logfmt line.
pub fn extract_url(line: &str) -> Option<String> {
    if !line.contains("started tunnel") {
        return None;
    }
    let idx = line.find("url=")?;
    let rest = &line[idx + 4..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', '"']).to_string();
    url.starts_with("http").then_some(url)
}

pub fn status(cached_plan: Option<&str>) -> NgrokStatus {
    let path = bin();
    let installed = path.is_some();
    let config_path = if installed { config_path() } else { None };
    let configured = config_path.as_deref().is_some_and(has_authtoken);

    let plan = cached_plan.map(str::to_string);
    let paid = plan.as_deref() == Some("paid");

    let detail = if !installed {
        "Not installed".to_string()
    } else if !configured {
        "Installed, but no authtoken — ngrok needs an account".to_string()
    } else {
        match plan.as_deref() {
            Some("paid") => "Ready · paid plan — custom domains available".to_string(),
            Some("free") => "Ready · free plan — the hostname changes each run".to_string(),
            _ => "Ready · run a check to detect your plan".to_string(),
        }
    };

    NgrokStatus {
        installed,
        path,
        configured,
        config_path,
        plan,
        custom_domain_supported: paid,
        detail,
    }
}

/// The command used to expose a site.
///
/// Points at the same loopback twin cloudflared uses, with the site's real
/// hostname in the Host header, so a request arrives at exactly the handler a
/// local one would.
pub fn tunnel_args(port: u16, domain: &str, custom_domain: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "http".to_string(),
        port.to_string(),
        format!("--host-header={domain}"),
        "--log".to_string(),
        "stdout".to_string(),
        "--log-format".to_string(),
        "logfmt".to_string(),
    ];
    if let Some(d) = custom_domain.filter(|d| !d.trim().is_empty()) {
        args.push(format!("--domain={}", d.trim()));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_url_out_of_a_real_log_line() {
        let line = r#"t=2026-09-09T04:05:04+0530 lvl=info msg="started tunnel" obj=tunnels name=command_line addr=http://localhost:8880 url=https://04eb-2402-3a80.ngrok-free.app"#;
        assert_eq!(
            extract_url(line).as_deref(),
            Some("https://04eb-2402-3a80.ngrok-free.app")
        );
    }

    #[test]
    fn ignores_lines_that_are_not_a_tunnel_starting() {
        assert!(extract_url(r#"lvl=info msg="session closing" url=https://x.ngrok.app"#).is_none());
        assert!(extract_url("no url here").is_none());
    }

    #[test]
    fn the_hostname_reveals_the_plan() {
        // The free tier is always on ngrok-free.app; anything else is paid,
        // including a custom domain.
        assert_eq!(plan_from_url("https://04eb-1-2.ngrok-free.app"), "free");
        assert_eq!(plan_from_url("https://steady-name.ngrok.app"), "paid");
        assert_eq!(plan_from_url("https://hooks.mycompany.com"), "paid");
    }

    #[test]
    fn a_custom_domain_is_only_passed_when_given() {
        let plain = tunnel_args(8880, "site.io", None);
        assert!(plain.contains(&"--host-header=site.io".to_string()));
        assert!(!plain.iter().any(|a| a.starts_with("--domain=")));

        let custom = tunnel_args(8880, "site.io", Some("hooks.example.com"));
        assert!(custom.contains(&"--domain=hooks.example.com".to_string()));

        // Whitespace-only must not become an empty --domain, which ngrok rejects.
        let blank = tunnel_args(8880, "site.io", Some("   "));
        assert!(!blank.iter().any(|a| a.starts_with("--domain=")));
    }
}
