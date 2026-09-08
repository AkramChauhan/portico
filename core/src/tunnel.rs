use crate::config::PLAIN_PORT;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelStatus {
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TunnelInfo {
    pub status: TunnelStatus,
    pub url: Option<String>,
    pub error: Option<String>,
}

impl TunnelInfo {
    fn stopped() -> Self {
        TunnelInfo { status: TunnelStatus::Stopped, url: None, error: None }
    }
}

struct Handle {
    child: Child,
    info: Arc<Mutex<TunnelInfo>>,
}

fn registry() -> &'static Mutex<HashMap<String, Handle>> {
    static REG: OnceLock<Mutex<HashMap<String, Handle>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Locate cloudflared, preferring a copy the user manages themselves.
///
/// Their own install stays authoritative — if they have it from Homebrew we
/// use that and let brew keep it updated, rather than shadowing it with ours.
pub fn cloudflared_bin() -> Option<String> {
    let mut candidates: Vec<String> = vec![
        "/opt/homebrew/bin/cloudflared".to_string(),
        "/usr/local/bin/cloudflared".to_string(),
    ];
    candidates.push(crate::paths::managed_cloudflared().to_string_lossy().into_owned());

    candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

pub fn is_installed() -> bool {
    cloudflared_bin().is_some()
}

/// Fetch cloudflared if it is not already present.
///
/// Public tunnels need no Cloudflare account — a quick tunnel is anonymous —
/// but they do need this binary, and asking a user to run `brew install` first
/// is exactly the manual step this app exists to remove.
///
/// The download comes over HTTPS from Cloudflare's official GitHub release, is
/// unpacked into our own directory, and is then required to run and report a
/// version before we will use it.
pub fn ensure_cloudflared() -> anyhow::Result<String> {
    if let Some(found) = cloudflared_bin() {
        return Ok(found);
    }
    install_cloudflared()
}

/// Download cloudflared into our own directory, whether or not one is already
/// present elsewhere.
pub fn install_cloudflared() -> anyhow::Result<String> {
    let status = crate::bins::ensure_cloudflared_reporting(true, &crate::bins::no_progress)?;
    status
        .path
        .ok_or_else(|| anyhow::anyhow!("cloudflared did not install"))
}

pub fn extract_url(line: &str) -> Option<String> {
    let idx = line.find("https://")?;
    let rest = &line[idx..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|' || c == '"')
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',']).to_string();
    if url.contains("trycloudflare.com") {
        Some(url)
    } else {
        None
    }
}

/// Start a public tunnel for one site.
///
/// cloudflared is pointed at the loopback plain-HTTP twin rather than the
/// HTTPS vhost, so it never needs to trust the local CA; `--http-host-header`
/// makes the request arrive at Caddy under the site's real hostname.
pub fn start(site_id: &str, domain: &str) -> anyhow::Result<()> {
    stop(site_id);

    let cfg = crate::config::Config::load();
    let provider = cfg.tunnel_provider;

    // Each provider needs a different binary, different arguments and a
    // different way of finding the URL in its output.
    let (bin, args) = match provider {
        crate::ngrok::Provider::Cloudflare => {
            // Fetches it on first use if needed, so turning a site public never
            // depends on the user having installed anything.
            let bin = ensure_cloudflared()?;
            let args = vec![
                "tunnel".to_string(),
                "--no-autoupdate".to_string(),
                "--url".to_string(),
                format!("http://127.0.0.1:{PLAIN_PORT}"),
                "--http-host-header".to_string(),
                domain.to_string(),
            ];
            (bin, args)
        }
        crate::ngrok::Provider::Ngrok => {
            let bin = crate::ngrok::bin()
                .ok_or_else(|| anyhow::anyhow!("ngrok is not installed"))?;
            let status = crate::ngrok::status(cfg.ngrok_plan.as_deref());
            if !status.configured {
                anyhow::bail!(
                    "ngrok has no authtoken. Add one in Settings — ngrok requires an account."
                );
            }
            let custom = (!cfg.ngrok_domain.trim().is_empty()).then_some(cfg.ngrok_domain.as_str());
            (bin, crate::ngrok::tunnel_args(PLAIN_PORT, domain, custom))
        }
    };

    let info = Arc::new(Mutex::new(TunnelInfo {
        status: TunnelStatus::Starting,
        url: None,
        error: None,
    }));

    let mut child = Command::new(bin)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // cloudflared logs the URL on stderr; ngrok on stdout. Read both so the
    // provider does not have to be special-cased twice.
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(child.stderr.take().expect("stderr piped")),
        Box::new(child.stdout.take().expect("stdout piped")),
    ];

    let log_path = crate::paths::tunnel_log(site_id);
    let _ = crate::paths::ensure_dirs();
    let log = Arc::new(Mutex::new(std::fs::File::create(&log_path).ok()));

    for stream in streams {
        let info = Arc::clone(&info);
        let log = Arc::clone(&log);
        let log_path = log_path.clone();
        std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if let Ok(mut f) = log.lock() {
                    if let Some(f) = f.as_mut() {
                        use std::io::Write;
                        let _ = writeln!(f, "{line}");
                    }
                }

                let found = match provider {
                    crate::ngrok::Provider::Cloudflare => extract_url(&line),
                    crate::ngrok::Provider::Ngrok => crate::ngrok::extract_url(&line),
                };
                if let Some(url) = found {
                    let mut i = info.lock().unwrap();
                    i.url = Some(url);
                    i.status = TunnelStatus::Running;
                    continue;
                }

                tail.push(line);
                if tail.len() > 40 {
                    tail.remove(0);
                }
            }

            // Stream closed. Only the first one to notice reports a failure.
            let mut i = info.lock().unwrap();
            if i.status == TunnelStatus::Starting {
                i.status = TunnelStatus::Failed;
                i.error = Some(
                    tail.iter()
                        .rev()
                        .find(|l| l.contains("ERR_NGROK") || l.contains("ERR") || l.contains("error"))
                        .cloned()
                        .unwrap_or_else(|| {
                            format!("Tunnel exited before publishing a URL — see {}", log_path.display())
                        }),
                );
            }
        });
    }

    registry()
        .lock()
        .unwrap()
        .insert(site_id.to_string(), Handle { child, info });
    Ok(())
}

pub fn stop(site_id: &str) {
    if let Some(mut h) = registry().lock().unwrap().remove(site_id) {
        let _ = h.child.kill();
        let _ = h.child.wait();
    }
}

pub fn stop_all() {
    let ids: Vec<String> = registry().lock().unwrap().keys().cloned().collect();
    for id in ids {
        stop(&id);
    }
}

pub fn status(site_id: &str) -> TunnelInfo {
    registry()
        .lock()
        .unwrap()
        .get(site_id)
        .map(|h| h.info.lock().unwrap().clone())
        .unwrap_or_else(TunnelInfo::stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quick_tunnel_url() {
        let line = "2026-09-08T10:00:00Z INF |  https://brave-lion-runs-fast.trycloudflare.com   |";
        assert_eq!(
            extract_url(line).as_deref(),
            Some("https://brave-lion-runs-fast.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_other_urls() {
        assert!(extract_url("see https://developers.cloudflare.com/docs for help").is_none());
        assert!(extract_url("no url at all").is_none());
    }

    #[test]
    fn unknown_site_reports_stopped() {
        assert_eq!(status("nope").status, TunnelStatus::Stopped);
    }
}
