//! Cloudflare API, enough of it to own a named tunnel end to end.
//!
//! A quick tunnel is anonymous and free, and its hostname changes on every
//! restart. That is fine for a one-off test and useless for a webhook you
//! registered with Stripe last week. A *named* tunnel fixes exactly that: a
//! hostname you choose, on a domain you own, that survives restarts.
//!
//! The price is an API token and a zone. This module does the four things
//! that buys: prove the token works, create the tunnel, point DNS at it, and
//! take both away again cleanly.
//!
//! Requests go through curl, like every other network call in this crate, so
//! the dependency list stays at four. The token is handed to curl over stdin
//! in a config file — never as an argument, which `ps` would show to any
//! local process.

use crate::paths;
use serde::Serialize;
use std::io::Write;
use std::process::{Command, Stdio};

const API: &str = "https://api.cloudflare.com/client/v4";

/// A zone the token can see — one domain the user actually owns.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Zone {
    pub id: String,
    pub name: String,
}

/// What we know after a token checks out.
#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub zones: Vec<Zone>,
}

/// A named tunnel, plus the secret needed to run it.
#[derive(Debug, Clone)]
pub struct Tunnel {
    pub id: String,
    pub name: String,
    /// Base64, as it appears in the credentials file. Only known at creation.
    pub secret: String,
}

/// Base64 of 32 bytes from the kernel, which is what a tunnel secret is.
///
/// Written out rather than pulled in: one dependency for twenty lines of
/// table lookup is a poor trade in a crate that has four.
fn base64(bytes: &[u8]) -> String {
    const SET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(SET[(n >> 18 & 63) as usize] as char);
        out.push(SET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { SET[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { SET[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn random_secret() -> anyhow::Result<String> {
    // Read exactly 32 bytes: /dev/urandom never reaches EOF, so anything that
    // reads "the whole file" would hang forever.
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(base64(&buf))
}

/// One API call. Returns the `result` member, or the API's own error text.
fn api(
    token: &str,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> anyhow::Result<serde_json::Value> {
    if !crate::secrets::plausible(token) {
        anyhow::bail!("The stored Cloudflare token is not a valid token");
    }

    let url = format!("{API}{path}");
    let mut args: Vec<String> = vec![
        "-sS".into(),
        "--max-time".into(),
        "30".into(),
        "-X".into(),
        method.into(),
        "-H".into(),
        "Content-Type: application/json".into(),
        // The Authorization header arrives on stdin, below.
        "--config".into(),
        "-".into(),
    ];

    // The body goes through a 0600 file in our own 0700 directory. It is not
    // secret, but it can contain characters that curl's config format would
    // have to escape, and a file sidesteps that entirely.
    let body_file = body.map(|_| paths::tmp_dir().join(format!("cf-{}.json", std::process::id())));
    if let (Some(b), Some(f)) = (body, body_file.as_ref()) {
        paths::ensure_dirs()?;
        std::fs::write(f, serde_json::to_string(b)?)?;
        args.push("-d".into());
        args.push(format!("@{}", f.display()));
    }
    args.push(url);

    let mut child = Command::new("/usr/bin/curl")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let stdin = child.stdin.as_mut().expect("stdin piped");
        writeln!(stdin, "header = \"Authorization: Bearer {token}\"")?;
    }
    let out = child.wait_with_output()?;

    if let Some(f) = body_file.as_ref() {
        let _ = std::fs::remove_file(f);
    }

    if !out.status.success() {
        anyhow::bail!(
            "Could not reach the Cloudflare API: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| anyhow::anyhow!("Cloudflare returned something that was not JSON"))?;

    if json.get("success").and_then(|s| s.as_bool()) != Some(true) {
        anyhow::bail!("{}", error_text(&json));
    }
    Ok(json.get("result").cloned().unwrap_or(serde_json::Value::Null))
}

/// Turn Cloudflare's error array into one sentence a person can act on.
fn error_text(json: &serde_json::Value) -> String {
    let msgs: Vec<String> = json
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if msgs.is_empty() {
        "Cloudflare rejected the request".to_string()
    } else {
        msgs.join("; ")
    }
}

/// Prove the token works and collect what it can reach.
///
/// A token is useless to us without both halves: an account to hang the
/// tunnel on, and at least one zone to put a hostname in. Reporting that here
/// means the settings screen can say what is missing instead of failing later
/// when a site is turned public.
pub fn verify(token: &str) -> anyhow::Result<Account> {
    let accounts = api(token, "GET", "/accounts", None)?;
    let first = accounts
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow::anyhow!("That token cannot see any account"))?;

    let id = first
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Cloudflare returned an account with no id"))?
        .to_string();
    let name = first
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("your account")
        .to_string();

    Ok(Account { id, name, zones: zones(token)? })
}

pub fn zones(token: &str) -> anyhow::Result<Vec<Zone>> {
    let result = api(token, "GET", "/zones?per_page=50", None)?;
    Ok(result
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|z| {
                    Some(Zone {
                        id: z.get("id")?.as_str()?.to_string(),
                        name: z.get("name")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The zone a hostname belongs to, longest match first.
///
/// `hooks.api.example.com` matches the `example.com` zone, but if the account
/// also holds `api.example.com` that is the more specific — and correct — one.
pub fn zone_for<'a>(zones: &'a [Zone], hostname: &str) -> Option<&'a Zone> {
    let host = hostname.trim().trim_end_matches('.').to_ascii_lowercase();
    zones
        .iter()
        .filter(|z| {
            let n = z.name.to_ascii_lowercase();
            host == n || host.ends_with(&format!(".{n}"))
        })
        .max_by_key(|z| z.name.len())
}

/// Create a named tunnel, or bail if one already exists under that name.
pub fn create_tunnel(token: &str, account_id: &str, name: &str) -> anyhow::Result<Tunnel> {
    let secret = random_secret()?;
    let body = serde_json::json!({
        "name": name,
        "tunnel_secret": secret,
        // We write the ingress rules ourselves, next to the credentials.
        "config_src": "local",
    });

    let result = api(token, "POST", &format!("/accounts/{account_id}/cfd_tunnel"), Some(&body))?;
    let id = result
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Cloudflare created a tunnel with no id"))?
        .to_string();

    Ok(Tunnel { id, name: name.to_string(), secret })
}

/// An existing tunnel with this name, if there is one.
///
/// Deleted tunnels keep their name reserved until they are purged, so the
/// list is filtered to the live ones.
pub fn find_tunnel(token: &str, account_id: &str, name: &str) -> anyhow::Result<Option<String>> {
    let result = api(
        token,
        "GET",
        &format!("/accounts/{account_id}/cfd_tunnel?name={name}&is_deleted=false"),
        None,
    )?;
    Ok(result
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

pub fn delete_tunnel(token: &str, account_id: &str, tunnel_id: &str) -> anyhow::Result<()> {
    api(
        token,
        "DELETE",
        &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}"),
        None,
    )?;
    Ok(())
}

/// Point a hostname at a tunnel, replacing any record already there.
///
/// Proxied is not optional: `<id>.cfargotunnel.com` only resolves through
/// Cloudflare's edge, so an unproxied record would be a dead name.
pub fn upsert_dns(token: &str, zone_id: &str, hostname: &str, tunnel_id: &str) -> anyhow::Result<()> {
    let content = format!("{tunnel_id}.cfargotunnel.com");
    let body = serde_json::json!({
        "type": "CNAME",
        "name": hostname,
        "content": content,
        "proxied": true,
        "comment": "Managed by Portico",
    });

    match existing_record(token, zone_id, hostname)? {
        // Ours to move: it already points at a tunnel we made.
        Some(record) if record.ours => {
            let id = record.id;
            api(token, "PUT", &format!("/zones/{zone_id}/dns_records/{id}"), Some(&body))?;
        }
        // Somebody else's. Refuse.
        //
        // `delete_dns` already declines to remove a record it did not create,
        // and overwriting one is the worse half of that same mistake: the PUT
        // both replaces a live record with a tunnel to a laptop *and* stamps
        // it as Portico's, so clearing the hostname later would delete it
        // outright rather than put it back. The apex is the likely casualty,
        // since an empty first part in the interface means the domain itself.
        Some(_) => anyhow::bail!(
            "{hostname} already has a DNS record Portico did not create. \
             Pick another name, or remove that record in Cloudflare first."
        ),
        None => {
            api(token, "POST", &format!("/zones/{zone_id}/dns_records"), Some(&body))?;
        }
    }
    Ok(())
}

/// A DNS record already sitting on a hostname, and whether we put it there.
struct Existing {
    id: String,
    ours: bool,
}

/// Whether a record carries our marker. The comment is the only thing that
/// distinguishes a record Portico created from one that was already there.
fn is_ours(record: &serde_json::Value) -> bool {
    record
        .get("comment")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.contains("Portico"))
}

fn existing_record(
    token: &str,
    zone_id: &str,
    hostname: &str,
) -> anyhow::Result<Option<Existing>> {
    let result = api(
        token,
        "GET",
        &format!("/zones/{zone_id}/dns_records?name={hostname}"),
        None,
    )?;
    let Some(record) = result.as_array().and_then(|a| a.first()) else {
        return Ok(None);
    };
    let Some(id) = record.get("id").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    Ok(Some(Existing { id: id.to_string(), ours: is_ours(record) }))
}

/// Remove the DNS record for a hostname, but only if Portico created it.
///
/// A record we did not write may be someone's real production CNAME that
/// happens to share the name. Deleting that to tidy up after ourselves would
/// be far worse than leaving a stale record behind.
pub fn delete_dns(token: &str, zone_id: &str, hostname: &str) -> anyhow::Result<()> {
    let result = api(
        token,
        "GET",
        &format!("/zones/{zone_id}/dns_records?name={hostname}"),
        None,
    )?;
    let Some(record) = result.as_array().and_then(|a| a.first()) else {
        return Ok(());
    };

    if !is_ours(record) {
        return Ok(());
    }

    if let Some(id) = record.get("id").and_then(|v| v.as_str()) {
        api(token, "DELETE", &format!("/zones/{zone_id}/dns_records/{id}"), None)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zones() -> Vec<Zone> {
        vec![
            Zone { id: "z1".into(), name: "example.com".into() },
            Zone { id: "z2".into(), name: "api.example.com".into() },
            Zone { id: "z3".into(), name: "other.dev".into() },
        ]
    }

    #[test]
    fn picks_the_most_specific_zone() {
        // Both zones match; the longer one is the right owner.
        assert_eq!(zone_for(&zones(), "hooks.api.example.com").unwrap().id, "z2");
        assert_eq!(zone_for(&zones(), "hooks.example.com").unwrap().id, "z1");
    }

    #[test]
    fn matches_the_zone_apex_itself() {
        assert_eq!(zone_for(&zones(), "example.com").unwrap().id, "z1");
    }

    #[test]
    fn does_not_match_a_zone_that_is_merely_a_suffix_of_the_label() {
        // "notexample.com" ends with "example.com" as a *string* but is a
        // different domain entirely. Only a label boundary counts.
        assert!(zone_for(&zones(), "notexample.com").is_none());
        assert!(zone_for(&zones(), "unrelated.io").is_none());
    }

    #[test]
    fn zone_matching_ignores_case_and_a_trailing_dot() {
        assert_eq!(zone_for(&zones(), "Hooks.Example.COM.").unwrap().id, "z1");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_tunnel_secret_is_32_bytes_of_base64() {
        let s = random_secret().expect("urandom readable");
        assert_eq!(s.len(), 44, "32 bytes encodes to 44 base64 characters");
        assert_ne!(s, random_secret().unwrap(), "must not be deterministic");
    }

    #[test]
    fn error_text_reads_back_what_cloudflare_said() {
        let json = serde_json::json!({
            "success": false,
            "errors": [{ "code": 1004, "message": "DNS name is invalid" }]
        });
        assert_eq!(error_text(&json), "DNS name is invalid");
    }

    #[test]
    fn a_record_portico_did_not_create_is_never_treated_as_ours() {
        // The comment is the only marker separating a record we made from a
        // live one that was already there. Overwriting the latter would point
        // a production hostname at a laptop, and — because the write stamps
        // it as ours — clearing the hostname later would then delete it.
        let mine = serde_json::json!({ "id": "r1", "comment": "Managed by Portico" });
        let theirs = serde_json::json!({ "id": "r2", "comment": "prod www" });
        let bare = serde_json::json!({ "id": "r3" });

        assert!(is_ours(&mine));
        assert!(!is_ours(&theirs), "someone else's record must not be adopted");
        assert!(!is_ours(&bare), "a record with no comment is not ours");
    }
}
