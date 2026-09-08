//! Portico's /etc/hosts helper.
//!
//! Runs as root, launched by launchd whenever the request file changes, does
//! exactly one job, and exits. There is no persistent root process, no sudoers
//! entry and no setuid bit.
//!
//! # Threat model
//!
//! The file that triggers this runs is writable by the logged-in user, so treat
//! its contents as hostile input. Everything here is built around that:
//!
//!   * the only thing ever written is `127.0.0.1`/`::1` mappings, between our
//!     own markers — the rest of /etc/hosts is copied through untouched;
//!   * every hostname is validated against a strict allowlist before use, so
//!     no whitespace, newline or comment character can break out of a line and
//!     forge an entry;
//!   * the count is capped, and the file is replaced atomically.
//!
//! The worst a hostile local process can achieve is pointing a hostname at
//! loopback — which is the feature being asked for. It cannot obtain root
//! code execution, and it cannot alter any line it did not create.

use std::io::Write;

const HOSTS: &str = "/etc/hosts";
const TMP: &str = "/etc/.portico-hosts.tmp";
const BEGIN_MARK: &str = "# === Portico BEGIN — managed block, do not edit ===";
const END_MARK: &str = "# === Portico END ===";

/// Generous enough for any real workflow, small enough that a runaway or
/// malicious writer cannot bloat /etc/hosts without bound.
const MAX_DOMAINS: usize = 200;

fn main() {
    let request = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: portico-hostsd <request-file>");
            std::process::exit(2);
        }
    };

    let domains = match read_request(&request) {
        Ok(d) => d,
        Err(e) => {
            // A malformed request must never damage /etc/hosts.
            eprintln!("portico-hostsd: ignoring request: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = sync(&domains) {
        eprintln!("portico-hostsd: {e}");
        std::process::exit(1);
    }

    flush_dns();
    eprintln!("portico-hostsd: {} domain(s) mapped", domains.len());
}

fn read_request(path: &str) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("not valid JSON: {e}"))?;
    let list = json
        .get("domains")
        .and_then(|v| v.as_array())
        .ok_or("missing \"domains\" array")?;

    if list.len() > MAX_DOMAINS {
        return Err(format!("too many domains ({} > {MAX_DOMAINS})", list.len()));
    }

    let mut out = Vec::new();
    for v in list {
        let d = v.as_str().ok_or("domain entry is not a string")?;
        if is_valid_hostname(d) {
            out.push(d.to_string());
        } else {
            eprintln!("portico-hostsd: rejecting invalid hostname {d:?}");
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Strict allowlist. Anything that could alter the meaning of a hosts line —
/// whitespace, `#`, control characters, a second field — fails here.
fn is_valid_hostname(d: &str) -> bool {
    if d.is_empty() || d.len() > 253 || !d.contains('.') {
        return false;
    }
    if d.starts_with('.') || d.ends_with('.') {
        return false;
    }
    d.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

fn sync(domains: &[String]) -> Result<(), String> {
    let current = std::fs::read_to_string(HOSTS).map_err(|e| format!("cannot read hosts: {e}"))?;

    let mut out = String::new();
    let mut inside = false;
    for line in current.lines() {
        if line.starts_with(BEGIN_MARK) {
            inside = true;
            continue;
        }
        if inside {
            if line.starts_with(END_MARK) {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    if !domains.is_empty() {
        out.push_str(BEGIN_MARK);
        out.push('\n');
        for d in domains {
            out.push_str(&format!("127.0.0.1\t{d}\n"));
            out.push_str(&format!("::1\t\t{d}\n"));
        }
        out.push_str(END_MARK);
        out.push('\n');
    }

    if out == current {
        return Ok(()); // nothing to do
    }

    // Replace atomically so a crash can never leave a truncated hosts file.
    {
        let mut f = std::fs::File::create(TMP).map_err(|e| format!("cannot create temp: {e}"))?;
        f.write_all(out.as_bytes())
            .map_err(|e| format!("cannot write temp: {e}"))?;
        f.sync_all().ok();
    }
    set_mode(TMP, 0o644)?;
    std::fs::rename(TMP, HOSTS).map_err(|e| format!("cannot replace hosts: {e}"))?;
    Ok(())
}

fn set_mode(path: &str, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("cannot set permissions: {e}"))
}

fn flush_dns() {
    let _ = std::process::Command::new("/usr/bin/dscacheutil")
        .arg("-flushcache")
        .status();
    let _ = std::process::Command::new("/usr/bin/killall")
        .args(["-HUP", "mDNSResponder"])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_hostnames() {
        assert!(is_valid_hostname("app.io"));
        assert!(is_valid_hostname("api.staging.my-app.test"));
        assert!(is_valid_hostname("a1.b2"));
    }

    #[test]
    fn rejects_anything_that_could_forge_a_second_entry() {
        // The whole point: no input may introduce a line or field break.
        assert!(!is_valid_hostname("evil.com\n127.0.0.1\tbank.example"));
        assert!(!is_valid_hostname("evil.com bank.example"));
        assert!(!is_valid_hostname("evil.com\tbank.example"));
        assert!(!is_valid_hostname("evil.com#bank.example"));
        assert!(!is_valid_hostname("evil.com\r"));
        assert!(!is_valid_hostname("evil.com\0"));
    }

    #[test]
    fn rejects_malformed_names() {
        assert!(!is_valid_hostname(""));
        assert!(!is_valid_hostname("nodot"));
        assert!(!is_valid_hostname(".leading.dot"));
        assert!(!is_valid_hostname("trailing.dot."));
        assert!(!is_valid_hostname("double..dot"));
        assert!(!is_valid_hostname("-lead.hyphen"));
        assert!(!is_valid_hostname("trail-.hyphen"));
        assert!(!is_valid_hostname("UPPER.CASE"));
        assert!(!is_valid_hostname("uni\u{00e7}ode.test"));
        assert!(!is_valid_hostname(&format!("{}.test", "a".repeat(64))));
        assert!(!is_valid_hostname(&format!("{}.test", "a.".repeat(200))));
    }

    #[test]
    fn request_parsing_filters_and_dedupes() {
        let dir = std::env::temp_dir().join("portico-hostsd-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("req.json");
        std::fs::write(
            &path,
            r#"{"domains":["b.test","a.test","a.test","not valid","NOPE.test"]}"#,
        )
        .unwrap();
        let got = read_request(path.to_str().unwrap()).unwrap();
        assert_eq!(got, vec!["a.test".to_string(), "b.test".to_string()]);
    }

    #[test]
    fn rejects_oversized_and_malformed_requests() {
        let dir = std::env::temp_dir().join("portico-hostsd-test");
        std::fs::create_dir_all(&dir).unwrap();

        let bad = dir.join("bad.json");
        std::fs::write(&bad, "not json at all").unwrap();
        assert!(read_request(bad.to_str().unwrap()).is_err());

        let noarr = dir.join("noarr.json");
        std::fs::write(&noarr, r#"{"nope":1}"#).unwrap();
        assert!(read_request(noarr.to_str().unwrap()).is_err());

        let huge = dir.join("huge.json");
        let many: Vec<String> = (0..MAX_DOMAINS + 1).map(|i| format!("\"h{i}.test\"")).collect();
        std::fs::write(&huge, format!(r#"{{"domains":[{}]}}"#, many.join(","))).unwrap();
        assert!(read_request(huge.to_str().unwrap()).is_err());
    }
}
