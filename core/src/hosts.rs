use crate::paths::HOSTS_FILE;

pub const BEGIN_MARK: &str = "# === Portico BEGIN — managed block, do not edit ===";
pub const END_MARK: &str = "# === Portico END ===";

/// The block we own inside /etc/hosts. Rewritten wholesale each time.
/// With nothing to map we emit nothing at all, so uninstalling the last
/// custom-TLD site leaves no trace behind.
///
/// `loopback` is the address pair from `Config::loopback`, which depends on
/// the mode: System mode maps to the forwarded address, not 127.0.0.1.
pub fn render_block(domains: &[String], loopback: (&str, &str)) -> String {
    if domains.is_empty() {
        return String::new();
    }
    let mut s = String::from(BEGIN_MARK);
    s.push('\n');
    for d in domains {
        s.push_str(&format!("{}\t{d}\n", loopback.0));
        s.push_str(&format!("{}\t\t{d}\n", loopback.1));
    }
    s.push_str(END_MARK);
    s.push('\n');
    s
}

/// The managed block exactly as it currently appears in /etc/hosts.
pub fn current_block() -> String {
    let content = std::fs::read_to_string(HOSTS_FILE).unwrap_or_default();
    let mut out = String::new();
    let mut inside = false;
    for line in content.lines() {
        if line.starts_with(BEGIN_MARK) {
            inside = true;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
        if inside && line.starts_with(END_MARK) {
            break;
        }
    }
    out
}

/// Does /etc/hosts need rewriting?
///
/// Comparing the whole block, rather than only looking for absent domains,
/// is what makes removals take effect — otherwise deleting a custom-TLD site
/// would leave its mapping behind and keep shadowing the real domain.
pub fn needs_sync(domains: &[String], loopback: (&str, &str)) -> bool {
    current_block() != render_block(domains, loopback)
}

/// Ask the root helper to make /etc/hosts match this list.
///
/// Writing the file is the whole trigger: launchd watches it and runs the
/// helper, so no password is involved once setup has installed it.
pub fn write_request(domains: &[String]) -> std::io::Result<()> {
    let json = serde_json::json!({ "domains": domains });
    std::fs::write(crate::paths::hosts_request(), json.to_string())
}

/// Is the root helper installed and registered with launchd?
pub fn watcher_installed() -> bool {
    std::path::Path::new(crate::paths::HOSTSD_BIN).exists()
        && std::path::Path::new(crate::paths::HOSTSD_PLIST).exists()
}

/// Wait for the helper to catch up after a request.
/// launchd still paces job starts, so allow generously more time than the
/// helper itself needs.
pub fn wait_for_sync(domains: &[String], loopback: (&str, &str), millis: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(millis);
    while std::time::Instant::now() < deadline {
        if !needs_sync(domains, loopback) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    !needs_sync(domains, loopback)
}

/// Which of these domains are missing from /etc/hosts right now.
pub fn missing(domains: &[String]) -> Vec<String> {
    let content = std::fs::read_to_string(HOSTS_FILE).unwrap_or_default();
    let mapped: Vec<&str> = content
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .filter(|l| !l.trim().is_empty())
        .collect();

    domains
        .iter()
        .filter(|d| {
            !mapped.iter().any(|line| {
                line.split_whitespace().skip(1).any(|name| name == d.as_str())
            })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDALONE: (&str, &str) = ("127.0.0.1", "::1");
    const SYSTEM: (&str, &str) = (crate::config::LOOPBACK_V4, crate::config::LOOPBACK_V6);

    #[test]
    fn block_contains_both_stacks() {
        let b = render_block(&["mywebsite.io".to_string()], STANDALONE);
        assert!(b.contains("127.0.0.1\tmywebsite.io"));
        assert!(b.contains("::1\t\tmywebsite.io"));
        assert!(b.starts_with(BEGIN_MARK));
        assert!(b.trim_end().ends_with(END_MARK));
    }

    #[test]
    fn empty_list_renders_nothing() {
        assert_eq!(render_block(&[], SYSTEM), "");
    }

    #[test]
    fn system_mode_maps_to_the_forwarded_address() {
        let b = render_block(&["app.io".to_string()], SYSTEM);
        assert!(b.contains(&format!("{}\tapp.io", crate::config::LOOPBACK_V4)));
        assert!(b.contains(&format!("{}\t\tapp.io", crate::config::LOOPBACK_V6)));
        assert!(!b.contains("127.0.0.1"), "127.0.0.1:443 is not ours to answer on:\n{b}");
    }

    #[test]
    fn the_root_helper_writes_the_same_addresses_as_core() {
        // hostsd is a separate crate with no dependency on core, so it spells
        // the addresses out itself. If the two ever differ, the doctor reports
        // the hosts block stale forever and every edit waits 12s then fails.
        let helper = include_str!("../../hostsd/src/main.rs");
        assert!(helper.contains(&format!("\"{}\"", crate::config::LOOPBACK_V4)));
        assert!(helper.contains(&format!("\"{}\"", crate::config::LOOPBACK_V6)));
    }

    #[test]
    fn sync_is_needed_when_the_block_would_change() {
        // A machine with no managed block yet needs no sync for an empty list...
        let none: Vec<String> = vec![];
        assert_eq!(needs_sync(&none, SYSTEM), !current_block().is_empty());
        // ...but always needs one to add a domain that is not there.
        assert!(needs_sync(&["definitely-not-mapped.example".to_string()], SYSTEM));
    }

    #[test]
    fn localhost_is_already_present_on_every_mac() {
        assert!(missing(&["localhost".to_string()]).is_empty());
    }
}
