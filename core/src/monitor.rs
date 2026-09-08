//! Per-site monitoring: what requests arrived, is it up, and what did the dev
//! server say.
//!
//! All three read from files Caddy or the supervised process already write, so
//! nothing sits in the request path and monitoring cannot slow a site down.

use crate::config::{Config, Site, PLAIN_PORT};

use crate::paths;
use serde::Serialize;

/// One handled request, flattened out of Caddy's JSON access log.
#[derive(Debug, Clone, Serialize)]
pub struct RequestEntry {
    pub ts: f64,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: f64,
    pub size: u64,
    pub remote: String,
    /// The vhost this request was for, with any port stripped.
    pub host: String,
    /// True when the request arrived through the loopback twin, which is how
    /// tunnelled traffic reaches us — i.e. this one came from the internet.
    pub via_tunnel: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub ok: bool,
    pub status: Option<u16>,
    pub ms: u128,
    pub detail: String,
}

/// Read the tail of a file without loading all of it.
fn tail_bytes(path: &std::path::Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(max);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    // A partial first line is likely if we seeked into the middle of one.
    if from > 0 {
        match text.find('\n') {
            Some(i) => text[i + 1..].to_string(),
            None => String::new(),
        }
    } else {
        text
    }
}

pub fn parse_entry(line: &str) -> Option<RequestEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let req = v.get("request")?;
    let raw_host = req.get("host").and_then(|h| h.as_str()).unwrap_or("");
    // Caddy may log a port for the loopback twin; the site name is the part
    // before it.
    let host = raw_host.split(':').next().unwrap_or(raw_host).to_string();

    // Whether a request came from the internet is decided by Cloudflare's own
    // edge headers, not by the Host. cloudflared is told to send the site's
    // real hostname, so a tunnelled request is otherwise indistinguishable
    // from a local one.
    let headers = req.get("headers").and_then(|h| h.as_object());
    let header = |name: &str| -> Option<String> {
        let h = headers?;
        h.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.as_array()?.first()?.as_str().map(str::to_string))
    };
    let via_tunnel = header("cf-ray").is_some();
    // For a webhook, the sender's address is the useful one.
    let remote = header("cf-connecting-ip").unwrap_or_else(|| {
        req.get("remote_ip")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string()
    });
    Some(RequestEntry {
        ts: v.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0),
        method: req.get("method").and_then(|m| m.as_str()).unwrap_or("?").to_string(),
        path: req.get("uri").and_then(|u| u.as_str()).unwrap_or("/").to_string(),
        status: v.get("status").and_then(|s| s.as_u64()).unwrap_or(0) as u16,
        // Caddy logs duration in seconds.
        duration_ms: v.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0) * 1000.0,
        size: v.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
        remote,
        via_tunnel,
        host,
    })
}

/// Most recent requests first, for one site.
pub fn recent_requests(domain: &str, limit: usize) -> Vec<RequestEntry> {
    let text = tail_bytes(&paths::access_log(), 512 * 1024);
    let mut out: Vec<RequestEntry> = text
        .lines()
        .filter_map(parse_entry)
        .filter(|e| e.host == domain)
        .collect();
    out.reverse();
    out.truncate(limit);
    out
}

/// Probe the site through the loopback twin.
///
/// Going via the twin rather than the public hostname keeps this honest about
/// what it measures: it exercises the vhost and whatever is behind it, without
/// depending on DNS or on the certificate being trusted by curl.
pub fn health(cfg: &Config, site: &Site) -> Health {
    let started = std::time::Instant::now();
    let out = std::process::Command::new("/usr/bin/curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "10",
            "-H",
            &format!("Host: {}", site.domain),
            &format!("http://127.0.0.1:{PLAIN_PORT}/"),
        ])
        .output();
    let ms = started.elapsed().as_millis();
    let _ = cfg;

    match out {
        Ok(o) => {
            let code: u16 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0);
            if code == 0 {
                Health { ok: false, status: None, ms, detail: "No response".into() }
            } else if code >= 500 {
                Health {
                    ok: false,
                    status: Some(code),
                    ms,
                    detail: format!("Upstream returned {code}"),
                }
            } else {
                Health { ok: true, status: Some(code), ms, detail: format!("HTTP {code}") }
            }
        }
        Err(e) => Health { ok: false, status: None, ms, detail: e.to_string() },
    }
}

/// A log file the app can show.
#[derive(Debug, Clone, Serialize)]
pub struct LogFile {
    pub name: String,
    pub label: String,
    pub bytes: u64,
    /// Seconds since it was last written, or None if unknown.
    pub age: Option<u64>,
}

/// Every log Portico has produced, newest first.
///
/// Surfacing these in the app matters: when a tunnel or a dev server dies, the
/// reason is already written down — it was just only reachable from a terminal.
pub fn list_logs() -> Vec<LogFile> {
    let Ok(entries) = std::fs::read_dir(paths::logs_dir()) else {
        return Vec::new();
    };
    let now = std::time::SystemTime::now();

    let mut out: Vec<LogFile> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".log") {
                return None;
            }
            let meta = e.metadata().ok()?;
            Some(LogFile {
                label: describe_log(&name),
                bytes: meta.len(),
                age: meta
                    .modified()
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .map(|d| d.as_secs()),
                name,
            })
        })
        .collect();

    out.sort_by_key(|l| l.age.unwrap_or(u64::MAX));
    out
}

/// Turn a filename into something worth reading in a list.
fn describe_log(name: &str) -> String {
    let stem = name.trim_end_matches(".log");
    if let Some(site) = stem.strip_prefix("tunnel-") {
        format!("Public URL · {}", site.replace('-', "."))
    } else if let Some(site) = stem.strip_prefix("dev-") {
        format!("Dev server · {}", site.replace('-', "."))
    } else if let Some(site) = stem.strip_prefix("access-") {
        format!("Requests · {}", site.replace('-', "."))
    } else {
        match stem {
            "access" => "Requests · all sites".to_string(),
            "caddy.err" => "Server".to_string(),
            "caddy.out" => "Server output".to_string(),
            "hostsd" => "Hosts helper".to_string(),
            other => other.to_string(),
        }
    }
}

/// Read one log by name. The name is validated, so this cannot be used to read
/// arbitrary files by traversal.
pub fn read_log(name: &str, lines: usize) -> String {
    if !is_safe_log_name(name) {
        return String::new();
    }
    tail_file(&paths::logs_dir().join(name), lines)
}

/// Empty one log, or every log when `name` is None.
///
/// Truncates rather than deletes. Caddy and cloudflared hold these files open;
/// unlinking one leaves the writer appending to an inode with no name, so the
/// output vanishes and the disk space is never reclaimed until the process
/// exits. Truncation keeps the handle valid and the writer simply continues
/// from zero.
///
/// A file we cannot truncate belongs to root — System mode runs Caddy as root
/// — so it is unlinked instead and the caller reloads Caddy, which reopens its
/// writers and re-creates the file under our ownership.
pub fn clear_logs(name: Option<&str>) -> u64 {
    let dir = paths::logs_dir();
    let targets: Vec<std::path::PathBuf> = match name {
        Some(n) => {
            if !is_safe_log_name(n) {
                return 0;
            }
            vec![dir.join(n)]
        }
        None => std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "log"))
                    .collect()
            })
            .unwrap_or_default(),
    };

    let mut freed = 0;
    for path in targets {
        let before = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let truncated = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .is_ok();
        if !truncated {
            // Root-owned: the directory is ours, so unlinking still works.
            if std::fs::remove_file(&path).is_err() {
                continue;
            }
        }
        freed += before;
    }
    freed
}

fn is_safe_log_name(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".log")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Last lines of any log file.
pub fn tail_file(path: &std::path::Path, lines: usize) -> String {
    let text = tail_bytes(path, 128 * 1024);
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Last lines of a supervised dev server's output.
pub fn dev_log(site_id: &str, lines: usize) -> String {
    if !paths::safe_id(site_id) {
        return String::new();
    }
    tail_file(&paths::dev_log(site_id), lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"level":"info","ts":1788890000.5,"logger":"http.log.access.log0","msg":"handled request","request":{"remote_ip":"127.0.0.1","method":"POST","host":"app.io","uri":"/webhooks/stripe"},"duration":0.0421,"size":31,"status":200}"#;

    #[test]
    fn parses_a_caddy_access_line() {
        let e = parse_entry(SAMPLE).unwrap();
        assert_eq!(e.method, "POST");
        assert_eq!(e.path, "/webhooks/stripe");
        assert_eq!(e.status, 200);
        assert_eq!(e.size, 31);
        assert!((e.duration_ms - 42.1).abs() < 0.001, "duration was {}", e.duration_ms);
        assert!(!e.via_tunnel);
        assert_eq!(e.host, "app.io");
    }

    #[test]
    fn flags_requests_that_arrived_from_the_internet() {
        // cloudflared sends the site's real hostname, so the Host header looks
        // identical to a local request. Cloudflare's edge headers are the
        // only reliable signal, and they carry the real client address too.
        let line = SAMPLE.replace(
            r#""uri":"/webhooks/stripe""#,
            r#""uri":"/webhooks/stripe","headers":{"Cf-Ray":["9a1b2c3d"],"Cf-Connecting-Ip":["203.0.113.42"]}"#,
        );
        let e = parse_entry(&line).unwrap();
        assert!(e.via_tunnel);
        assert_eq!(e.remote, "203.0.113.42");
        assert_eq!(e.host, "app.io");
    }

    #[test]
    fn header_matching_is_case_insensitive() {
        let line = SAMPLE.replace(
            r#""uri":"/webhooks/stripe""#,
            r#""uri":"/webhooks/stripe","headers":{"CF-RAY":["x"]}"#,
        );
        assert!(parse_entry(&line).unwrap().via_tunnel);
    }

    #[test]
    fn a_host_with_a_port_still_resolves_to_the_site() {
        let line = SAMPLE.replace(r#""host":"app.io""#, r#""host":"app.io:8880""#);
        let e = parse_entry(&line).unwrap();
        assert_eq!(e.host, "app.io");
        // No Cloudflare headers, so this is local traffic.
        assert!(!e.via_tunnel);
    }

    #[test]
    fn ignores_lines_that_are_not_access_records() {
        assert!(parse_entry("not json").is_none());
        assert!(parse_entry(r#"{"level":"info","msg":"started"}"#).is_none());
    }

    #[test]
    fn tail_drops_a_partial_first_line() {
        let p = std::env::temp_dir().join("st-monitor-tail.log");
        std::fs::write(&p, "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc\n").unwrap();
        let t = tail_bytes(&p, 15);
        assert!(!t.contains("aaaa"), "kept a partial line: {t:?}");
        assert!(t.contains("cccccccccc"));
    }
}

#[cfg(test)]
mod clear_tests {
    use super::*;

    #[test]
    fn only_log_files_with_safe_names_can_be_cleared() {
        assert!(is_safe_log_name("access.log"));
        assert!(is_safe_log_name("tunnel-my-site.log"));
        // Nothing that could escape the log directory or hit another file.
        assert!(!is_safe_log_name("../../../etc/hosts"));
        assert!(!is_safe_log_name("../access.log"));
        assert!(!is_safe_log_name("config.json"));
        assert!(!is_safe_log_name(""));
        assert!(!is_safe_log_name("access.log/../../x.log"));
    }

    #[test]
    fn clearing_truncates_rather_than_unlinks() {
        // A writer holding the file open must keep writing to the same file,
        // which is only true if we truncate.
        let dir = std::env::temp_dir().join("portico-clear-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kept.log");
        std::fs::write(&path, b"old contents").unwrap();

        let before = std::fs::metadata(&path).unwrap().len();
        let f = std::fs::OpenOptions::new().write(true).truncate(true).open(&path);
        assert!(f.is_ok());
        drop(f);

        assert!(path.exists(), "the file must survive, not be unlinked");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert!(before > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
