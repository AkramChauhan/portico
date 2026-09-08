use crate::paths;
use crate::target::Target;
use serde::{Deserialize, Serialize};

/// Plain-HTTP listener that tunnels are pointed at. Bound to 127.0.0.1 only:
/// cloudflared connects to it locally and sets the real Host header, so a
/// tunnelled request lands on exactly the same handler as a local one.
pub const PLAIN_PORT: u16 = 8880;

/// Where PHP-FPM listens. Homebrew's php formula uses this by default, which
/// is what lets a PHP project be served from a path with no port of its own.
pub const PHP_FPM_DEFAULT: &str = "127.0.0.1:9000";

/// How much of the machine the app is allowed to touch.
///
/// The difference is forced by one fact: binding ports 80 and 443 requires
/// root on macOS. Everything else — DNS, certificate trust, keeping a server
/// alive across reboots — has a perfectly good unprivileged equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Nothing outside the user's own home directory is touched, and no
    /// password is ever requested. Sites live on a high port.
    Standalone,
    /// One admin prompt buys clean `https://site.test` URLs on port 443.
    System,
}

impl Default for Mode {
    fn default() -> Self {
        // System is what people actually want from this tool: port-free
        // https://site.io URLs and any domain. Standalone is the fallback for
        // a locked-down machine, offered explicitly during setup.
        Mode::System
    }
}

impl Mode {
    pub fn is_standalone(self) -> bool {
        matches!(self, Mode::Standalone)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Site {
    pub id: String,
    pub domain: String,
    pub target: Target,
    /// Serve over HTTPS with Caddy's internal CA.
    #[serde(default = "default_true")]
    pub ssl: bool,
    /// Fall back to index.html for unknown paths (folder targets only).
    #[serde(default)]
    pub spa: bool,
    /// Whether this site should have a public cloudflared tunnel running.
    #[serde(default)]
    pub tunnel: bool,
    /// Whether Portico supervises this site's dev server. Only ever true
    /// for Node targets — everything else needs no process of its own.
    #[serde(default)]
    pub run: bool,
}

fn default_true() -> bool {
    true
}

impl Site {
    pub fn url(&self, https_port: u16, http_port: u16) -> String {
        let (scheme, port, default) = if self.ssl {
            ("https", https_port, 443)
        } else {
            ("http", http_port, 80)
        };
        if port == default {
            format!("{scheme}://{}", self.domain)
        } else {
            format!("{scheme}://{}:{port}", self.domain)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub mode: Mode,
    /// Ports used in standalone mode, where 80/443 are out of reach.
    #[serde(default = "default_https_port")]
    pub standalone_https_port: u16,
    #[serde(default = "default_http_port")]
    pub standalone_http_port: u16,
    /// "system", "light" or "dark".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// "auto" to follow macOS, or a language code the interface ships.
    #[serde(default = "default_language")]
    pub language: String,
    /// Keep managed tools up to date automatically at launch.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Which service public URLs go through.
    #[serde(default)]
    pub tunnel_provider: crate::ngrok::Provider,
    /// Optional fixed hostname for ngrok. Needs a paid plan.
    #[serde(default)]
    pub ngrok_domain: String,
    /// Last observed ngrok plan, learned from an assigned hostname.
    #[serde(default)]
    pub ngrok_plan: Option<String>,
    #[serde(default)]
    pub sites: Vec<Site>,
    /// TLDs resolved wholesale by dnsmasq. Anything under one of these needs
    /// no /etc/hosts entry, so adding a site never asks for a password.
    #[serde(default = "default_tlds")]
    pub managed_tlds: Vec<String>,
}

/// Resolved to loopback by macOS itself; nothing for us to configure.
pub const NATIVE_TLDS: &[&str] = &["localhost"];

fn is_native(domain: &str) -> bool {
    NATIVE_TLDS
        .iter()
        .any(|t| domain == *t || domain.ends_with(&format!(".{t}")))
}

fn default_tlds() -> Vec<String> {
    vec!["test".to_string(), "localhost".to_string()]
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_language() -> String {
    "auto".to_string()
}

fn default_https_port() -> u16 {
    8443
}

fn default_http_port() -> u16 {
    8080
}

impl Default for Config {
    fn default() -> Self {
        Config {
            tunnel_provider: crate::ngrok::Provider::default(),
            ngrok_domain: String::new(),
            ngrok_plan: None,
            theme: default_theme(),
            language: default_language(),
            auto_update: true,
            mode: Mode::default(),
            standalone_https_port: default_https_port(),
            standalone_http_port: default_http_port(),
            sites: Vec::new(),
            managed_tlds: default_tlds(),
        }
    }
}

impl Config {
    pub fn load() -> Config {
        let path = paths::config_file();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        paths::ensure_dirs()?;
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(paths::config_file(), json)?;
        Ok(())
    }

    /// The port sites are actually served on, given the current mode.
    pub fn https_port(&self) -> u16 {
        match self.mode {
            Mode::System => 443,
            Mode::Standalone => self.standalone_https_port,
        }
    }

    pub fn http_port(&self) -> u16 {
        match self.mode {
            Mode::System => 80,
            Mode::Standalone => self.standalone_http_port,
        }
    }

    /// Does this domain need a line in /etc/hosts to resolve?
    ///
    /// Only two things avoid it: a TLD macOS resolves natively, or a wildcard
    /// TLD served by dnsmasq. dnsmasq is optional — without it every domain
    /// simply gets a hosts entry, which the helper writes without a password.
    pub fn needs_hosts_entry(&self, domain: &str) -> bool {
        if is_native(domain) {
            return false;
        }
        if self.mode.is_standalone() {
            // Standalone installs no dnsmasq configuration of its own.
            return true;
        }
        if crate::paths::dnsmasq_bin().is_none() {
            return true;
        }
        !self.is_wildcard_resolved(domain)
    }

    pub fn find(&self, id: &str) -> Option<&Site> {
        self.sites.iter().find(|s| s.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Site> {
        self.sites.iter_mut().find(|s| s.id == id)
    }

    /// True when dnsmasq already resolves this domain, meaning no hosts entry
    /// (and therefore no admin prompt) is needed.
    pub fn is_wildcard_resolved(&self, domain: &str) -> bool {
        self.managed_tlds.iter().any(|tld| {
            domain == tld || domain.ends_with(&format!(".{tld}"))
        })
    }

    /// TLDs we must actually configure dnsmasq and /etc/resolver for.
    /// macOS already resolves *.localhost to loopback per RFC 6761, so wiring
    /// it up ourselves would be redundant and would fail the doctor forever.
    pub fn tlds_needing_dns(&self) -> Vec<String> {
        if crate::paths::dnsmasq_bin().is_none() {
            // Nothing to configure without dnsmasq; hosts entries cover it.
            return Vec::new();
        }
        self.managed_tlds
            .iter()
            .filter(|t| !NATIVE_TLDS.contains(&t.as_str()))
            .cloned()
            .collect()
    }

    /// Domains that must be pinned in /etc/hosts because nothing resolves them
    /// otherwise.
    ///
    /// Standalone mode still writes no daemon, no dnsmasq and no resolver
    /// files; a custom domain costs exactly one line here and nothing else.
    pub fn hosts_entries(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .sites
            .iter()
            .map(|s| s.domain.clone())
            .filter(|d| self.needs_hosts_entry(d))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Normalise and validate what the user typed into the domain box.
pub fn normalise_domain(raw: &str) -> Result<String, String> {
    let d = raw
        .trim()
        .to_lowercase()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string();

    if d.is_empty() {
        return Err("Domain is empty".into());
    }
    if d.contains(':') {
        return Err("Leave the port out of the domain — it goes in the Target field".into());
    }
    if d.contains('/') {
        return Err("Domain must not contain a path".into());
    }
    if !d.contains('.') {
        return Err("Use a dotted name such as mysite.test".into());
    }
    for label in d.split('.') {
        if label.is_empty() {
            return Err("Domain has an empty label".into());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!("Invalid label \"{label}\" in domain"));
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("Invalid character in \"{label}\""));
        }
    }
    Ok(d)
}

pub fn slug(domain: &str) -> String {
    domain.replace('.', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scheme_and_lowercases() {
        assert_eq!(normalise_domain(" HTTPS://MySite.Test/ ").unwrap(), "mysite.test");
    }

    #[test]
    fn rejects_port_and_path() {
        assert!(normalise_domain("mysite.test:8000").is_err());
        assert!(normalise_domain("mysite.test/app").is_err());
        assert!(normalise_domain("mysite").is_err());
    }

    #[test]
    fn standalone_maps_custom_domains_but_nothing_else() {
        let mut c = Config::default();
        c.mode = Mode::Standalone;
        c.sites.push(Site {
            id: "mywebsite-io".into(),
            domain: "mywebsite.io".into(),
            target: crate::target::Target::Proxy { upstream: "127.0.0.1:1".into() },
            ssl: true,
            spa: false,
            tunnel: false,
            run: false,
        });
        assert_eq!(c.hosts_entries(), vec!["mywebsite.io".to_string()]);
        assert!(c.needs_hosts_entry("mywebsite.io"));
        assert!(!c.needs_hosts_entry("app.localhost"));
        // .test needs dnsmasq, which standalone mode does not install.
        assert!(c.needs_hosts_entry("app.test"));
    }

    #[test]
    fn ports_follow_the_mode() {
        let mut c = Config::default();
        // System is the default, and serves on the real ports.
        assert_eq!(c.mode, Mode::System);
        assert_eq!(c.https_port(), 443);
        assert_eq!(c.http_port(), 80);

        c.mode = Mode::Standalone;
        assert_eq!(c.https_port(), 8443);
        assert_eq!(c.http_port(), 8080);
    }

    #[test]
    fn url_hides_default_ports_only() {
        let s = Site {
            id: "a".into(), domain: "app.localhost".into(),
            target: crate::target::Target::Proxy { upstream: "127.0.0.1:1".into() },
            ssl: true, spa: false, tunnel: false, run: false,
        };
        assert_eq!(s.url(443, 80), "https://app.localhost");
        assert_eq!(s.url(8443, 8080), "https://app.localhost:8443");
    }

    #[test]
    fn native_tlds_are_excluded_from_dns_setup() {
        let c = Config::default();
        assert!(c.is_wildcard_resolved("app.localhost"));
        assert_eq!(c.tlds_needing_dns(), vec!["test".to_string()]);
    }

    #[test]
    fn wildcard_covers_managed_tlds_only() {
        let c = Config::default();
        assert!(c.is_wildcard_resolved("mysite.test"));
        assert!(c.is_wildcard_resolved("api.mysite.test"));
        assert!(!c.is_wildcard_resolved("mywebsite.io"));
    }
}
