//! Supervises dev servers that Portico starts on the user's behalf.
//!
//! Node has no equivalent of PHP-FPM — nothing is listening until someone runs
//! `npm run dev` — so for a path-only Node site we run it ourselves and proxy
//! to the port we assigned.
//!
//! Two details do most of the work here:
//!
//!   * the command runs through a **login shell**, because a GUI app inherits a
//!     minimal PATH and `nvm` lives in the user's shell profile. Spawning
//!     `npm` directly would fail with "command not found" on any nvm machine;
//!   * the child gets its own **process group**, because `npm run dev` spawns
//!     the real server as a grandchild. Killing only the shell would orphan the
//!     server, leaving the port held by a process nothing tracks.

use crate::paths;
use serde::Serialize;
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunState {
    pub status: RunStatus,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

impl RunState {
    fn stopped() -> Self {
        RunState { status: RunStatus::Stopped, pid: None, error: None }
    }
}

struct Handle {
    child: Child,
    state: Arc<Mutex<RunState>>,
}

fn registry() -> &'static Mutex<HashMap<String, Handle>> {
    static REG: OnceLock<Mutex<HashMap<String, Handle>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A port nothing is currently listening on, searched from a predictable base
/// so a project tends to keep the same port between runs.
pub fn free_port(from: u16) -> Option<u16> {
    (from..from.saturating_add(400)).find(|p| !port_open(*p))
}

pub fn port_open(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// Read the `.env` files a JS framework would load, in the same precedence
/// order Next, Vite and CRA use (lowest first, so later files win).
///
/// Frameworks load these for you, but a custom `server.mjs` that reads
/// `process.env` at import time runs before any of that — so a project can
/// depend on `.env.local` while nothing in it ever loads the file. Doing it
/// here is what makes "just point at the project" actually work.
pub fn load_env_files(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for name in [".env", ".env.development", ".env.local", ".env.development.local"] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            continue;
        };
        for (k, v) in parse_env(&text) {
            out.retain(|(existing, _)| existing != &k);
            out.push((k, v));
        }
    }
    out
}

/// A deliberately small .env parser: `KEY=VALUE`, optional `export`, optional
/// quotes, `#` comments.
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // A shell variable name may not start with a digit.
        let valid = !key.is_empty()
            && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            continue;
        }
        let value = value.trim();

        let value = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            inner.replace("\\n", "\n").replace("\\\"", "\"")
        } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            inner.to_string()
        } else {
            // Unquoted: a ` #` begins a trailing comment.
            match value.find(" #") {
                Some(i) => value[..i].trim().to_string(),
                None => value.to_string(),
            }
        };
        out.retain(|(k, _): &(String, String)| k != key);
        out.push((key.to_string(), value));
    }
    out
}

/// The shell to run dev commands through, so profile-managed tools (nvm, asdf,
/// volta, homebrew paths) are on PATH exactly as in the user's terminal.
fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

pub fn start(
    site_id: &str,
    dir: &str,
    command: &str,
    port: u16,
    log_path: &std::path::Path,
) -> anyhow::Result<()> {
    stop(site_id);

    if !std::path::Path::new(dir).is_dir() {
        anyhow::bail!("Project folder is gone: {dir}");
    }

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Truncate per run: a log that only covers the current attempt is the one
    // worth showing when a start fails.
    let log = std::fs::File::create(log_path)?;
    let log_err = log.try_clone()?;

    let state = Arc::new(Mutex::new(RunState {
        status: RunStatus::Starting,
        pid: None,
        error: None,
    }));

    let mut cmd = Command::new(login_shell());
    cmd.args(["-lic", command]).current_dir(dir);

    // Project .env files first, but never clobbering a variable the user
    // already has set in their real environment.
    for (k, v) in load_env_files(std::path::Path::new(dir)) {
        if std::env::var_os(&k).is_none() {
            cmd.env(k, v);
        }
    }

    // Ours win outright: PORT is the port Caddy proxies to, so a stale value
    // in a .env file must not override it.
    cmd.env("PORT", port.to_string())
        .env("BROWSER", "none") // stop CRA and friends opening a tab
        .env("FORCE_COLOR", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    // Own process group, so stopping takes the whole tree with it.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let child = cmd.spawn()?;
    let pid = child.id();
    state.lock().unwrap().pid = Some(pid);

    // Record it on disk. setsid() made the child its own group leader, so its
    // pid doubles as the process-group id — which is what stop() signals.
    write_pid_file(site_id, pid, port);

    // Wait for the port rather than for the process: a dev server is only
    // usable once it is actually listening, and a cold build can take a while.
    {
        let state = Arc::clone(&state);
        let log_path = log_path.to_path_buf();
        let id = site_id.to_string();
        std::thread::spawn(move || {
            for _ in 0..240 {
                std::thread::sleep(std::time::Duration::from_millis(500));

                if port_open(port) {
                    let mut s = state.lock().unwrap();
                    if s.status == RunStatus::Starting {
                        s.status = RunStatus::Running;
                    }
                    return;
                }

                // Give up early if the process died.
                let alive = registry()
                    .lock()
                    .unwrap()
                    .get(&id)
                    .map(|h| unsafe { libc::kill(h.child.id() as i32, 0) == 0 })
                    .unwrap_or(false);
                if !alive {
                    let mut s = state.lock().unwrap();
                    s.status = RunStatus::Failed;
                    s.error = Some(tail(&log_path, 6));
                    return;
                }
            }
            let mut s = state.lock().unwrap();
            if s.status == RunStatus::Starting {
                s.status = RunStatus::Failed;
                s.error = Some(format!(
                    "Timed out waiting for port {port}.\n{}",
                    tail(&log_path, 6)
                ));
            }
        });
    }

    registry()
        .lock()
        .unwrap()
        .insert(site_id.to_string(), Handle { child, state });
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct Recorded {
    pgid: i32,
    port: u16,
}

fn write_pid_file(site_id: &str, pid: u32, port: u16) {
    let _ = paths::ensure_dirs();
    let json = serde_json::json!({ "pgid": pid, "port": port });
    let _ = std::fs::write(paths::pid_file(site_id), json.to_string());
}

fn read_pid_file(site_id: &str) -> Option<Recorded> {
    let raw = std::fs::read_to_string(paths::pid_file(site_id)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(Recorded {
        pgid: v.get("pgid")?.as_u64()? as i32,
        port: v.get("port")?.as_u64()? as u16,
    })
}

fn clear_pid_file(site_id: &str) {
    let _ = std::fs::remove_file(paths::pid_file(site_id));
}

fn alive(pgid: i32) -> bool {
    pgid > 0 && unsafe { libc::kill(pgid, 0) } == 0
}

/// Is a dev server for this site already up, started by some earlier run?
pub fn adopted(site_id: &str) -> Option<u16> {
    let rec = read_pid_file(site_id)?;
    if alive(rec.pgid) {
        Some(rec.port)
    } else {
        clear_pid_file(site_id);
        None
    }
}

fn terminate(pgid: i32) {
    unsafe {
        // Negative pid signals the whole group: `npm run dev` runs the real
        // server as a grandchild, and killing only the shell orphans it.
        libc::kill(-pgid, libc::SIGTERM);
    }
    for _ in 0..20 {
        if !alive(pgid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

pub fn stop(site_id: &str) {
    // Whether this process started it or a previous CLI run did, the pid file
    // is the source of truth.
    let recorded = read_pid_file(site_id).map(|r| r.pgid);
    if let Some(mut h) = registry().lock().unwrap().remove(site_id) {
        terminate(h.child.id() as i32);
        let _ = h.child.wait();
    } else if let Some(pgid) = recorded {
        if alive(pgid) {
            terminate(pgid);
        }
    }
    clear_pid_file(site_id);
}

pub fn stop_all() {
    let ids: Vec<String> = registry().lock().unwrap().keys().cloned().collect();
    for id in ids {
        stop(&id);
    }
}

pub fn status(site_id: &str) -> RunState {
    if let Some(h) = registry().lock().unwrap().get(site_id) {
        return h.state.lock().unwrap().clone();
    }
    // Started by an earlier run of the CLI or a previous app session.
    match read_pid_file(site_id) {
        Some(rec) if alive(rec.pgid) => RunState {
            status: if port_open(rec.port) { RunStatus::Running } else { RunStatus::Starting },
            pid: Some(rec.pgid as u32),
            error: None,
        },
        Some(_) => {
            clear_pid_file(site_id);
            RunState::stopped()
        }
        None => RunState::stopped(),
    }
}

pub fn tail(path: &std::path::Path, lines: usize) -> String {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = content.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_port_skips_ports_in_use() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = listener.local_addr().unwrap().port();
        assert!(port_open(taken));
        assert_ne!(free_port(taken), Some(taken));
    }

    #[test]
    fn unknown_site_is_stopped() {
        assert_eq!(status("nope").status, RunStatus::Stopped);
    }

    #[test]
    fn parses_ordinary_env_lines() {
        let got = parse_env("# comment\nexport A=1\nB = two \nC=\"quoted value\"\nD='single'\n\nE=with # comment\n");
        let map: std::collections::HashMap<_, _> = got.into_iter().collect();
        assert_eq!(map["A"], "1");
        assert_eq!(map["B"], "two");
        assert_eq!(map["C"], "quoted value");
        assert_eq!(map["D"], "single");
        assert_eq!(map["E"], "with");
    }

    #[test]
    fn keeps_hashes_that_are_part_of_a_value() {
        // Postgres passwords and fragments legitimately contain '#'.
        let got = parse_env("DATABASE_URL=postgres://u:p#1@localhost/db\n");
        assert_eq!(got[0].1, "postgres://u:p#1@localhost/db");
    }

    #[test]
    fn ignores_junk_lines() {
        assert!(parse_env("not an assignment\n1BAD=x\n=novalue\n").is_empty());
    }

    #[test]
    fn later_env_files_win() {
        let d = std::env::temp_dir().join("st-env-precedence");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".env"), "SHARED=base\nONLY_BASE=1\n").unwrap();
        std::fs::write(d.join(".env.local"), "SHARED=local\n").unwrap();
        let map: std::collections::HashMap<_, _> = load_env_files(&d).into_iter().collect();
        assert_eq!(map["SHARED"], "local");
        assert_eq!(map["ONLY_BASE"], "1");
    }

    #[test]
    fn tail_returns_the_last_lines() {
        let p = std::env::temp_dir().join("st-runner-tail.log");
        std::fs::write(&p, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(tail(&p, 2), "three\nfour");
    }
}
