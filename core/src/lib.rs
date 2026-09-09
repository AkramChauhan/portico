//! Portico core.
//!
//! One local vhost manager: pretty domains with trusted HTTPS for day-to-day
//! development, plus an optional public tunnel per site so external services
//! can actually deliver webhooks.
//!
//! The split matters and is the reason this crate exists:
//!   * `https://mysite.test` is resolved from this Mac only (dnsmasq/hosts) and
//!     signed by a local CA. Perfect for OAuth redirect URLs, useless to Stripe.
//!   * A cloudflared tunnel gives a real, publicly resolvable hostname with a
//!     publicly trusted certificate. That is what a webhook sender needs.

pub mod bins;
pub mod caddy;
pub mod cloudflare;
pub mod config;
pub mod hosts;
pub mod lan;
pub mod monitor;
pub mod ngrok;
pub mod paths;
pub mod runner;
pub mod privileged;
pub mod secrets;
pub mod setup;
pub mod target;
pub mod update;
pub mod tunnel;

use config::{normalise_domain, slug, Config, Site};
use serde::Serialize;
use target::Target;

/// A site plus everything the UI needs to render it.
#[derive(Debug, Clone, Serialize)]
pub struct SiteView {
    #[serde(flatten)]
    pub site: Site,
    pub local_url: String,
    pub target_label: String,
    /// False when the domain needs an /etc/hosts entry (custom TLD).
    pub wildcard_dns: bool,
    pub tunnel_state: tunnel::TunnelInfo,
    /// Address other devices on this network can open, when exposed.
    pub lan_url: Option<String>,
    pub run_state: runner::RunState,
    /// True when this site has a dev server we can start and stop.
    pub managed: bool,
}

fn view(cfg: &Config, site: &Site) -> SiteView {
    SiteView {
        local_url: site.url(cfg.https_port(), cfg.http_port()),
        target_label: site.target.describe(),
        wildcard_dns: !cfg.needs_hosts_entry(&site.domain),
        tunnel_state: tunnel::status(&site.id),
        lan_url: lan::url(site),
        run_state: runner::status(&site.id),
        managed: matches!(site.target, Target::Node { .. }),
        site: site.clone(),
    }
}

/// Result of probing whether a target is actually serving anything.
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub status: Option<u32>,
    pub elapsed_ms: u128,
    pub message: String,
}

/// Check a target over plain HTTP before committing to it.
///
/// This is deliberately a check of the *upstream*, not of the vhost: it answers
/// "is anything actually listening where you said?", which is the mistake worth
/// catching before a site is created.
pub fn test_target(raw: &str) -> TestResult {
    let started = std::time::Instant::now();

    let target = match Target::parse(raw) {
        Ok(t) => t,
        Err(e) => {
            return TestResult {
                ok: false,
                status: None,
                elapsed_ms: started.elapsed().as_millis(),
                message: e,
            }
        }
    };

    match &target {
        Target::Node { dir, command, port } => {
            // Nothing is listening yet by definition — the point of this target
            // is that we start it. Report what will run.
            TestResult {
                ok: true,
                status: None,
                elapsed_ms: started.elapsed().as_millis(),
                message: format!(
                    "JavaScript project detected — will run `{command}` in {dir} on port {port}. No port needed."
                ),
            }
        }
        Target::Php { root } => {
            // A PHP site needs no port of its own, but it does need PHP-FPM to
            // be up — so that is what we actually check.
            use std::net::{SocketAddr, TcpStream};
            let addr: SocketAddr = config::PHP_FPM_DEFAULT.parse().unwrap();
            let fpm = TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(700)).is_ok();
            let elapsed_ms = started.elapsed().as_millis();
            if fpm {
                TestResult {
                    ok: true,
                    status: None,
                    elapsed_ms,
                    message: format!(
                        "PHP project detected — serving {root} through PHP-FPM ({}). No port needed.",
                        config::PHP_FPM_DEFAULT
                    ),
                }
            } else {
                TestResult {
                    ok: false,
                    status: None,
                    elapsed_ms,
                    message: format!(
                        "PHP project detected, but PHP-FPM is not listening on {}. Start it with: brew services start php",
                        config::PHP_FPM_DEFAULT
                    ),
                }
            }
        }
        Target::Dir { path } => {
            let has_index = std::path::Path::new(path).join("index.html").exists();
            TestResult {
                ok: true,
                status: None,
                elapsed_ms: started.elapsed().as_millis(),
                message: if has_index {
                    format!("Folder found, index.html present")
                } else {
                    format!("Folder found (no index.html — directory listing will 404)")
                },
            }
        }
        Target::Proxy { upstream } => {
            let url = format!("http://{upstream}/");
            let out = std::process::Command::new("/usr/bin/curl")
                .args([
                    "-s", "-o", "/dev/null", "-w", "%{http_code}",
                    "--max-time", "8", &url,
                ])
                .output();
            let elapsed_ms = started.elapsed().as_millis();

            match out {
                Ok(o) => {
                    let code: u32 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0);
                    if code == 0 {
                        TestResult {
                            ok: false,
                            status: None,
                            elapsed_ms,
                            message: format!("Nothing responded on {upstream} — is the app running?"),
                        }
                    } else {
                        // Any HTTP reply proves something is listening. A 401 or
                        // 302 from a real app is a pass, not a failure.
                        TestResult {
                            ok: true,
                            status: Some(code),
                            elapsed_ms,
                            message: format!("{upstream} responded with HTTP {code}"),
                        }
                    }
                }
                Err(e) => TestResult {
                    ok: false,
                    status: None,
                    elapsed_ms,
                    message: format!("Could not run the test: {e}"),
                },
            }
        }
    }
}

pub fn list_sites() -> Vec<SiteView> {
    let cfg = Config::load();
    cfg.sites.iter().map(|s| view(&cfg, s)).collect()
}

pub fn add_site(
    domain: &str,
    target: &str,
    ssl: bool,
    spa: bool,
) -> anyhow::Result<SiteView> {
    let domain = normalise_domain(domain).map_err(|e| anyhow::anyhow!(e))?;
    let target = Target::parse(target).map_err(|e| anyhow::anyhow!(e))?;

    let mut cfg = Config::load();
    if cfg.sites.iter().any(|s| s.domain == domain) {
        anyhow::bail!("{domain} is already set up");
    }


    // A Node target exists precisely so we run it; anything else is already
    // being served by something and must not be touched.
    let run = matches!(target, Target::Node { .. });

    let site = Site {
        id: slug(&domain),
        domain: domain.clone(),
        target,
        ssl,
        spa,
        tunnel: false,
        public_domain: String::new(),
        tunnel_id: None,
        lan: false,
        lan_port: None,
        run,
    };
    cfg.sites.push(site);
    // Same ordering as removal: nothing is persisted until the system change
    // it implies has actually gone through.
    apply(&cfg)?;
    cfg.save()?;
    warm_certificate(&domain, cfg.https_port());

    if run {
        let site = cfg.find(&slug(&domain)).unwrap().clone();
        start_process(&site)?;
    }

    let site = cfg.find(&slug(&domain)).unwrap().clone();
    Ok(view(&cfg, &site))
}

/// Caddy mints a certificate on the first request to a hostname, so that first
/// request can fail or stall. Trigger issuance ourselves the moment a site is
/// added, in the background, so the user's first click is already warm.
fn warm_certificate(domain: &str, port: u16) {
    let url = format!("https://{domain}:{port}/");
    std::thread::spawn(move || {
        let _ = std::process::Command::new("/usr/bin/curl")
            .args(["-sk", "-o", "/dev/null", "--max-time", "10", &url])
            .output();
    });
}

/// Launch the dev server behind a Node site.
fn start_process(site: &Site) -> anyhow::Result<()> {
    let Target::Node { dir, command, port } = &site.target else {
        return Ok(());
    };
    runner::start(&site.id, dir, command, *port, &paths::dev_log(&site.id))
}

/// Start or stop a supervised dev server.
pub fn set_run(id: &str, on: bool) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    let site = cfg
        .find_mut(id)
        .ok_or_else(|| anyhow::anyhow!("No such site: {id}"))?;
    if !matches!(site.target, Target::Node { .. }) {
        anyhow::bail!("{} is not a site Portico runs", site.domain);
    }
    site.run = on;
    let site = site.clone();

    if on {
        start_process(&site)?;
    } else {
        runner::stop(id);
    }
    cfg.save()
}

/// Requests this site has handled, most recent first.
///
/// Takes a site id, not a domain: the shared access log records the vhost, so
/// the id has to be resolved before filtering.
pub fn site_requests(id: &str, limit: usize) -> Vec<monitor::RequestEntry> {
    let cfg = Config::load();
    match cfg.find(id) {
        Some(site) => monitor::recent_requests(&site.domain, limit),
        None => Vec::new(),
    }
}

/// Probe a site right now.
pub fn site_health(id: &str) -> anyhow::Result<monitor::Health> {
    let cfg = Config::load();
    let site = cfg.find(id).ok_or_else(|| anyhow::anyhow!("No such site: {id}"))?;
    Ok(monitor::health(&cfg, site))
}

/// Every log Portico has produced.
pub fn logs() -> Vec<monitor::LogFile> {
    monitor::list_logs()
}

pub fn read_log(name: &str, lines: usize) -> String {
    monitor::read_log(name, lines)
}

/// Empty one log, or all of them. Returns the bytes reclaimed.
///
/// Caddy is reloaded afterwards so it reopens any log it had open, which also
/// re-creates files that had to be unlinked because root owned them.
pub fn clear_logs(name: Option<&str>) -> u64 {
    let freed = monitor::clear_logs(name);
    if caddy::is_running() {
        let _ = caddy::reload();
    }
    freed
}

/// cloudflared's output for a site's public tunnel.
pub fn site_tunnel_log(id: &str, lines: usize) -> String {
    if !paths::safe_id(id) {
        return String::new();
    }
    monitor::tail_file(&paths::tunnel_log(id), lines)
}

/// Output of the dev server behind a supervised site.
pub fn site_dev_log(id: &str, lines: usize) -> String {
    monitor::dev_log(id, lines)
}

pub fn remove_site(id: &str) -> anyhow::Result<()> {
    tunnel::stop(id);
    runner::stop(id);
    let mut cfg = Config::load();

    // Hand back the hostname and the tunnel behind it. Doing this before the
    // site is gone is the only chance we get: once it is out of the config,
    // nothing knows the tunnel existed.
    if let Some(site) = cfg.find(id) {
        if site.tunnel_id.is_some() {
            tunnel::deprovision(site);
        }
    }

    let before = cfg.sites.len();
    cfg.sites.retain(|s| s.id != id);
    if cfg.sites.len() == before {
        anyhow::bail!("No such site: {id}");
    }
    // Apply *before* persisting. Removing a custom domain rewrites /etc/hosts,
    // which can be refused — and if the config had already been saved, the
    // hosts entry would be orphaned with nothing left that knows to remove it.
    apply(&cfg)?;
    cfg.save()
}

pub fn set_ssl(id: &str, on: bool) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    cfg.find_mut(id).ok_or_else(|| anyhow::anyhow!("No such site: {id}"))?.ssl = on;
    cfg.save()?;
    apply(&cfg)
}

pub fn set_spa(id: &str, on: bool) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    cfg.find_mut(id).ok_or_else(|| anyhow::anyhow!("No such site: {id}"))?.spa = on;
    cfg.save()?;
    apply(&cfg)
}

/// Serve this site to other devices on the same network, or stop doing so.
///
/// The port is assigned once and kept, so turning this off and on again hands
/// back the same address rather than breaking a link already open elsewhere.
pub fn set_lan(id: &str, on: bool) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    if cfg.find(id).is_none() {
        anyhow::bail!("No such site: {id}");
    }

    let port = if on {
        Some(lan::assign_port(&cfg, id).ok_or_else(|| {
            anyhow::anyhow!("No free port left for local network sharing")
        })?)
    } else {
        cfg.find(id).and_then(|s| s.lan_port)
    };

    let site = cfg.find_mut(id).expect("site was present a moment ago");
    site.lan = on;
    site.lan_port = port;

    // Same ordering as everywhere else: the listener has to exist before the
    // config claims it does, or the app shows an address nothing answers on.
    let updated = cfg.clone();
    apply(&updated)?;
    cfg.save()
}

pub fn set_tunnel(id: &str, on: bool) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    let site = cfg
        .find(id)
        .ok_or_else(|| anyhow::anyhow!("No such site: {id}"))?
        .clone();

    // Start first, persist second. Saving `tunnel: true` before the process is
    // actually up left the toggle switched on with nothing running and no
    // error to show — which reads as the app having lost the site.
    if on {
        // Claiming the hostname and pointing DNS at it comes before the agent
        // runs, and before anything is written down: a tunnel id saved for a
        // tunnel that was never created is a tunnel nothing can clean up.
        let tunnel_id = tunnel::provision(&site)?;
        let mut site = site;
        if let Some(tid) = tunnel_id.clone() {
            site.tunnel_id = Some(tid);
        }
        tunnel::start(&site)?;

        let stored = cfg.find_mut(id).expect("site was present a moment ago");
        stored.tunnel = true;
        if let Some(tid) = tunnel_id {
            stored.tunnel_id = Some(tid);
        }
    } else {
        tunnel::stop(id);
        cfg.find_mut(id).expect("site was present a moment ago").tunnel = false;
    }
    cfg.save()
}

/// Give a site a stable public hostname, or take it away.
///
/// Changing the name means the old one must be given back: leaving a DNS
/// record pointing at a tunnel that no longer serves it is a hostname that
/// resolves to an error for as long as the user owns the domain.
pub fn set_public_domain(id: &str, domain: &str) -> anyhow::Result<()> {
    let wanted = domain.trim();
    let normalised = if wanted.is_empty() {
        String::new()
    } else {
        normalise_domain(wanted).map_err(|e| anyhow::anyhow!(e))?
    };

    let mut cfg = Config::load();
    let site = cfg
        .find(id)
        .ok_or_else(|| anyhow::anyhow!("No such site: {id}"))?
        .clone();

    if site.public_domain == normalised {
        return Ok(());
    }
    if !normalised.is_empty() {
        if let Some(other) = cfg
            .sites
            .iter()
            .find(|s| s.id != site.id && s.public_domain == normalised)
        {
            anyhow::bail!("{normalised} is already used by {}", other.domain);
        }
    }

    // A running tunnel is serving the old name; stop it before the name moves.
    tunnel::stop(id);
    if site.tunnel_id.is_some() {
        tunnel::deprovision(&site);
    }

    let stored = cfg.find_mut(id).expect("site was present a moment ago");
    stored.public_domain = normalised;
    stored.tunnel_id = None;
    stored.tunnel = false;
    cfg.save()
}

/// Which zones a stored Cloudflare token can write to.
pub fn cloudflare_status() -> CloudflareStatus {
    let Some(token) = secrets::get(secrets::CLOUDFLARE_TOKEN) else {
        return CloudflareStatus {
            connected: false,
            account: None,
            zones: Vec::new(),
            detail: "Not connected — add an API token to use your own hostnames".into(),
        };
    };

    match cloudflare::verify(&token) {
        Ok(account) => {
            // Cache the account id so starting a tunnel is one call shorter.
            let mut cfg = Config::load();
            if cfg.cloudflare_account.as_deref() != Some(account.id.as_str()) {
                cfg.cloudflare_account = Some(account.id.clone());
                let _ = cfg.save();
            }
            let detail = if account.zones.is_empty() {
                "Connected, but this token can see no domains — it needs Zone:DNS:Edit".to_string()
            } else {
                format!("Connected to {} · {} domain(s)", account.name, account.zones.len())
            };
            CloudflareStatus {
                connected: true,
                account: Some(account.name),
                zones: account.zones,
                detail,
            }
        }
        Err(e) => CloudflareStatus {
            connected: false,
            account: None,
            zones: Vec::new(),
            detail: e.to_string(),
        },
    }
}

/// Store a Cloudflare API token, but only one that actually works.
///
/// Verified before it is saved: a token that cannot see an account is not
/// worth keeping, and finding that out now beats finding out when a site is
/// turned public.
pub fn connect_cloudflare(token: &str) -> anyhow::Result<CloudflareStatus> {
    let token = token.trim();
    let account = cloudflare::verify(token)?;
    secrets::set(secrets::CLOUDFLARE_TOKEN, token)?;

    let mut cfg = Config::load();
    cfg.cloudflare_account = Some(account.id.clone());
    cfg.save()?;

    Ok(cloudflare_status())
}

/// Forget the stored token. Tunnels already created are left alone — they are
/// on the user's account, and deleting them here would be a surprise.
pub fn disconnect_cloudflare() -> anyhow::Result<()> {
    secrets::delete(secrets::CLOUDFLARE_TOKEN);
    let mut cfg = Config::load();
    cfg.cloudflare_account = None;
    cfg.save()
}

/// Push the current config into Caddy, and refresh /etc/hosts if any site sits
/// on a TLD that dnsmasq does not cover.
///
/// A stopped daemon is not an error here: the Caddyfile is still written, the
/// diagnostics panel already reports the daemon as down, and setup picks the
/// file up when it starts. Only a genuinely bad config should fail an edit.
fn apply(cfg: &Config) -> anyhow::Result<()> {
    paths::ensure_dirs()?;
    caddy::write_config(cfg)?;

    let serving = caddy::is_running();
    if serving {
        caddy::reload()?;
    }

    // Only prompt when the block is stale *and* we are actually serving. If
    // Caddy is not up yet, setup will write the block itself, so asking for a
    // password now would just be a second prompt for the same change.
    // Keep the helper's request file current either way, so a later setup or
    // a manual kick has the right list to work from.
    let entries = cfg.hosts_entries();
    let _ = hosts::write_request(&entries);

    if serving && hosts::needs_sync(&entries) {
        if hosts::watcher_installed() {
            // launchd is watching the request file we just wrote; the helper
            // does the rest, with no password.
            if !hosts::wait_for_sync(&entries, 12000) {
                anyhow::bail!(
                    "The /etc/hosts helper did not apply the change — see ~/.portico/logs/hostsd.log"
                );
            }
        } else {
            setup::sync_hosts(cfg)?;
        }
    }
    Ok(())
}

/// What a stored Cloudflare token can reach.
#[derive(Debug, Clone, Serialize)]
pub struct CloudflareStatus {
    pub connected: bool,
    pub account: Option<String>,
    /// Domains a hostname may be created under.
    pub zones: Vec<cloudflare::Zone>,
    pub detail: String,
}

/// User-facing preferences.
#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    pub tunnel_provider: ngrok::Provider,
    pub ngrok_domain: String,
    pub theme: String,
    pub language: String,
    pub auto_update: bool,
    pub mode: config::Mode,
    pub app_version: String,
}

pub fn settings() -> Settings {
    let cfg = Config::load();
    Settings {
        tunnel_provider: cfg.tunnel_provider,
        ngrok_domain: cfg.ngrok_domain.clone(),
        theme: cfg.theme.clone(),
        language: cfg.language.clone(),
        auto_update: cfg.auto_update,
        mode: cfg.mode,
        app_version: update::APP_VERSION.to_string(),
    }
}

pub fn set_theme(theme: &str) -> anyhow::Result<()> {
    if !matches!(theme, "system" | "light" | "dark") {
        anyhow::bail!("Unknown theme: {theme}");
    }
    let mut cfg = Config::load();
    cfg.theme = theme.to_string();
    cfg.save()
}

/// Interface language. Validation lives in the frontend, which owns the
/// catalogue; an unknown code simply falls back to English there.
pub fn set_language(language: &str) -> anyhow::Result<()> {
    if language.len() > 16 || !language.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        anyhow::bail!("Invalid language code");
    }
    let mut cfg = Config::load();
    cfg.language = language.to_string();
    cfg.save()
}

pub fn set_auto_update(on: bool) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    cfg.auto_update = on;
    cfg.save()
}

/// Version report for the app and the tools it manages.
pub fn check_updates() -> update::UpdateReport {
    update::check(None)
}

pub fn mode() -> config::Mode {
    Config::load().mode
}

pub fn doctor() -> Vec<setup::Check> {
    setup::doctor(&Config::load())
}

/// What one install run did.
#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub tools: Vec<bins::ToolStatus>,
    pub message: String,
}

/// Install Portico's prerequisites, in two phases.
///
/// 1. **Tools** — fetch anything missing. Nothing here needs a password, and
///    anything the user already installed is left alone.
/// 2. **Access** — one authorisation that covers every privileged change,
///    including the /etc/hosts helper. After this the app never asks again.
///
/// Tools come first because the privileged step bakes their paths into the
/// launchd job it installs.
/// Phase one on its own: fetch any missing tool. No privileges involved.
pub fn install_tools_reporting(report: bins::Reporter) -> anyhow::Result<Vec<bins::ToolStatus>> {
    bins::provision_reporting(report)
}

/// Phase two on its own: the one authorisation. Assumes the tools are present.
pub fn install_access() -> anyhow::Result<String> {
    setup::install(&Config::load())
}

pub fn run_setup() -> anyhow::Result<InstallReport> {
    run_setup_reporting(&bins::no_progress, &|_, _| {})
}

/// As `run_setup`, but reporting each step so a UI can show progress rather
/// than an unexplained pause on a 44 MB download.
pub fn run_setup_reporting(
    report: bins::Reporter,
    phase: &(dyn Fn(&str, &str) + Send + Sync),
) -> anyhow::Result<InstallReport> {
    let tools = bins::provision_reporting(report)?;
    phase("access", "Waiting for your password…");
    let message = setup::install(&Config::load())?;
    phase("done", "Ready.");
    Ok(InstallReport { tools, message })
}

/// Re-install the local CA into the user's trust store. Useful on its own when
/// the certificate was rotated or the trust prompt was dismissed.
/// Make sure the tunnelling tool is available, fetching it if needed.
/// Which tools are present, for the onboarding screen.
/// Local ngrok status, including the plan if we have observed a hostname.
pub fn ngrok_status() -> ngrok::NgrokStatus {
    ngrok::status(Config::load().ngrok_plan.as_deref())
}

pub fn set_tunnel_provider(provider: ngrok::Provider) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    cfg.tunnel_provider = provider;
    cfg.save()
}

/// Start a short throwaway ngrok tunnel to learn which plan the account is on.
///
/// The plan is not exposed to the agent, but the hostname it assigns gives it
/// away: the free tier is always on ngrok-free.app.
///
/// Doubles as the only real check that an authtoken works. ngrok has no
/// "verify this token" endpoint we can call, so actually connecting is the
/// test — which is why saving a token runs this too.
fn probe_ngrok_plan() -> anyhow::Result<String> {
    use std::io::{BufRead, BufReader};

    let bin = ngrok::bin().ok_or_else(|| anyhow::anyhow!("ngrok is not installed"))?;
    let token = ngrok::authtoken();

    // Our own config file, so the probe tests the token we were given rather
    // than one already sitting in ngrok's default config. Without this a bad
    // token looked valid on any machine where a good one was already set up.
    let mut args = ngrok::config_args();
    args.extend(ngrok::tunnel_args(config::PLAIN_PORT, "portico.probe", None));

    let mut child = std::process::Command::new(bin)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut found = None;
    let mut failure = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok).take(60) {
            if let Some(url) = ngrok::extract_url(&line) {
                found = Some(url);
                break;
            }
            // ngrok quotes a rejected token back at us; never keep it.
            if line.contains("ERR_NGROK") {
                failure = Some(ngrok::redact(&line, token.as_deref()));
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    let url = found.ok_or_else(|| match failure {
        Some(f) => anyhow::anyhow!("{f}"),
        None => anyhow::anyhow!("ngrok did not publish a URL — check that its authtoken is valid"),
    })?;
    let plan = ngrok::plan_from_url(&url).to_string();

    let mut cfg = Config::load();
    cfg.ngrok_plan = Some(plan.clone());
    cfg.save()?;
    Ok(plan)
}

/// Store an ngrok authtoken, but only one that actually connects.
///
/// The token goes in first because the probe reads it from the keychain, and
/// is taken back out if the probe fails — so a rejected token is never left
/// behind looking configured.
pub fn connect_ngrok(token: &str) -> anyhow::Result<ngrok::NgrokStatus> {
    let token = token.trim();
    if ngrok::bin().is_none() {
        anyhow::bail!("ngrok is not installed — install it first, then add the token");
    }
    secrets::set(secrets::NGROK_AUTHTOKEN, token)?;

    match probe_ngrok_plan() {
        Ok(_) => Ok(ngrok_status()),
        Err(e) => {
            secrets::delete(secrets::NGROK_AUTHTOKEN);
            ngrok::forget_config();
            Err(e)
        }
    }
}

/// Forget the stored ngrok token.
///
/// A token the user added with `ngrok config add-authtoken` lives in ngrok's
/// own file and is untouched — removing that is theirs to do.
pub fn disconnect_ngrok() -> anyhow::Result<()> {
    secrets::delete(secrets::NGROK_AUTHTOKEN);
    ngrok::forget_config();
    let mut cfg = Config::load();
    cfg.ngrok_plan = None;
    cfg.save()
}

pub fn tools_status() -> Vec<bins::ToolStatus> {
    bins::status()
}

pub fn ensure_tunneling() -> anyhow::Result<String> {
    tunnel::ensure_cloudflared()
}

pub fn tunneling_installed() -> bool {
    tunnel::is_installed()
}

pub fn trust_ca() -> anyhow::Result<String> {
    setup::trust_ca_for_user()?;
    Ok("Local certificate authority trusted".into())
}

pub fn run_uninstall() -> anyhow::Result<String> {
    tunnel::stop_all();
    setup::uninstall(&Config::load())
}

/// Record a mode choice without installing anything.
///
/// Used during setup, where nothing is installed yet so there is nothing to
/// tear down — unlike `set_mode`, which switches a live installation.
pub fn set_mode_preference(mode: config::Mode) -> anyhow::Result<()> {
    let mut cfg = Config::load();
    cfg.mode = mode;
    cfg.save()
}

/// Switch between standalone and system mode.
///
/// The old mode's plumbing is torn down first, so the two never fight over a
/// port or leave an orphaned agent behind.
pub fn set_mode(mode: config::Mode) -> anyhow::Result<String> {
    let mut cfg = Config::load();
    if cfg.mode == mode {
        return Ok("Already in that mode".into());
    }

    tunnel::stop_all();
    let previous = setup::uninstall(&cfg);

    cfg.mode = mode;
    cfg.save()?;

    let _ = bins::provision()?;
    match setup::install(&cfg) {
        Ok(msg) => Ok(msg),
        Err(e) => {
            let _ = previous;
            Err(e)
        }
    }
}

/// Forget any public tunnel left flagged by a previous session.
///
/// Deliberately does *not* restore them. A quick tunnel gets a new random
/// hostname every time, so restoring one cannot revive the URL you configured
/// somewhere — it would only re-expose the site under a URL you have not seen.
/// Combined with a flag that survives an unclean exit, that means simply
/// opening the app could publish a private site to the internet. Exposure
/// stays an explicit, in-session action.
///
/// A stable named tunnel would be a different case, and could be restored.
pub fn clear_stale_tunnels() {
    let mut cfg = Config::load();
    if clear_tunnel_flags(&mut cfg) {
        let _ = cfg.save();
    }
}

/// Clear every public-tunnel flag. Returns whether anything changed.
fn clear_tunnel_flags(cfg: &mut Config) -> bool {
    let had_any = cfg.sites.iter().any(|s| s.tunnel);
    for s in cfg.sites.iter_mut() {
        s.tunnel = false;
    }
    had_any
}

/// Restart the dev servers that were running when the app last closed.
pub fn restore_processes() {
    for s in Config::load().sites.iter().filter(|s| s.run) {
        // A server left running by an earlier session is adopted rather than
        // restarted — otherwise reopening the app would fight it for the port.
        if runner::adopted(&s.id).is_some() {
            continue;
        }
        let _ = start_process(s);
    }
}

/// Stop everything this process started, and leave nothing armed to expose a
/// site on next launch.
pub fn shutdown() {
    tunnel::stop_all();
    runner::stop_all();
    clear_stale_tunnels();
}


#[cfg(test)]
mod safety_tests {
    //! Publishing a site to the internet must never happen implicitly.
    use super::*;

    fn site_flagged_public(domain: &str) -> Site {
        Site {
            id: slug(domain),
            domain: domain.into(),
            target: Target::Proxy { upstream: "127.0.0.1:8000".into() },
            ssl: true,
            spa: false,
            tunnel: true,
            public_domain: String::new(),
            tunnel_id: None,
            lan: false,
            lan_port: None,
            run: false,
        }
    }

    #[test]
    fn a_flag_left_by_a_previous_session_is_cleared_not_honoured() {
        // An unclean exit (Ctrl-C, crash, kill) leaves `tunnel: true` behind.
        // Launching must never read that as "publish this site again".
        let mut cfg = Config::default();
        cfg.sites.push(site_flagged_public("private.io"));
        cfg.sites.push(site_flagged_public("control.io"));

        assert!(clear_tunnel_flags(&mut cfg), "should report a change");
        assert!(
            cfg.sites.iter().all(|s| !s.tunnel),
            "a site was left armed to expose itself"
        );
    }

    #[test]
    fn clearing_is_a_no_op_when_nothing_was_public() {
        let mut cfg = Config::default();
        let mut s = site_flagged_public("local.io");
        s.tunnel = false;
        cfg.sites.push(s);
        assert!(!clear_tunnel_flags(&mut cfg), "should not rewrite config needlessly");
    }
}
