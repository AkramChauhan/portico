use crate::caddy;
use crate::config::{Config, Mode};
use crate::hosts;
use crate::paths;
use crate::privileged;
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: String,
    pub label: String,
    pub ok: bool,
    pub detail: String,
    /// True when the one-click setup can repair this.
    pub fixable: bool,
}

fn check(id: &str, label: &str, ok: bool, detail: impl Into<String>, fixable: bool) -> Check {
    Check { id: id.into(), label: label.into(), ok, detail: detail.into(), fixable }
}

fn run(bin: &str, args: &[&str]) -> (bool, String) {
    match Command::new(bin).args(args).output() {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        ),
        Err(e) => (false, e.to_string()),
    }
}

fn exists(p: &str) -> bool {
    std::path::Path::new(p).exists()
}

/// Everything that has to be true for a site to actually resolve and serve.
/// Which checks apply depends on the mode — standalone mode deliberately has
/// far fewer, because it changes far less.
pub fn doctor(cfg: &Config) -> Vec<Check> {
    let mut out = Vec::new();
    let standalone = cfg.mode.is_standalone();

    let caddy_bin = paths::caddy_bin();
    out.push(check(
        "caddy_bin",
        "Caddy installed",
        exists(&caddy_bin),
        if exists(&caddy_bin) { caddy_bin.clone() } else { "Run: brew install caddy".to_string() },
        false,
    ));

    // Not having it is no longer a problem: it is fetched on first use.
    let cf = crate::tunnel::cloudflared_bin();
    out.push(check(
        "cloudflared",
        "Tunnelling tool",
        true,
        match &cf {
            Some(p) => p.clone(),
            None => "Not installed — will be downloaded when you first go public".to_string(),
        },
        false,
    ));

    let running = caddy::is_running();
    out.push(check(
        "caddy_daemon",
        if standalone { "Caddy agent running" } else { "Caddy daemon running" },
        running,
        if running {
            format!("admin API on {}", paths::CADDY_ADMIN)
        } else {
            "Not answering — run setup".to_string()
        },
        true,
    ));

    if standalone {
        let plist = paths::agent_plist();
        out.push(check(
            "agent",
            "Login agent installed",
            plist.exists(),
            if plist.exists() {
                "~/Library/LaunchAgents — no admin rights used".to_string()
            } else {
                "Not installed yet".to_string()
            },
            true,
        ));

        let (_, trust) = run("/usr/bin/security", &["dump-trust-settings"]);
        let trusted = trust.contains("Caddy Local Authority");
        out.push(check(
            "ca_trusted",
            "Certificate trusted for this user",
            trusted,
            if trusted {
                "Trusted in your login keychain".to_string()
            } else {
                "Browsers will warn until setup runs".to_string()
            },
            true,
        ));

    } else {
        let dnsmasq_bin = format!("{}/sbin/dnsmasq", paths::brew_prefix());
        out.push(check(
            "dnsmasq_bin",
            "dnsmasq installed",
            exists(&dnsmasq_bin),
            if exists(&dnsmasq_bin) { dnsmasq_bin } else { "Run: brew install dnsmasq".to_string() },
            false,
        ));

        let tld = cfg.tlds_needing_dns().first().cloned().unwrap_or_else(|| "test".to_string());
        let probe = format!("probe-portico.{tld}");
        let (_, dig) = run(
            "/usr/bin/dig",
            &["+short", "+time=1", "+tries=1", &probe, "@127.0.0.1"],
        );
        let dns_ok = dig.contains("127.0.0.1");
        out.push(check(
            "wildcard_dns",
            format!("*.{tld} resolves to 127.0.0.1").as_str(),
            dns_ok,
            if dns_ok { format!("{probe} → 127.0.0.1") } else { "dnsmasq is not answering on 127.0.0.1:53".to_string() },
            true,
        ));

        let missing_resolvers: Vec<String> = cfg
            .tlds_needing_dns()
            .iter()
            .filter(|t| !exists(&format!("{}/{t}", paths::RESOLVER_DIR)))
            .cloned()
            .collect();
        out.push(check(
            "resolvers",
            "macOS resolver files present",
            missing_resolvers.is_empty(),
            if missing_resolvers.is_empty() {
                "/etc/resolver configured".to_string()
            } else {
                format!("Missing: {}", missing_resolvers.join(", "))
            },
            true,
        ));

        let (_, certs) = run(
            "/usr/bin/security",
            &["find-certificate", "-a", "/Library/Keychains/System.keychain"],
        );
        let trusted = certs.contains("Caddy Local Authority");
        out.push(check(
            "ca_trusted",
            "Local certificate authority trusted",
            trusted,
            if trusted { "Caddy Local Authority in System keychain".to_string() } else { "Browsers will warn until this is installed".to_string() },
            true,
        ));

    }

    // Custom domains need a hosts line in either mode.
    let need_hosts = cfg.hosts_entries();
    let stale = hosts::needs_sync(&need_hosts);
    out.push(check(
        "hosts",
        "/etc/hosts entries",
        !stale,
        if need_hosts.is_empty() {
            "No custom-domain sites — nothing needed".to_string()
        } else if stale {
            format!("Needs one line for: {}", need_hosts.join(", "))
        } else {
            format!("{} domain(s) mapped", need_hosts.len())
        },
        true,
    ));

    // The helper needs root to install, so it belongs to System mode only.
    // Reporting it as a fixable failure in Standalone made onboarding
    // unfinishable: nothing could ever clear it, so the app never handed off
    // to the main window.
    let watcher = hosts::watcher_installed();
    out.push(check(
        "hosts_helper",
        "Password-free /etc/hosts updates",
        if standalone { true } else { watcher },
        if watcher {
            "Installed — new domains map without a prompt".to_string()
        } else if standalone {
            "Not used in Standalone mode — a custom domain asks once, .localhost never does".to_string()
        } else {
            "Not installed — each new custom domain asks for a password".to_string()
        },
        !standalone,
    ));

    // Anything already holding our HTTPS port will break every site.
    let port = cfg.https_port();
    let conflict = tcp_open(port) && !running;
    out.push(check(
        "ports",
        format!("Port {port} available").as_str(),
        !conflict,
        if conflict {
            format!("Something else is listening on {port} — stop it first")
        } else {
            "Clear".to_string()
        },
        false,
    ));

    out
}

fn tcp_open(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// Shell that installs the /etc/hosts helper, or explains why it could not.
///
/// The binary is copied into a root-owned directory: left where it was built
/// it would be user-writable, and a root-launched user-writable binary is a
/// straight path to privilege escalation.
fn hostsd_install_block() -> String {
    let Some(src) = paths::hostsd_source() else {
        return "echo \"   portico-hostsd not found next to the app — skipping (you will be asked for a password per domain)\"".to_string();
    };
    format!(
        r#"mkdir -p "{dir}"
chown root:wheel "{dir}"
chmod 755 "{dir}"
cp "{src}" "{bin}"
chown root:wheel "{bin}"
chmod 755 "{bin}"
cat > {plist} <<'HOSTSD_EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>{request}</string>
  </array>
  <key>WatchPaths</key>
  <array><string>{request}</string></array>
  <key>RunAtLoad</key><true/>
  <!-- launchd refuses to restart a job more than once per ThrottleInterval,
       which defaults to 10s. Editing two sites in quick succession would then
       silently skip the second sync. -->
  <key>ThrottleInterval</key><integer>1</integer>
  <key>StandardErrorPath</key><string>{logs}/hostsd.log</string>
</dict>
</plist>
HOSTSD_EOF
chown root:wheel {plist}
chmod 644 {plist}
st_unload {label}
launchctl bootstrap system {plist} 2>/dev/null || true"#,
        dir = paths::HOSTSD_DIR,
        bin = paths::HOSTSD_BIN,
        src = src.to_string_lossy(),
        plist = paths::HOSTSD_PLIST,
        label = paths::HOSTSD_LABEL,
        request = paths::hosts_request().to_string_lossy(),
        logs = paths::logs_dir().to_string_lossy(),
    )
}

/// dnsmasq setup, or nothing at all when it is not installed.
///
/// It is optional: without it every domain is resolved through /etc/hosts,
/// which the helper writes with no password. Skipping it entirely keeps the
/// install from depending on Homebrew.
fn dns_block(cfg: &Config) -> String {
    if paths::dnsmasq_bin().is_none() || cfg.tlds_needing_dns().is_empty() {
        return "echo \"==> skipping dnsmasq (not installed; /etc/hosts covers every domain)\"".to_string();
    }

    let prefix = paths::brew_prefix();
    let conf_dir = paths::dnsmasq_conf_dir();
    let conf_file = paths::dnsmasq_conf_file();
    let address_lines: String = cfg
        .tlds_needing_dns()
        .iter()
        .map(|t| format!("address=/{t}/127.0.0.1\n"))
        .collect();
    let resolver_writes: String = cfg
        .tlds_needing_dns()
        .iter()
        .map(|t| {
            format!(
                "printf 'nameserver 127.0.0.1\\nport 53\\n' > {}/{t}\nchmod 644 {}/{t}\n",
                paths::RESOLVER_DIR,
                paths::RESOLVER_DIR
            )
        })
        .collect();

    DNS_TEMPLATE
        .replace("{conf_dir}", &conf_dir)
        .replace("{conf_file}", &conf_file)
        .replace("{address_lines}", &address_lines)
        .replace("{resolver_dir}", paths::RESOLVER_DIR)
        .replace("{resolver_writes}", &resolver_writes)
        .replace("{prefix}", prefix)
}

const DNS_TEMPLATE: &str = r#"echo "==> dnsmasq wildcard for local TLDs"
mkdir -p "{conf_dir}"
cat > "{conf_file}" <<'DNSMASQ_EOF'
# Generated by Portico
{address_lines}listen-address=127.0.0.1
DNSMASQ_EOF
chmod 644 "{conf_file}"

echo "==> macOS resolver entries"
mkdir -p {resolver_dir}
{resolver_writes}
echo "==> dnsmasq as a system daemon"
DNSMASQ_PLIST=/Library/LaunchDaemons/homebrew.mxcl.dnsmasq.plist
if [ ! -f "$DNSMASQ_PLIST" ]; then
cat > "$DNSMASQ_PLIST" <<'PLIST_EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>homebrew.mxcl.dnsmasq</string>
  <key>ProgramArguments</key>
  <array>
    <string>{prefix}/sbin/dnsmasq</string>
    <string>--keep-in-foreground</string>
    <string>-C</string>
    <string>{prefix}/etc/dnsmasq.conf</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
PLIST_EOF
chown root:wheel "$DNSMASQ_PLIST"
chmod 644 "$DNSMASQ_PLIST"
fi
st_unload homebrew.mxcl.dnsmasq
launchctl bootstrap system "$DNSMASQ_PLIST" 2>/dev/null || true
launchctl kickstart -k system/homebrew.mxcl.dnsmasq 2>/dev/null || true

"#;

fn uid() -> String {
    run("/usr/bin/id", &["-u"]).1
}

/// The LaunchAgent that keeps Caddy alive in standalone mode. It lives in the
/// user's own ~/Library/LaunchAgents, so installing it needs no privileges and
/// leaves nothing behind outside their home directory.
fn agent_plist_xml(cfg: &Config) -> String {
    let _ = cfg;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{caddy}</string>
    <string>run</string>
    <string>--config</string>
    <string>{conf}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>WorkingDirectory</key><string>{root}</string>
  <key>StandardOutPath</key><string>{logs}/caddy.out.log</string>
  <key>StandardErrorPath</key><string>{logs}/caddy.err.log</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>{home}</string>
    <key>XDG_DATA_HOME</key><string>{root}/caddy/data</string>
    <key>XDG_CONFIG_HOME</key><string>{root}/caddy/config</string>
  </dict>
</dict>
</plist>
"#,
        label = paths::DAEMON_LABEL,
        caddy = paths::caddy_bin(),
        conf = paths::caddyfile().to_string_lossy(),
        logs = paths::logs_dir().to_string_lossy(),
        root = paths::root().to_string_lossy(),
        home = paths::home().to_string_lossy(),
    )
}

/// Fetch Caddy's root certificate from its admin API and mark it trusted in
/// the *user* trust domain. `security add-trusted-cert` only needs an admin
/// when given `-d`; without it the change is scoped to this login account.
pub fn trust_ca_for_user() -> anyhow::Result<()> {
    let (ok, body) = run(
        "/usr/bin/curl",
        &["-s", "--max-time", "10", &format!("http://{}/pki/ca/local", paths::CADDY_ADMIN)],
    );
    if !ok || body.is_empty() {
        anyhow::bail!("Could not reach Caddy's admin API to fetch the root certificate");
    }

    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("Unexpected reply from Caddy admin API: {e}"))?;
    let pem = json
        .get("root_certificate")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Caddy did not return a root certificate"))?;

    let pem_path = paths::ca_pem();
    // A previous run in System mode may have left this file owned by root.
    // The directory is ours, so replacing it is always possible.
    if pem_path.exists()
        && std::fs::OpenOptions::new().write(true).open(&pem_path).is_err()
    {
        std::fs::remove_file(&pem_path)?;
    }
    std::fs::write(&pem_path, pem)?;

    let out = Command::new("/usr/bin/security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-p", "ssl", "-k"])
        .arg(paths::login_keychain())
        .arg(&pem_path)
        .output()?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("User canceled") || err.contains("-128") {
            anyhow::bail!("Cancelled at the keychain prompt — sites will show a certificate warning");
        }
        anyhow::bail!("Could not trust the local certificate: {}", err.trim());
    }
    Ok(())
}

/// Is this launchd job still loaded?
fn agent_loaded(label: &str) -> bool {
    Command::new("/bin/launchctl")
        .args(["print", label])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Retire a launchd job, retrying while it respawns itself.
fn unload_agent(label: &str) {
    for _ in 0..12 {
        if !agent_loaded(label) {
            return;
        }
        let _ = Command::new("/bin/launchctl").args(["bootout", label]).output();
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Logs left behind by a previous run in System mode belong to root. Caddy
/// cannot open them as the user and refuses to start with a bare "permission
/// denied", which reads as a broken install rather than a stale file.
///
/// This enumerates the directory rather than naming files. An earlier version
/// listed two names and silently went stale as access, hostsd, dev-server and
/// tunnel logs were added — each one a fresh way for the agent to fail.
fn reclaim_logs() {
    reclaim_dir(&paths::logs_dir());
}

/// Drop every file in `dir` that this process cannot append to.
fn reclaim_dir(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Unlinking needs write permission on the directory, which is ours,
        // not on the file itself.
        if std::fs::OpenOptions::new().append(true).open(&path).is_err() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn wait_for_caddy(seconds: u64) -> bool {
    for _ in 0..(seconds * 4) {
        if caddy::is_running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}

/// Set everything up without ever asking for an administrator password.
///
/// Nothing outside the user's home directory is written: no /etc/hosts, no
/// /etc/resolver, no /Library/LaunchDaemons. The trade-off is the port — 443
/// is unreachable without root, so sites are served on a high port instead.
pub fn install_standalone(cfg: &Config) -> anyhow::Result<String> {
    paths::ensure_dirs()?;
    std::fs::create_dir_all(paths::root().join("caddy/data"))?;
    std::fs::create_dir_all(paths::root().join("caddy/config"))?;
    reclaim_logs();
    caddy::write_config(cfg)?;

    let plist = paths::agent_plist();
    if let Some(dir) = plist.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&plist, agent_plist_xml(cfg))?;

    let u = uid();
    let target = format!("gui/{u}");
    let label = format!("gui/{u}/{}", paths::DAEMON_LABEL);

    // A KeepAlive job that is crash-looping respawns faster than a single
    // bootout can retire it. If the old definition survives, launchd keeps
    // running the *previous* plist and every edit here appears to do nothing —
    // so wait until the job is genuinely gone before loading the new one.
    unload_agent(&label);

    let out = Command::new("/bin/launchctl")
        .args(["bootstrap", &target])
        .arg(&plist)
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("Could not start the Caddy agent: {}", err.trim());
    }

    if !wait_for_caddy(15) {
        let log = std::fs::read_to_string(paths::logs_dir().join("caddy.err.log"))
            .unwrap_or_default();
        let tail: String = log.lines().rev().take(6).collect::<Vec<_>>().join("\n");
        anyhow::bail!("Caddy did not start.\n{tail}");
    }

    trust_ca_for_user()?;
    Ok("Standalone setup complete — no system files were changed.".to_string())
}

pub fn uninstall_standalone() -> anyhow::Result<String> {
    let u = uid();
    let label = format!("gui/{u}/{}", paths::DAEMON_LABEL);
    unload_agent(&label);
    let _ = std::fs::remove_file(paths::agent_plist());

    // Drop the trust setting and the certificate itself.
    let _ = Command::new("/usr/bin/security")
        .args(["remove-trusted-cert", "-r"])
        .arg(paths::ca_pem())
        .output();
    let _ = Command::new("/usr/bin/security")
        .args(["delete-certificate", "-c", "Caddy Local Authority", "-t"])
        .arg(paths::login_keychain())
        .output();
    let _ = std::fs::remove_file(paths::ca_pem());
    Ok("Removed the Caddy agent and its certificate. Nothing else was touched.".to_string())
}

/// One shell script covering every root-owned change, so the user is asked for
/// their password exactly once.
pub fn install_script(cfg: &Config) -> String {
    // dnsmasq's own configuration now lives in dns_block(), which omits it
    // entirely when dnsmasq is not installed.
    let caddy_bin = paths::caddy_bin();
    let caddyfile = paths::caddyfile().to_string_lossy().into_owned();
    let logs = paths::logs_dir().to_string_lossy().into_owned();
    let hosts_block = hosts::render_block(&cfg.hosts_entries());

    format!(
        r#"#!/bin/sh
# Portico setup — every root-owned change in one pass.
set -u

# Retire a launchd job and wait until it is really gone. A job that is already
# loaded makes `bootstrap` fail outright ("Input/output error"), and one with
# KeepAlive can respawn faster than a single bootout retires it.
st_unload() {{
  i=0
  while [ $i -lt 12 ]; do
    launchctl print "system/$1" >/dev/null 2>&1 || return 0
    launchctl bootout "system/$1" 2>/dev/null || true
    sleep 0.25
    i=$((i+1))
  done
}}

{dns_block}
echo "==> Caddy as a system daemon (needed to bind 80/443)"
mkdir -p "{logs}"
cat > {daemon_plist} <<'CADDY_EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{caddy_bin}</string>
    <string>run</string>
    <string>--config</string>
    <string>{caddyfile}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>WorkingDirectory</key><string>{st_root}</string>
  <key>StandardOutPath</key><string>{logs}/caddy.out.log</string>
  <key>StandardErrorPath</key><string>{logs}/caddy.err.log</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>/var/root</string>
    <key>XDG_DATA_HOME</key><string>{st_root}/caddy/data</string>
    <key>XDG_CONFIG_HOME</key><string>{st_root}/caddy/config</string>
  </dict>
</dict>
</plist>
CADDY_EOF
chown root:wheel {daemon_plist}
chmod 644 {daemon_plist}
st_unload {label}
launchctl bootstrap system {daemon_plist}
launchctl enable system/{label} 2>/dev/null || true

echo "==> waiting for Caddy admin API"
i=0
while [ $i -lt 30 ]; do
  if nc -z 127.0.0.1 2019 2>/dev/null; then break; fi
  sleep 0.5
  i=$((i+1))
done

echo "==> /etc/hosts helper (so future domains need no password)"
{hostsd_install}
echo "==> /etc/hosts entries for custom TLDs"
TMP_HOSTS=$(mktemp)
awk -v b='{begin_mark}' -v e='{end_mark}' \
  'index($0,b){{skip=1}} !skip{{print}} index($0,e){{skip=0}}' {hosts_file} > "$TMP_HOSTS"
cat >> "$TMP_HOSTS" <<'HOSTS_EOF'
{hosts_block}HOSTS_EOF
cat "$TMP_HOSTS" > {hosts_file}
rm -f "$TMP_HOSTS"

echo "==> flushing DNS cache"
dscacheutil -flushcache 2>/dev/null || true
killall -HUP mDNSResponder 2>/dev/null || true

echo "Portico setup complete."
"#,
        st_root = paths::root().to_string_lossy(),
        dns_block = dns_block(cfg),
        hostsd_install = hostsd_install_block(),
        daemon_plist = paths::LAUNCH_DAEMON,
        label = paths::DAEMON_LABEL,
        hosts_file = paths::HOSTS_FILE,
        begin_mark = hosts::BEGIN_MARK,
        end_mark = hosts::END_MARK,
    )
}

/// Run whichever setup the current mode calls for.
pub fn install(cfg: &Config) -> anyhow::Result<String> {
    match cfg.mode {
        Mode::Standalone => install_standalone(cfg),
        Mode::System => install_system(cfg),
    }
}

pub fn uninstall(cfg: &Config) -> anyhow::Result<String> {
    match cfg.mode {
        Mode::Standalone => uninstall_standalone(),
        Mode::System => privileged::run_as_root(&uninstall_script()),
    }
}

/// Apply every system change. Prompts for the admin password once.
fn install_system(cfg: &Config) -> anyhow::Result<String> {
    // Everything below is interpolated into a script and a plist that run as
    // root. Refuse rather than try to escape a hostile path for both grammars.
    for path in [
        paths::root().to_string_lossy().into_owned(),
        paths::home().to_string_lossy().into_owned(),
        paths::caddyfile().to_string_lossy().into_owned(),
        paths::logs_dir().to_string_lossy().into_owned(),
        paths::caddy_bin(),
    ] {
        if !paths::safe_path(&path) {
            anyhow::bail!(
                "Refusing to run a privileged setup: the path {path:?} contains characters \
                 that are unsafe to interpolate into a root script."
            );
        }
    }

    paths::ensure_dirs()?;
    // The helper daemon runs at load; give it a correct request file to find.
    let _ = hosts::write_request(&cfg.hosts_entries());
    // Write the Caddyfile first — the daemon reads it the moment it starts.
    caddy::write_config(cfg)?;

    // A broken per-user dnsmasq agent would fight the system daemon for :53.
    let _ = Command::new(format!("{}/bin/brew", paths::brew_prefix()))
        .args(["services", "stop", "dnsmasq"])
        .output();

    let out = privileged::run_as_root(&install_script(cfg))?;

    // Trust must happen outside the privileged script. `security` needs to
    // show its own authorization dialog, and nothing inside `do shell script
    // ... with administrator privileges` can present UI — it fails with
    // "authorization was denied since no user interaction was possible".
    // The user trust domain needs no admin at all and is what this machine's
    // browsers actually consult.
    if !wait_for_caddy(15) {
        anyhow::bail!("System setup ran, but Caddy did not come up");
    }
    trust_ca_for_user()?;

    Ok(out)
}

/// Re-apply only the /etc/hosts block — needed when a custom-TLD site is
/// added or removed. Wildcard-TLD sites never reach this path.
pub fn sync_hosts(cfg: &Config) -> anyhow::Result<String> {
    let block = hosts::render_block(&cfg.hosts_entries());
    let script = format!(
        r#"#!/bin/sh
set -u
TMP_HOSTS=$(mktemp)
awk -v b='{begin}' -v e='{end}' \
  'index($0,b){{skip=1}} !skip{{print}} index($0,e){{skip=0}}' {hosts_file} > "$TMP_HOSTS"
cat >> "$TMP_HOSTS" <<'HOSTS_EOF'
{block}HOSTS_EOF
cat "$TMP_HOSTS" > {hosts_file}
rm -f "$TMP_HOSTS"
dscacheutil -flushcache 2>/dev/null || true
killall -HUP mDNSResponder 2>/dev/null || true
echo "hosts updated"
"#,
        begin = hosts::BEGIN_MARK,
        end = hosts::END_MARK,
        hosts_file = paths::HOSTS_FILE,
    );
    privileged::run_as_root(&script)
}

/// Remove every system change Portico made. Site config is left alone.
pub fn uninstall_script() -> String {
    format!(
        r#"#!/bin/sh
set -u
launchctl bootout system/{label} 2>/dev/null || true
rm -f {plist}
launchctl bootout system/{hostsd_label} 2>/dev/null || true
rm -f {hostsd_plist}
rm -rf "{hostsd_dir}"
rm -f {dnsmasq_conf}
TMP_HOSTS=$(mktemp)
awk -v b='{begin}' -v e='{end}' \
  'index($0,b){{skip=1}} !skip{{print}} index($0,e){{skip=0}}' {hosts_file} > "$TMP_HOSTS"
cat "$TMP_HOSTS" > {hosts_file}
rm -f "$TMP_HOSTS"
launchctl kickstart -k system/homebrew.mxcl.dnsmasq 2>/dev/null || true
dscacheutil -flushcache 2>/dev/null || true
killall -HUP mDNSResponder 2>/dev/null || true
echo "Portico removed. The Caddy root CA is still in your keychain; remove it in Keychain Access if you want it gone."
"#,
        label = paths::DAEMON_LABEL,
        plist = paths::LAUNCH_DAEMON,
        hostsd_label = paths::HOSTSD_LABEL,
        hostsd_plist = paths::HOSTSD_PLIST,
        hostsd_dir = paths::HOSTSD_DIR,
        dnsmasq_conf = paths::dnsmasq_conf_file(),
        begin = hosts::BEGIN_MARK,
        end = hosts::END_MARK,
        hosts_file = paths::HOSTS_FILE,
    )
}


#[cfg(test)]
mod reclaim_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn every_unreadable_log_is_reclaimed_not_just_named_ones() {
        // The real failure: System mode leaves root-owned logs that the
        // user-level agent cannot open, and Caddy refuses to start. A file with
        // no permission bits reproduces that without needing root.
        let dir = std::env::temp_dir().join("portico-reclaim-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let blocked = ["access.log", "hostsd.log", "dev-x.log", "tunnel-x.log"];
        for n in blocked {
            let p = dir.join(n);
            std::fs::write(&p, b"stale").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        // A healthy log must survive; reclaiming is not "delete everything".
        let keep = dir.join("caddy.err.log");
        std::fs::write(&keep, b"fine").unwrap();

        reclaim_dir(&dir);

        for n in blocked {
            assert!(!dir.join(n).exists(), "{n} was left behind and would block startup");
        }
        assert!(keep.exists(), "a writable log was destroyed");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
