//! Fetching the tools Portico needs.
//!
//! The point is that installing the app is the only thing a user does: neither
//! Homebrew nor any manual download should be a prerequisite. Anything already
//! installed is used as-is, so we never shadow a copy the user maintains.

use crate::paths;
use serde::Serialize;
use std::process::{Command, Stdio};

/// A step in the install, reported as it happens so the UI can show progress
/// instead of an unexplained pause on a 44 MB download.
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub tool: String,
    /// "checking" | "downloading" | "verifying" | "ready" | "skipped"
    pub phase: String,
    pub downloaded: u64,
    /// Zero when the server does not say how big the file is.
    pub total: u64,
    pub detail: String,
}

/// Somewhere to send progress. A no-op closure is fine.
pub type Reporter<'a> = &'a (dyn Fn(Progress) + Send + Sync);

pub fn no_progress(_: Progress) {}

/// Download a file, reporting how far along it is.
///
/// Progress is measured by watching the output file grow rather than by
/// parsing curl's output: it is simpler, and it still works when the server
/// sends no Content-Length (Caddy's build endpoint does not).
fn download_with_progress(
    url: &str,
    dest: &std::path::Path,
    tool: &str,
    report: Reporter,
) -> anyhow::Result<()> {
    let total = content_length(url);

    report(Progress {
        tool: tool.into(),
        phase: "downloading".into(),
        downloaded: 0,
        total,
        detail: if total > 0 {
            format!("Downloading {tool} ({:.0} MB)", total as f64 / 1_048_576.0)
        } else {
            format!("Downloading {tool}")
        },
    });

    let _ = std::fs::remove_file(dest);
    let mut child = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "600", "-o"])
        .arg(dest)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    loop {
        match child.try_wait()? {
            Some(status) => {
                if !status.success() {
                    anyhow::bail!("Download of {tool} failed");
                }
                break;
            }
            None => {
                let got = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
                report(Progress {
                    tool: tool.into(),
                    phase: "downloading".into(),
                    downloaded: got,
                    total,
                    detail: format!("Downloading {tool}"),
                });
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }

    report(Progress {
        tool: tool.into(),
        phase: "verifying".into(),
        downloaded: total,
        total,
        detail: format!("Checking {tool} runs"),
    });
    Ok(())
}

fn content_length(url: &str) -> u64 {
    let out = Command::new("/usr/bin/curl")
        .args(["-sIL", "--max-time", "15", "-o", "/dev/null", "-w", "%{size_download}", url])
        .output();
    // A HEAD gives no body, so ask curl for the header instead.
    let _ = out;
    let headers = Command::new("/usr/bin/curl")
        .args(["-sIL", "--max-time", "15", url])
        .output();
    let Ok(h) = headers else { return 0 };
    String::from_utf8_lossy(&h.stdout)
        .lines()
        .filter(|l| l.to_ascii_lowercase().starts_with("content-length:"))
        .filter_map(|l| l.split(':').nth(1)?.trim().parse::<u64>().ok())
        .next_back()
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub name: String,
    pub path: Option<String>,
    /// True when this run had to fetch it.
    pub downloaded: bool,
    /// False only for optional tools we could not obtain.
    pub ok: bool,
    pub detail: String,
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "amd64",
    }
}

fn verify_runs(path: &std::path::Path, arg: &str) -> bool {
    Command::new(path)
        .arg(arg)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Caddy, from the project's own versionless download endpoint.
pub fn ensure_caddy() -> anyhow::Result<ToolStatus> {
    ensure_caddy_inner(false)
}

/// Download Caddy into our own directory even if one is already installed.
pub fn install_caddy() -> anyhow::Result<ToolStatus> {
    ensure_caddy_inner(true)
}

fn ensure_caddy_inner(force: bool) -> anyhow::Result<ToolStatus> {
    ensure_caddy_reporting(force, &no_progress)
}

pub fn ensure_caddy_reporting(force: bool, report: Reporter) -> anyhow::Result<ToolStatus> {
    if let (false, Some(found)) = (force, paths::find_bin("caddy")) {
        report(Progress {
            tool: "caddy".into(),
            phase: "ready".into(),
            downloaded: 0,
            total: 0,
            detail: format!("Already installed — {found}"),
        });
        return Ok(ToolStatus {
            name: "caddy".into(),
            path: Some(found.clone()),
            downloaded: false,
            ok: true,
            detail: found,
        });
    }

    let dir = paths::bin_dir();
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join("caddy");
    let url = format!("https://caddyserver.com/api/download?os=darwin&arch={}", arch());

    download_with_progress(&url, &dest, "caddy", report)?;

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
    if !verify_runs(&dest, "version") {
        let _ = std::fs::remove_file(&dest);
        anyhow::bail!("The downloaded Caddy would not run");
    }

    report(Progress {
        tool: "caddy".into(),
        phase: "ready".into(),
        downloaded: 0,
        total: 0,
        detail: "Installed".into(),
    });
    Ok(ToolStatus {
        name: "caddy".into(),
        path: Some(dest.to_string_lossy().into_owned()),
        downloaded: true,
        ok: true,
        detail: dest.to_string_lossy().into_owned(),
    })
}

pub fn ensure_cloudflared() -> anyhow::Result<ToolStatus> {
    ensure_cloudflared_reporting(false, &no_progress)
}

pub fn ensure_cloudflared_reporting(force: bool, report: Reporter) -> anyhow::Result<ToolStatus> {
    if let (false, Some(found)) = (force, crate::tunnel::cloudflared_bin()) {
        report(Progress {
            tool: "cloudflared".into(),
            phase: "ready".into(),
            downloaded: 0,
            total: 0,
            detail: format!("Already installed — {found}"),
        });
        return Ok(ToolStatus {
            name: "cloudflared".into(),
            path: Some(found.clone()),
            downloaded: false,
            ok: true,
            detail: found,
        });
    }

    let dir = paths::bin_dir();
    std::fs::create_dir_all(&dir)?;
    let dest = paths::managed_cloudflared();
    let archive = dir.join("cloudflared-download.tgz");
    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-{}.tgz",
        arch()
    );

    download_with_progress(&url, &archive, "cloudflared", report)?;

    let out = Command::new("/usr/bin/tar")
        .arg("-xzf").arg(&archive).arg("-C").arg(&dir)
        .output()?;
    let _ = std::fs::remove_file(&archive);
    if !out.status.success() {
        anyhow::bail!("Could not unpack cloudflared: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    if !dest.exists() {
        anyhow::bail!("Download did not contain a cloudflared binary");
    }

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
    if !verify_runs(&dest, "--version") {
        let _ = std::fs::remove_file(&dest);
        anyhow::bail!("The downloaded cloudflared would not run");
    }

    report(Progress {
        tool: "cloudflared".into(),
        phase: "ready".into(),
        downloaded: 0,
        total: 0,
        detail: "Installed".into(),
    });
    Ok(ToolStatus {
        name: "cloudflared".into(),
        path: Some(dest.to_string_lossy().into_owned()),
        downloaded: true,
        ok: true,
        detail: dest.to_string_lossy().into_owned(),
    })
}

/// dnsmasq is genuinely optional.
///
/// It only buys wildcard TLD resolution (`*.test`). Every domain works without
/// it through the /etc/hosts helper, which needs no password after setup — so
/// its absence costs a convenience, not a feature. There is no official macOS
/// binary release, so we never try to fetch it.
pub fn check_dnsmasq() -> ToolStatus {
    match paths::dnsmasq_bin() {
        Some(p) => ToolStatus {
            name: "dnsmasq".into(),
            path: Some(p.clone()),
            downloaded: false,
            ok: true,
            detail: format!("Found — wildcard *.test enabled ({p})"),
        },
        None => ToolStatus {
            name: "dnsmasq".into(),
            path: None,
            downloaded: false,
            ok: true,
            // Kept short: this is shown in a single-line checklist row.
            detail: "Optional — only needed for wildcard *.test names".into(),
        },
    }
}

/// What is present right now, without installing anything.
///
/// The onboarding screen needs this before the user commits: showing three
/// rows reading "Waiting…" tells them nothing about their own machine.
pub fn status() -> Vec<ToolStatus> {
    let mut out = Vec::new();

    for (name, needed) in [("caddy", true), ("cloudflared", true)] {
        let path = if name == "cloudflared" {
            crate::tunnel::cloudflared_bin()
        } else {
            paths::find_bin(name)
        };
        out.push(ToolStatus {
            name: name.to_string(),
            path: path.clone(),
            downloaded: false,
            ok: path.is_some() || !needed,
            detail: match &path {
                Some(p) => format!("Found — {p}"),
                None => "Will be downloaded".to_string(),
            },
        });
    }

    out.push(check_dnsmasq());
    out
}

/// Phase one of installation: make sure every tool is present.
pub fn provision() -> anyhow::Result<Vec<ToolStatus>> {
    provision_reporting(&no_progress)
}

pub fn provision_reporting(report: Reporter) -> anyhow::Result<Vec<ToolStatus>> {
    let caddy = ensure_caddy_reporting(false, report)?;
    let cf = ensure_cloudflared_reporting(false, report)?;
    let dns = check_dnsmasq();
    report(Progress {
        tool: "dnsmasq".into(),
        phase: if dns.path.is_some() { "ready".into() } else { "skipped".into() },
        downloaded: 0,
        total: 0,
        detail: dns.detail.clone(),
    });
    Ok(vec![caddy, cf, dns])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_maps_to_vendor_naming() {
        assert!(matches!(arch(), "arm64" | "amd64"));
    }

    #[test]
    fn dnsmasq_absence_is_never_an_error() {
        // Optional by design: its status is reported, never failed.
        assert!(check_dnsmasq().ok);
    }
}
