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
//!
//! The authtoken is entered in the app and kept in the login keychain, then
//! handed to the agent through `NGROK_AUTHTOKEN` in its environment. Not
//! `--authtoken`, which would put the secret in `argv` for any local process
//! to read; and not `ngrok config add-authtoken`, which would do the same on
//! the way to writing ngrok's own config file. A token the user added that
//! way still works — it is simply found in the config file instead.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::process::Command;

/// What we can tell about the local ngrok installation.
#[derive(Debug, Clone, Serialize)]
pub struct NgrokStatus {
    pub installed: bool,
    pub path: Option<String>,
    /// True when an authtoken is present from *either* source, which is what
    /// the agent requires before it will start.
    pub configured: bool,
    /// True only when Portico itself holds the token.
    ///
    /// The distinction matters to the interface: a token sitting in ngrok's
    /// own config satisfies the agent but is not ours to remove, so offering
    /// "Disconnect" for it is a button that cannot do anything — and hiding
    /// the input field alongside it leaves no way to enter one at all.
    pub token_stored: bool,
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

/// The authtoken to run the agent with, if we hold one.
///
/// Only the copy we were given: a token in ngrok's own config file is found by
/// the agent on its own and never needs to pass through here.
pub fn authtoken() -> Option<String> {
    crate::secrets::get(crate::secrets::NGROK_AUTHTOKEN)
}

/// Write the authtoken into a config file only this account can read, and
/// return the arguments that make ngrok use it.
///
/// Empty when we hold no token, which leaves ngrok to find the user's own
/// config exactly as it would without us.
///
/// This is deliberately not `NGROK_AUTHTOKEN`. That variable is read, but
/// ngrok's default config file takes precedence over it — so with a token
/// already in `~/Library/Application Support/ngrok/ngrok.yml`, the value we
/// passed was silently ignored and a wrong token still appeared to work.
/// `--config` replaces the default instead of merging with it, so what the
/// user typed here is what actually runs.
pub fn config_args() -> Vec<String> {
    let Some(token) = authtoken() else {
        return Vec::new();
    };
    match write_config(&token) {
        Ok(path) => vec!["--config".to_string(), path.to_string_lossy().into_owned()],
        Err(_) => Vec::new(),
    }
}

/// The config file contents.
///
/// Schema 3 nests the token under `agent`; a bare top-level `authtoken` is
/// schema 2 and is rejected outright by a v3 agent with "field authtoken not
/// found in type config.v3yamlConfig". Quoted because a YAML scalar starting
/// with certain characters would otherwise change meaning — quotes themselves
/// cannot appear, the charset check in `secrets` having already refused them.
fn render_config(token: &str) -> String {
    format!("version: \"3\"\nagent:\n  authtoken: \"{token}\"\n")
}

/// Write it 0600: it holds the authtoken in clear.
fn write_config(token: &str) -> std::io::Result<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    crate::paths::ensure_dirs()?;
    let path = crate::paths::ngrok_config();
    std::fs::write(&path, render_config(token))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(path)
}

/// Delete our config file. The user's own is never touched.
pub fn forget_config() {
    let _ = std::fs::remove_file(crate::paths::ngrok_config());
}

/// Remove `token` from text on its way to a log file or the interface.
///
/// ngrok quotes a rejected authtoken back in its own error — "Your authtoken:
/// <value>" — and that error is written to `tunnel-<site>.log` and shown in
/// the app. Without this, one mistyped token ends up in plaintext in a file
/// that outlives the mistake.
pub fn redact(text: &str, token: Option<&str>) -> String {
    match token.filter(|t| !t.is_empty()) {
        Some(t) => text.replace(t, "<authtoken redacted>"),
        None => text.to_string(),
    }
}

pub fn status(cached_plan: Option<&str>) -> NgrokStatus {
    let path = bin();
    let installed = path.is_some();
    let config_path = if installed { config_path() } else { None };
    // Either source counts: ours, or one the user added with the ngrok CLI
    // before they ever opened this app.
    let in_keychain = crate::secrets::has(crate::secrets::NGROK_AUTHTOKEN);
    let configured = in_keychain || config_path.as_deref().is_some_and(has_authtoken);

    let plan = cached_plan.map(str::to_string);
    let paid = plan.as_deref() == Some("paid");

    let detail = if !installed {
        "Not installed".to_string()
    } else if !configured {
        "Installed, but no authtoken — ngrok needs an account".to_string()
    } else if !in_keychain {
        // Working, but on a token we did not put there. Say so, rather than
        // implying Portico is holding a credential it does not have.
        "Ready · using the authtoken from ngrok's own config".to_string()
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
        token_stored: in_keychain,
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

    #[test]
    fn a_rejected_authtoken_is_scrubbed_before_it_reaches_a_log() {
        // Verbatim shape of what ngrok prints when it dislikes a token: the
        // token itself, quoted back. This line is written to
        // tunnel-<site>.log and surfaced in the app, so the secret must not
        // survive the trip.
        let token = "2abcDEF_realLookingToken";
        let line = format!(
            "lvl=eror msg=\"authentication failed: The authtoken you specified does not look \
             like a proper ngrok authtoken.\\nYour authtoken: {token}\\n\" ERR_NGROK_105"
        );
        let clean = redact(&line, Some(token));
        assert!(!clean.contains(token), "the token survived redaction");
        assert!(clean.contains("<authtoken redacted>"));
        assert!(clean.contains("ERR_NGROK_105"), "the useful part must remain");
    }

    #[test]
    fn redaction_is_a_no_op_without_a_token() {
        // cloudflared output goes through the same writer; nothing to scrub.
        let line = "lvl=info msg=\"started tunnel\"";
        assert_eq!(redact(line, None), line);
        assert_eq!(redact(line, Some("")), line);
    }

    #[test]
    fn the_config_uses_the_schema_a_v3_agent_accepts() {
        // A top-level `authtoken` is schema 2. A v3 agent refuses the whole
        // file with "field authtoken not found in type config.v3yamlConfig",
        // which reads as a Portico bug rather than a config one.
        let out = render_config("2abcTOKEN_value");
        assert!(out.contains("version: \"3\""));
        assert!(out.contains("agent:"), "the token must be nested under agent");
        assert!(out.contains("  authtoken: \"2abcTOKEN_value\""));
        assert!(
            !out.lines().any(|l| l.starts_with("authtoken:")),
            "a top-level authtoken is the schema 2 form and would be rejected"
        );
    }

    #[test]
    fn an_authtoken_of_the_real_shape_is_accepted_for_storage() {
        // ngrok authtokens are base62 with an underscore joining two halves.
        // The charset guard exists for injection, and must not reject these.
        assert!(crate::secrets::plausible("2abcDEFghi123JKL456mno_7PQRstu890VWXyz12AB"));
    }
}
