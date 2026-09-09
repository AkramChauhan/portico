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
    /// Stable public hostname for this site, on a domain the user owns.
    ///
    /// Empty means a quick tunnel: anonymous, and a new random hostname every
    /// run. Set, it selects a *named* tunnel instead — the whole point being
    /// that the URL you registered with Stripe last week still works today.
    ///
    /// This is per site rather than global because two sites cannot share one
    /// hostname; a single setting meant the second site to go public silently
    /// fought the first for it.
    #[serde(default)]
    pub public_domain: String,
    /// Cloudflare's id for the named tunnel backing `public_domain`.
    ///
    /// Recorded so the tunnel and its DNS record can be torn down later. A
    /// tunnel we cannot name is a tunnel we cannot delete, which would leave
    /// the user to find it in the dashboard themselves.
    #[serde(default)]
    pub tunnel_id: Option<String>,
    /// Whether this site answers on every network interface, not just
    /// loopback, so other devices on the same network can reach it.
    ///
    /// Unlike a public tunnel this survives a restart: the address is stable
    /// and the user picked it. It is still exposure — the network you are on
    /// at a café is not the network you were on at home — so the interface
    /// says so plainly rather than hiding it behind a badge.
    #[serde(default)]
    pub lan: bool,
    /// The port this site holds on the LAN listener.
    ///
    /// Kept even while `lan` is off so that turning it back on returns the
    /// same address, rather than invalidating a link left open on a phone.
    #[serde(default)]
    pub lan_port: Option<u16>,
    /// Whether Portico supervises this site's dev server. Only ever true
    /// for Node targets — everything else needs no process of its own.
    #[serde(default)]
    pub run: bool,
}

fn default_true() -> bool {
    true
}

impl Site {
    /// The fixed hostname this site publishes on, if it has claimed one.
    pub fn public_hostname(&self) -> Option<&str> {
        let host = self.public_domain.trim();
        (!host.is_empty()).then_some(host)
    }

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
    /// Legacy single fixed hostname for ngrok, kept only so an existing
    /// config keeps working. `Site::public_domain` replaces it and wins
    /// wherever both are set; nothing writes this field any more.
    #[serde(default)]
    pub ngrok_domain: String,
    /// Cloudflare account id, learned when a token is verified.
    ///
    /// Not a secret — the token itself is in the keychain — but caching it
    /// saves a round trip every time a named tunnel starts.
    #[serde(default)]
    pub cloudflare_account: Option<String>,
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
            cloudflare_account: None,
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
            .and_then(|s| serde_json::from_str::<Config>(&s).ok())
            .unwrap_or_default()
            .migrate()
    }

    /// Move a pre-per-site ngrok hostname onto the site that can use it.
    ///
    /// The old setting was global, so it could only ever have described one
    /// site. Handing the same value to every site would recreate exactly the
    /// collision that moving it here was meant to fix.
    ///
    /// Persisted the next time anything saves. If there is no site to take it
    /// yet the value stays put rather than being dropped, so a hostname
    /// configured before any site existed survives.
    fn migrate(mut self) -> Config {
        let legacy = self.ngrok_domain.trim().to_string();
        if legacy.is_empty() {
            return self;
        }
        if let Some(site) = self.sites.iter_mut().find(|s| s.public_domain.trim().is_empty()) {
            site.public_domain = legacy;
            self.ngrok_domain.clear();
        }
        self
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
        self.dns_managed_tlds()
    }

    /// The TLDs dnsmasq would have to answer for, whether or not it is
    /// installed. Kept separate from `tlds_needing_dns` so the rule can be
    /// asserted on a machine that does not happen to have dnsmasq — testing
    /// it through the gate made the outcome depend on the host.
    pub(crate) fn dns_managed_tlds(&self) -> Vec<String> {
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
            public_domain: String::new(),
            tunnel_id: None,
            lan: false,
            lan_port: None,
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
            ssl: true, spa: false, tunnel: false,
            public_domain: String::new(), tunnel_id: None, lan: false, lan_port: None, run: false,
        };
        assert_eq!(s.url(443, 80), "https://app.localhost");
        assert_eq!(s.url(8443, 8080), "https://app.localhost:8443");
    }

    #[test]
    fn native_tlds_are_excluded_from_dns_setup() {
        let c = Config::default();
        assert!(c.is_wildcard_resolved("app.localhost"));
        // `.localhost` resolves natively, so only `.test` is dnsmasq's problem.
        assert_eq!(c.dns_managed_tlds(), vec!["test".to_string()]);
    }

    #[test]
    fn no_dns_is_needed_without_dnsmasq() {
        // The gate, not the rule. This is the half that legitimately depends
        // on the machine, so it asserts only what holds either way.
        let c = Config::default();
        if crate::paths::dnsmasq_bin().is_none() {
            assert!(c.tlds_needing_dns().is_empty());
        } else {
            assert_eq!(c.tlds_needing_dns(), c.dns_managed_tlds());
        }
    }

    #[test]
    fn wildcard_covers_managed_tlds_only() {
        let c = Config::default();
        assert!(c.is_wildcard_resolved("mysite.test"));
        assert!(c.is_wildcard_resolved("api.mysite.test"));
        assert!(!c.is_wildcard_resolved("mywebsite.io"));
    }

    fn site_named(id: &str) -> Site {
        Site {
            id: id.into(),
            domain: format!("{id}.test"),
            target: crate::target::Target::Proxy { upstream: "127.0.0.1:1".into() },
            ssl: true,
            spa: false,
            tunnel: false,
            public_domain: String::new(),
            tunnel_id: None,
            lan: false,
            lan_port: None,
            run: false,
        }
    }

    #[test]
    fn a_legacy_global_hostname_lands_on_exactly_one_site() {
        // The old setting was global, so it described one site at most.
        // Copying it onto every site would recreate the collision that
        // moving it per-site exists to fix.
        let mut c = Config::default();
        c.ngrok_domain = "hooks.example.com".into();
        c.sites.push(site_named("first"));
        c.sites.push(site_named("second"));

        let c = c.migrate();
        assert_eq!(c.sites[0].public_domain, "hooks.example.com");
        assert_eq!(c.sites[1].public_domain, "", "the name must not be handed out twice");
        assert_eq!(c.ngrok_domain, "", "the global must not be able to migrate again");
    }

    #[test]
    fn migration_never_overwrites_a_hostname_a_site_already_has() {
        let mut c = Config::default();
        c.ngrok_domain = "legacy.example.com".into();
        let mut first = site_named("first");
        first.public_domain = "chosen.example.com".into();
        c.sites.push(first);
        c.sites.push(site_named("second"));

        let c = c.migrate();
        assert_eq!(c.sites[0].public_domain, "chosen.example.com");
        assert_eq!(c.sites[1].public_domain, "legacy.example.com");
    }

    #[test]
    fn a_legacy_hostname_survives_until_there_is_a_site_for_it() {
        // Configured before any site existed. Dropping it here would lose a
        // paid ngrok hostname the user had already set up.
        let mut c = Config::default();
        c.ngrok_domain = "hooks.example.com".into();
        let c = c.migrate();
        assert_eq!(c.ngrok_domain, "hooks.example.com");
    }

    #[test]
    fn migration_is_idempotent() {
        let mut c = Config::default();
        c.ngrok_domain = "hooks.example.com".into();
        c.sites.push(site_named("first"));
        let c = c.migrate().migrate().migrate();
        assert_eq!(c.sites[0].public_domain, "hooks.example.com");
        assert_eq!(c.ngrok_domain, "");
    }

    #[test]
    fn a_blank_hostname_is_no_hostname() {
        let mut s = site_named("a");
        s.public_domain = "   ".into();
        assert!(s.public_hostname().is_none(), "whitespace must not select a named tunnel");
        s.public_domain = "hooks.example.com".into();
        assert_eq!(s.public_hostname(), Some("hooks.example.com"));
    }
}
