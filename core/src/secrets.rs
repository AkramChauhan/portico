//! Credentials, kept in the login keychain rather than in config.json.
//!
//! Everything else Portico stores is a preference: losing it is an
//! inconvenience. A Cloudflare API token is different — it can create DNS
//! records and tunnels on a real account — and `config.json` is the wrong
//! place for it. That file is written 0644, protected only by the 0700
//! directory around it, and it is read wholesale by every `Config::load()`.
//!
//! The keychain gives us three things that matter: the token is encrypted at
//! rest, it never passes through a struct that gets serialised into a log or
//! an error message, and macOS decides who may read it.
//!
//! The token never appears in a command line. `security` puts every argument
//! in `argv`, which is world-readable through `ps`, so the value goes in over
//! stdin — twice, because the interactive `-w` form asks for confirmation.

use std::io::Write;
use std::process::{Command, Stdio};

/// One keychain service for the whole app; the account names the credential.
const SERVICE: &str = "io.portico";

/// Cloudflare API token, used to create named tunnels and their DNS records.
pub const CLOUDFLARE_TOKEN: &str = "cloudflare-api-token";

/// ngrok authtoken. Kept here rather than written into ngrok's own config so
/// that both providers are stored one way, and so the token never has to pass
/// through a command line to get there.
pub const NGROK_AUTHTOKEN: &str = "ngrok-authtoken";

/// Reject anything that is not plausibly a token before it reaches the
/// keychain or an HTTP header.
///
/// Cloudflare tokens are URL-safe base64. The check exists for the characters
/// it excludes: a newline would let a value forge a second header line in the
/// curl config we build, and a quote would break out of the quoted value.
pub fn plausible(secret: &str) -> bool {
    !secret.is_empty()
        && secret.len() <= 256
        && secret
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '+' | '='))
}

pub fn set(account: &str, secret: &str) -> anyhow::Result<()> {
    if !plausible(secret) {
        anyhow::bail!("That does not look like an API token");
    }

    let mut child = Command::new("/usr/bin/security")
        .args(["add-generic-password", "-s", SERVICE, "-a", account, "-U", "-w"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().expect("stdin piped");
        // Once for the value, once for the confirmation prompt.
        writeln!(stdin, "{secret}")?;
        writeln!(stdin, "{secret}")?;
    }

    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "Could not save to the keychain: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn get(account: &str) -> Option<String> {
    let out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Whether a credential is stored, without unlocking or reading it.
pub fn has(account: &str) -> bool {
    Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", SERVICE, "-a", account])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn delete(account: &str) {
    let _ = Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", account])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_anything_that_could_forge_a_header_line() {
        // The token is interpolated into a curl config file as a quoted
        // header value. A newline there would start a second directive, and a
        // quote would end the value early.
        assert!(!plausible("abc\ndef"));
        assert!(!plausible("abc\"def"));
        assert!(!plausible("abc def"));
        assert!(!plausible("abc\\def"));
        assert!(!plausible("abc\rdef"));
        assert!(!plausible(""));
        assert!(!plausible(&"a".repeat(257)));
    }

    #[test]
    fn accepts_a_real_token_shape() {
        // Cloudflare tokens are 40 URL-safe base64 characters.
        assert!(plausible("v1a2B3c4D5e6F7g8H9i0J1k2L3m4N5o6P7q8R9s0"));
        assert!(plausible("with-dashes_and_underscores.and~tildes"));
    }
}
