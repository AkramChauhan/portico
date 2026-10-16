use crate::caddy;
use crate::config::{self, Config, Mode};
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
        "Caddy agent running",
        running,
        if running {
            format!("admin API on {}", paths::CADDY_ADMIN)
        } else {
            "Not answering — run setup".to_string()
        },
        true,
    ));

    // Caddy runs as the user in both modes. System mode used a root daemon
    // until the loopback forwarding made port 443 reachable without one.
    let plist = paths::agent_plist();
    out.push(check(
        "agent",
        "Login agent installed",
        plist.exists(),
        if plist.exists() {
            "~/Library/LaunchAgents — Caddy runs as you, not root".to_string()
        } else {
            "Not installed yet".to_string()
        },
        true,
    ));

    // Portico trusts the CA in the user trust domain in both modes, so that is
    // what to look at. The System keychain only ever held it because a root
    // Caddy installed it there on its own.
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

    if !standalone {
        let legacy = exists(paths::LAUNCH_DAEMON);
        let forwarding = running && forwarding_reaches_caddy();
        out.push(check(
            "forwarding",
            "Ports 80 and 443 forwarded to Caddy",
            forwarding && !legacy,
            if legacy {
                "An older install still runs Caddy as root on ports 80 and 443 — run setup".to_string()
            } else if forwarding {
                format!(
                    "{} → 127.0.0.1:{} — port 443 itself stays free for other tools",
                    config::LOOPBACK_V4,
                    cfg.listen_https_port()
                )
            } else if !running {
                "Waiting for Caddy".to_string()
            } else {
                "Not forwarding — run setup".to_string()
            },
            true,
        ));

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
        let address = cfg.loopback().0;
        let answers: Vec<&str> = dig.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let ours = answers.contains(&address);
        let others: Vec<&str> = answers.iter().copied().filter(|a| *a != address).collect();
        out.push(check(
            "wildcard_dns",
            format!("*.{tld} resolves to {address}").as_str(),
            ours,
            if !ours && !others.is_empty() {
                format!("Answers {} instead, from an older setup — run setup", others.join(", "))
            } else if ours {
                format!("{probe} → {address}")
            } else {
                "dnsmasq is not answering on 127.0.0.1:53".to_string()
            },
            true,
        ));
        // A second answer comes from someone else's dnsmasq config — Laravel
        // Valet claims .test for 127.0.0.1. It was invisible while both said
        // 127.0.0.1; now a browser that picks that answer reaches whatever
        // holds 127.0.0.1:443 instead of the site. Not ours to delete, and not
        // something setup can fix, so it is reported without blocking.
        if ours && !others.is_empty() {
            out.push(check(
                "dns_conflict",
                format!("Nothing else claims *.{tld}").as_str(),
                false,
                format!(
                    "Another dnsmasq config also answers {} for .{tld} (Laravel Valet does) — \
                     .{tld} sites may intermittently miss Portico",
                    others.join(", ")
                ),
                false,
            ));
        }

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

    }

    // Custom domains need a hosts line in either mode.
    let need_hosts = cfg.hosts_entries();
    let stale = hosts::needs_sync(&need_hosts, cfg.loopback());
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

    // Anything already holding the port Caddy listens on will break every
    // site. In System mode that is the forwarded-to port, not 443: something
    // else on 443 is exactly what the forwarding exists to allow.
    let port = cfg.listen_https_port();
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

/// Does port 80 on the forwarded address actually reach *our* Caddy?
///
/// An open connection proves nothing — with the rules missing, whatever holds
/// 0.0.0.0:80 could answer. So ask for a hostname no site uses and look for
/// the catch-all's own reply, which only Portico's Caddyfile produces.
fn forwarding_reaches_caddy() -> bool {
    use std::io::{Read, Write};
    use std::net::{IpAddr, SocketAddr, TcpStream};
    use std::time::Duration;

    const PROBE_HOST: &str = "portico-forwarding-probe.invalid";
    let Ok(ip) = config::LOOPBACK_V4.parse::<IpAddr>() else {
        return false;
    };
    let addr = SocketAddr::new(ip, 80);
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET / HTTP/1.1\r\nHost: {PROBE_HOST}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut reply = Vec::new();
    let _ = stream.take(8192).read_to_end(&mut reply);
    String::from_utf8_lossy(&reply).contains(&format!("{}{PROBE_HOST}", caddy::UNMATCHED))
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
        .map(|t| {
            let (v4, v6) = cfg.loopback();
            format!("address=/{t}/{v4}\naddress=/{t}/{v6}\n")
        })
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

/// Load Caddy as a login agent and wait until it answers.
///
/// Both modes run Caddy this way. It needs no privileges: the agent lives in
/// ~/Library/LaunchAgents, and neither mode has Caddy bind a port below 1024.
fn start_agent(cfg: &Config) -> anyhow::Result<()> {
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
    Ok(())
}

fn stop_agent() {
    let label = format!("gui/{}/{}", uid(), paths::DAEMON_LABEL);
    unload_agent(&label);
    let _ = std::fs::remove_file(paths::agent_plist());
}

/// Set everything up without ever asking for an administrator password.
///
/// Nothing outside the user's home directory is written: no /etc/hosts, no
/// /etc/resolver, no /Library/LaunchDaemons. The trade-off is the port — 443
/// is unreachable without root, so sites are served on a high port instead.
pub fn install_standalone(cfg: &Config) -> anyhow::Result<String> {
    start_agent(cfg)?;
    trust_ca_for_user()?;
    Ok("Standalone setup complete — no system files were changed.".to_string())
}

pub fn uninstall_standalone() -> anyhow::Result<String> {
    stop_agent();

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

/// The pf rules: the forwarded addresses' 443 and 80, onto Caddy's
/// loopback-only listeners. Every value is a constant — nothing from config
/// reaches a ruleset root loads.
///
/// Scoped to those two destination addresses, never to 127.0.0.1 or to every
/// address. A rule matching 127.0.0.1:443 would quietly take the port back from
/// Docker, which is the one thing this whole arrangement exists to prevent.
pub fn pf_rules() -> String {
    let mut s = String::from("# Generated by Portico. Loaded into the com.apple/portico anchor.\n");
    for (family, from, to) in [
        ("inet", config::LOOPBACK_V4, "127.0.0.1"),
        ("inet6", config::LOOPBACK_V6, "::1"),
    ] {
        for (public, backend) in [
            (443, config::SYSTEM_HTTPS_BACKEND),
            (80, config::SYSTEM_HTTP_BACKEND),
        ] {
            s.push_str(&format!(
                "rdr pass on lo0 {family} proto tcp from any to {from} port {public} -> {to} port {backend}\n"
            ));
        }
    }
    s
}

/// The script launchd runs as root at every boot: loopback aliases, then the
/// pf rules. Neither survives a restart on its own.
pub fn loopback_script() -> String {
    format!(
        r#"#!/bin/sh
# Generated by Portico. Run as root by launchd at boot, and by setup.
#
# Gives lo0 the two addresses Portico's domains resolve to, and forwards their
# ports 80 and 443 to Caddy's loopback-only listeners. Caddy never holds 80 or
# 443 itself, so other tools — Docker in particular — can bind them.
set -u

ifconfig lo0 alias {v4} netmask 255.255.255.255 2>/dev/null || true
ifconfig lo0 inet6 {v6} prefixlen 128 alias 2>/dev/null || true

# /etc/pf.conf is what makes pf evaluate anchors under com.apple/. At boot it is
# loaded by Apple's own job, which may not have run yet; rules loaded before
# then would sit in an anchor nothing reaches.
i=0
while [ $i -lt 30 ]; do
  pfctl -s nat 2>/dev/null | grep -q 'rdr-anchor "com.apple/\*"' && break
  sleep 1
  i=$((i+1))
done

# Enabling pf returns a reference; holding exactly one means uninstall can
# release it without disabling pf for anything else that enabled it. Take the
# new reference before dropping the old, so pf is never briefly off.
OLD=$(cat "{token}" 2>/dev/null || true)
NEW=$(pfctl -a {anchor} -E -f "{rules}" 2>&1 | awk '/^Token/ {{ print $3 }}')
if [ -n "$NEW" ]; then
  echo "$NEW" > "{token}"
  chmod 600 "{token}"
fi
if [ -n "$OLD" ] && [ "$OLD" != "$NEW" ]; then
  pfctl -X "$OLD" >/dev/null 2>&1 || true
fi

if pfctl -a {anchor} -s nat 2>/dev/null | grep -q "port = 443"; then
  echo "portico-loopback: forwarding {v4} and {v6} ports 80/443"
else
  echo "portico-loopback: pf rules did not load" >&2
  exit 1
fi
"#,
        v4 = config::LOOPBACK_V4,
        v6 = config::LOOPBACK_V6,
        anchor = paths::PF_ANCHOR,
        rules = paths::PF_RULES,
        token = paths::PF_TOKEN,
    )
}

/// Shell that installs the loopback forwarding and runs it once now.
fn loopback_install_block() -> String {
    format!(
        r#"mkdir -p "{dir}"
chown root:wheel "{dir}"
chmod 755 "{dir}"
cat > "{rules}" <<'PF_EOF'
{pf_rules}PF_EOF
chown root:wheel "{rules}"
chmod 644 "{rules}"
cat > "{script}" <<'LOOPBACK_EOF'
{loopback}LOOPBACK_EOF
chown root:wheel "{script}"
chmod 755 "{script}"
cat > {plist} <<'LOOPBACK_PLIST_EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>{script}</string>
  </array>
  <!-- Once per boot. Aliases and pf rules are both lost on restart. -->
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
LOOPBACK_PLIST_EOF
chown root:wheel {plist}
chmod 644 {plist}
st_unload {label}
# Run it here rather than relying on RunAtLoad, so setup knows it worked
# before it reports success. Loading the job then arms it for the next boot;
# its own run now is the same idempotent work a second time.
/bin/sh "{script}"
launchctl bootstrap system {plist} 2>/dev/null || true"#,
        dir = paths::HOSTSD_DIR,
        rules = paths::PF_RULES,
        pf_rules = pf_rules(),
        script = paths::LOOPBACK_SCRIPT,
        loopback = loopback_script(),
        plist = paths::LOOPBACK_PLIST,
        label = paths::LOOPBACK_LABEL,
        log = paths::LOOPBACK_LOG,
    )
}

/// One shell script covering every root-owned change, so the user is asked for
/// their password exactly once.
pub fn install_script(cfg: &Config) -> String {
    // dnsmasq's own configuration now lives in dns_block(), which omits it
    // entirely when dnsmasq is not installed.
    let logs = paths::logs_dir().to_string_lossy().into_owned();
    let hosts_block = hosts::render_block(&cfg.hosts_entries(), cfg.loopback());

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
echo "==> retiring the root Caddy daemon from older versions"
# It bound 0.0.0.0:80 and :443, which is what blocked Docker. Caddy now runs as
# a login agent on loopback-only high ports.
st_unload {label}
rm -f {daemon_plist}

echo "==> handing Caddy's files back to you"
# A root Caddy left its certificates and logs owned by root. The agent runs as
# you and could not read its own CA key, so it would mint a new, untrusted one.
mkdir -p "{logs}"
chown -R {uid} "{st_root}/caddy" "{logs}" 2>/dev/null || true

echo "==> loopback forwarding for ports 80 and 443"
{loopback_install}

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
        uid = uid(),
        dns_block = dns_block(cfg),
        loopback_install = loopback_install_block(),
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
        Mode::System => {
            // Root first: if the password prompt is cancelled, the agent and
            // every site keep working.
            let out = privileged::run_as_root(&uninstall_script())?;
            stop_agent();
            Ok(out)
        }
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
    caddy::write_config(cfg)?;

    // A broken per-user dnsmasq agent would fight the system daemon for :53.
    let _ = Command::new(format!("{}/bin/brew", paths::brew_prefix()))
        .args(["services", "stop", "dnsmasq"])
        .output();

    // The root script retires any old root Caddy before the agent starts, so
    // the two never contend for the admin port.
    let out = privileged::run_as_root(&install_script(cfg))?;
    start_agent(cfg)?;

    // Trust must happen outside the privileged script. `security` needs to
    // show its own authorization dialog, and nothing inside `do shell script
    // ... with administrator privileges` can present UI — it fails with
    // "authorization was denied since no user interaction was possible".
    // The user trust domain needs no admin at all and is what this machine's
    // browsers actually consult.
    trust_ca_for_user()?;

    if !forwarding_reaches_caddy() {
        anyhow::bail!(
            "Setup ran, but {}:80 is not reaching Caddy — see {}",
            config::LOOPBACK_V4,
            paths::LOOPBACK_LOG
        );
    }
    Ok(out)
}

/// Re-apply only the /etc/hosts block — needed when a custom-TLD site is
/// added or removed. Wildcard-TLD sites never reach this path.
pub fn sync_hosts(cfg: &Config) -> anyhow::Result<String> {
    let block = hosts::render_block(&cfg.hosts_entries(), cfg.loopback());
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
launchctl bootout system/{loopback_label} 2>/dev/null || true
rm -f {loopback_plist} {loopback_log}
pfctl -a {anchor} -F all >/dev/null 2>&1 || true
if [ -s "{token}" ]; then
  pfctl -X "$(cat "{token}")" >/dev/null 2>&1 || true
fi
ifconfig lo0 -alias {v4} 2>/dev/null || true
ifconfig lo0 inet6 {v6} -alias 2>/dev/null || true
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
        loopback_label = paths::LOOPBACK_LABEL,
        loopback_plist = paths::LOOPBACK_PLIST,
        loopback_log = paths::LOOPBACK_LOG,
        anchor = paths::PF_ANCHOR,
        token = paths::PF_TOKEN,
        v4 = config::LOOPBACK_V4,
        v6 = config::LOOPBACK_V6,
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

#[cfg(test)]
mod forwarding_tests {
    use super::*;

    fn parses_as_shell(script: &str) -> Result<(), String> {
        use std::io::Write;
        let mut child = Command::new("/bin/sh")
            .arg("-n")
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    #[test]
    fn every_root_script_is_valid_shell() {
        // These run as root behind a password prompt; a syntax error there is
        // found by the user, halfway through changing their system.
        let cfg = Config::default();
        for (name, script) in [
            ("install", install_script(&cfg)),
            ("loopback", loopback_script()),
            ("uninstall", uninstall_script()),
        ] {
            parses_as_shell(&script).unwrap_or_else(|e| panic!("{name} script: {e}\n{script}"));
        }
    }

    #[test]
    fn pf_only_ever_redirects_portico_addresses() {
        // A rule for 127.0.0.1:443, or for any destination, would take the
        // port back from Docker without anything failing visibly.
        let rules = pf_rules();
        let lines: Vec<&str> = rules.lines().filter(|l| l.starts_with("rdr")).collect();
        assert_eq!(lines.len(), 4, "443 and 80, for each stack:\n{rules}");
        for l in &lines {
            assert!(l.starts_with("rdr pass on lo0 "), "must stay on loopback: {l}");
            assert!(
                l.contains(&format!("to {} port", config::LOOPBACK_V4))
                    || l.contains(&format!("to {} port", config::LOOPBACK_V6)),
                "redirects an address that is not Portico's: {l}"
            );
            assert!(!l.contains("to any"), "{l}");
        }
        assert!(rules.contains(&format!("port 443 -> 127.0.0.1 port {}", config::SYSTEM_HTTPS_BACKEND)));
        assert!(rules.contains(&format!("port 80 -> ::1 port {}", config::SYSTEM_HTTP_BACKEND)));
    }

    #[test]
    fn setup_retires_the_root_caddy_and_never_recreates_it() {
        let script = install_script(&Config::default());
        assert!(script.contains(&format!("st_unload {}", paths::DAEMON_LABEL)));
        assert!(script.contains(&format!("rm -f {}", paths::LAUNCH_DAEMON)));
        assert!(!script.contains("<string>run</string>"), "a root Caddy daemon is still being written");
        assert!(script.contains(paths::LOOPBACK_PLIST));
    }

    #[test]
    fn uninstall_leaves_no_forwarding_behind() {
        let script = uninstall_script();
        assert!(script.contains(&format!("pfctl -a {} -F all", paths::PF_ANCHOR)));
        assert!(script.contains("pfctl -X"), "the pf reference must be released");
        assert!(script.contains(&format!("-alias {}", config::LOOPBACK_V4)));
        assert!(script.contains(&format!("{} -alias", config::LOOPBACK_V6)));
        assert!(script.contains(paths::LOOPBACK_PLIST));
        // The token lives in the directory removed at the end; reading it
        // after that would release nothing.
        let released = script.find("pfctl -X").unwrap();
        let removed = script.find(&format!("rm -rf \"{}\"", paths::HOSTSD_DIR)).unwrap();
        assert!(released < removed);
    }
}
