//! Serving a site to other devices on the same network.
//!
//! The local half of Portico is deliberately loopback-only: `shop.test`
//! resolves on this Mac and nowhere else. That is the right default, and it
//! is also the thing standing between you and testing on your phone.
//!
//! Exposing a site to the LAN has one obstacle that is not about listening
//! sockets at all — **the other device cannot resolve the name**. Our dnsmasq
//! answers on 127.0.0.1 and hands back 127.0.0.1; a phone can neither reach it
//! nor use its answer. Publishing the name over mDNS or asking every device to
//! change its DNS both work, and both are a setup step per device.
//!
//! So a LAN-exposed site gets its own port on every interface instead, and the
//! address we hand the user is a bare `http://192.168.1.5:8801`. No DNS, no
//! certificate to install, nothing to configure on the phone. It is uglier
//! than a hostname and it works on everything, which is the better trade for
//! "open this on my phone for two minutes".
//!
//! HTTP, not HTTPS: the certificate is signed by a CA that exists only in this
//! Mac's trust store, so every other device would show a warning. A padlock
//! that has to be explained is worse than no padlock.

use crate::config::{Config, Site};

/// Ports handed out to LAN-exposed sites. Above the registered range and
/// clear of the loopback twin on 8880.
pub const LAN_PORT_FIRST: u16 = 8801;
pub const LAN_PORT_LAST: u16 = 8879;

/// This Mac's address on the network it is actually using.
///
/// Asked of the routing table rather than guessed: `en0` is Wi-Fi on a laptop
/// but often Ethernet on a desktop, and a Mac on both has two answers of which
/// only one carries the default route.
pub fn address() -> Option<String> {
    let iface = std::process::Command::new("/sbin/route")
        .args(["-n", "get", "default"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.trim().strip_prefix("interface:").map(|v| v.trim().to_string()))
        })?;

    let out = std::process::Command::new("/usr/sbin/ipconfig")
        .args(["getifaddr", &iface])
        .output()
        .ok()?;
    let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!ip.is_empty()).then_some(ip)
}

/// The lowest LAN port no other site has taken.
///
/// Reusing the port a site already holds keeps its URL stable, which matters:
/// the whole point is a link you can leave open on a phone.
pub fn assign_port(cfg: &Config, site_id: &str) -> Option<u16> {
    if let Some(existing) = cfg.find(site_id).and_then(|s| s.lan_port) {
        return Some(existing);
    }
    let taken: Vec<u16> = cfg
        .sites
        .iter()
        .filter(|s| s.id != site_id)
        .filter_map(|s| s.lan_port)
        .collect();
    (LAN_PORT_FIRST..=LAN_PORT_LAST).find(|p| !taken.contains(p))
}

/// The address to hand someone on the same network, if this site has one.
pub fn url(site: &Site) -> Option<String> {
    let port = site.lan_port.filter(|_| site.lan)?;
    let host = address()?;
    Some(format!("http://{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::Target;

    fn site(id: &str, lan_port: Option<u16>) -> Site {
        Site {
            id: id.into(),
            domain: format!("{id}.test"),
            target: Target::Proxy { upstream: "127.0.0.1:1".into() },
            ssl: true,
            spa: false,
            tunnel: false,
            public_domain: String::new(),
            tunnel_id: None,
            lan: false,
            lan_port,
            run: false,
        }
    }

    #[test]
    fn a_site_keeps_the_port_it_already_had() {
        // The URL is meant to be left open on a phone. Reassigning it on every
        // toggle would break that link for no reason.
        let mut cfg = Config::default();
        cfg.sites.push(site("a", Some(8805)));
        assert_eq!(assign_port(&cfg, "a"), Some(8805));
    }

    #[test]
    fn ports_are_not_handed_out_twice() {
        let mut cfg = Config::default();
        cfg.sites.push(site("a", Some(8801)));
        cfg.sites.push(site("b", Some(8802)));
        cfg.sites.push(site("c", None));
        assert_eq!(assign_port(&cfg, "c"), Some(8803));
    }

    #[test]
    fn the_lowest_free_port_is_reused_after_a_gap() {
        let mut cfg = Config::default();
        cfg.sites.push(site("a", Some(8802)));
        cfg.sites.push(site("b", None));
        assert_eq!(assign_port(&cfg, "b"), Some(8801), "a freed low port should come back");
    }

    #[test]
    fn running_out_of_ports_is_reported_rather_than_wrapping() {
        let mut cfg = Config::default();
        for (i, p) in (LAN_PORT_FIRST..=LAN_PORT_LAST).enumerate() {
            cfg.sites.push(site(&format!("s{i}"), Some(p)));
        }
        cfg.sites.push(site("one-too-many", None));
        assert_eq!(assign_port(&cfg, "one-too-many"), None);
    }

    #[test]
    fn no_url_until_the_site_is_actually_exposed() {
        let mut s = site("a", Some(8801));
        s.lan = false;
        assert!(url(&s).is_none(), "a port alone must not read as exposed");
    }
}
