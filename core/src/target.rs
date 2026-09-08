use serde::{Deserialize, Serialize};

/// Where a vhost sends its traffic.
///
/// The UI takes one free-text field and we work out which kind it is, so the
/// user never has to answer "is this a port or a folder?" — the answer is
/// always obvious from what they typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    /// Reverse-proxy to something already listening locally.
    Proxy { upstream: String },
    /// Serve files straight off disk.
    Dir { path: String },
    /// A dev server Portico starts and supervises itself, then proxies
    /// to. The port is assigned here rather than by the user, which is what
    /// removes the "which project owns 3000?" bookkeeping.
    Node { dir: String, command: String, port: u16 },
    /// Hand .php files to PHP-FPM and serve the rest as files.
    ///
    /// This is why a Laravel project needs no port: PHP-FPM is already a
    /// long-running daemon, so Caddy talks FastCGI to it directly and nothing
    /// has to be started per project.
    Php { root: String },
}

impl Target {
    pub fn parse(raw: &str) -> Result<Target, String> {
        let s = raw.trim();
        if s.is_empty() {
            return Err("Target is empty".into());
        }

        // A path is anything that looks like one, or that exists on disk.
        if s.starts_with('/') || s.starts_with("~/") || s.starts_with("./") {
            let expanded = if let Some(rest) = s.strip_prefix("~/") {
                crate::paths::home().join(rest).to_string_lossy().into_owned()
            } else {
                std::fs::canonicalize(s)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| s.to_string())
            };
            if !std::path::Path::new(&expanded).is_dir() {
                return Err(format!("Folder does not exist: {expanded}"));
            }
            return Self::detect_dir(&expanded);
        }

        // Otherwise it's a port or a host:port, with or without a scheme.
        let stripped = s
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/');

        // Bare port, e.g. "8000"
        if let Ok(port) = stripped.parse::<u16>() {
            if port == 0 {
                return Err("Port must be between 1 and 65535".into());
            }
            return Ok(Target::Proxy { upstream: format!("127.0.0.1:{port}") });
        }

        // host:port, e.g. "localhost:8000"
        if let Some((host, port)) = stripped.rsplit_once(':') {
            if port.parse::<u16>().is_ok() && !host.is_empty() {
                let host = if host == "localhost" { "127.0.0.1" } else { host };
                return Ok(Target::Proxy { upstream: format!("{host}:{port}") });
            }
        }

        Err(format!(
            "Could not read \"{s}\" as a port (8000), a host:port (localhost:8000), or a folder (/path/to/site)"
        ))
    }

    /// Work out how a folder should be served.
    ///
    /// The aim is that pointing at a project root just works, so the port field
    /// is only needed for stacks that genuinely require a process of their own.
    pub fn detect_dir(dir: &str) -> Result<Target, String> {
        let p = std::path::Path::new(dir);
        let has = |rel: &str| p.join(rel).exists();

        // PHP front controllers first: a Laravel `public/` holds index.php, and
        // checking for index.html first would mis-detect projects that ship both.
        if has("public/index.php") {
            return Ok(Target::Php { root: p.join("public").to_string_lossy().into_owned() });
        }
        if has("index.php") {
            return Ok(Target::Php { root: dir.to_string() });
        }

        // A runnable JS project takes priority over any build output next to
        // it: during development the dev server is live and `dist/` is stale.
        if let Some((command, _script)) = Self::npm_dev_command(p) {
            let port = crate::runner::free_port(3000)
                .ok_or_else(|| "No free port available in 3000-3400".to_string())?;
            return Ok(Target::Node { dir: dir.to_string(), command, port });
        }

        // Built output of a JS project, in the usual places.
        for candidate in ["dist", "build", "out", "_site", "public"] {
            if p.join(candidate).join("index.html").exists() {
                return Ok(Target::Dir {
                    path: p.join(candidate).to_string_lossy().into_owned(),
                });
            }
        }
        if has("index.html") {
            return Ok(Target::Dir { path: dir.to_string() });
        }

        // A JS project with neither a dev script nor a build is genuinely
        // ambiguous — say so rather than serving a mystifying 404.
        if has("package.json") {
            return Err(format!(
                "{dir} has a package.json but no dev/start script and no build output. \
                 Point at its build folder (dist, out, build), or start it yourself \
                 and give the port."
            ));
        }

        Ok(Target::Dir { path: dir.to_string() })
    }

    /// The dev command for a JS project, respecting whichever package manager
    /// the lockfile indicates.
    fn npm_dev_command(dir: &std::path::Path) -> Option<(String, String)> {
        let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
        let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let scripts = json.get("scripts")?.as_object()?;

        // "dev" is the convention across Next, Nuxt, Vite and Astro.
        let script = ["dev", "start", "serve"]
            .into_iter()
            .find(|s| scripts.contains_key(*s))?;

        let runner = if dir.join("bun.lockb").exists() {
            "bun run"
        } else if dir.join("pnpm-lock.yaml").exists() {
            "pnpm run"
        } else if dir.join("yarn.lock").exists() {
            "yarn"
        } else {
            "npm run"
        };
        Some((format!("{runner} {script}"), script.to_string()))
    }

    /// The plain local address this target is reachable at, used for tunnelling.
    /// Folder-backed sites have no upstream of their own, so they go through
    /// Caddy's plain-HTTP listener instead.
    pub fn local_http_upstream(&self, plain_port: u16, domain: &str) -> String {
        match self {
            Target::Proxy { upstream } => format!("http://{upstream}"),
            Target::Node { port, .. } => format!("http://127.0.0.1:{port}"),
            Target::Dir { .. } | Target::Php { .. } => {
                let _ = domain;
                format!("http://127.0.0.1:{plain_port}")
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Target::Proxy { upstream } => upstream.clone(),
            Target::Dir { path } => path.clone(),
            Target::Php { root } => format!("{root} (PHP-FPM)"),
            Target::Node { dir, command, port } => format!("{dir} · {command} · :{port}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_port() {
        assert_eq!(
            Target::parse("8000").unwrap(),
            Target::Proxy { upstream: "127.0.0.1:8000".into() }
        );
    }

    #[test]
    fn parses_host_port_and_normalises_localhost() {
        assert_eq!(
            Target::parse("localhost:3000").unwrap(),
            Target::Proxy { upstream: "127.0.0.1:3000".into() }
        );
        assert_eq!(
            Target::parse("http://localhost:3000/").unwrap(),
            Target::Proxy { upstream: "127.0.0.1:3000".into() }
        );
    }

    #[test]
    fn keeps_non_localhost_hosts() {
        assert_eq!(
            Target::parse("192.168.1.10:9000").unwrap(),
            Target::Proxy { upstream: "192.168.1.10:9000".into() }
        );
    }

    #[test]
    fn rejects_nonsense() {
        assert!(Target::parse("").is_err());
        assert!(Target::parse("not a target").is_err());
        assert!(Target::parse("/definitely/not/here").is_err());
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join("st-target-tests").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(dir: &std::path::Path, rel: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn laravel_project_root_needs_no_port() {
        let d = scratch("laravel");
        touch(&d, "artisan");
        touch(&d, "public/index.php");
        let t = Target::parse(d.to_str().unwrap()).unwrap();
        match t {
            Target::Php { root } => assert!(root.ends_with("public")),
            other => panic!("expected Php, got {other:?}"),
        }
    }

    #[test]
    fn php_wins_over_html_when_a_project_ships_both() {
        let d = scratch("both");
        touch(&d, "public/index.php");
        touch(&d, "public/index.html");
        assert!(matches!(
            Target::parse(d.to_str().unwrap()).unwrap(),
            Target::Php { .. }
        ));
    }

    #[test]
    fn finds_built_output_of_a_js_project() {
        for out in ["dist", "build", "out"] {
            let d = scratch(&format!("js-{out}"));
            touch(&d, "package.json");
            touch(&d, &format!("{out}/index.html"));
            match Target::parse(d.to_str().unwrap()).unwrap() {
                Target::Dir { path } => assert!(path.ends_with(out)),
                other => panic!("expected Dir for {out}, got {other:?}"),
            }
        }
    }

    #[test]
    fn js_project_with_a_dev_script_becomes_a_supervised_server() {
        let d = scratch("js-dev");
        std::fs::write(d.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        match Target::parse(d.to_str().unwrap()).unwrap() {
            Target::Node { command, port, .. } => {
                assert_eq!(command, "npm run dev");
                assert!(port >= 3000);
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn respects_the_lockfile_when_choosing_a_package_manager() {
        for (lock, expected) in [
            ("pnpm-lock.yaml", "pnpm run dev"),
            ("yarn.lock", "yarn dev"),
            ("bun.lockb", "bun run dev"),
        ] {
            let d = scratch(&format!("js-{lock}"));
            std::fs::write(d.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
            touch(&d, lock);
            match Target::parse(d.to_str().unwrap()).unwrap() {
                Target::Node { command, .. } => assert_eq!(command, expected),
                other => panic!("expected Node for {lock}, got {other:?}"),
            }
        }
    }

    #[test]
    fn falls_back_to_start_when_there_is_no_dev_script() {
        let d = scratch("js-start");
        std::fs::write(d.join("package.json"), r#"{"scripts":{"start":"node ."}}"#).unwrap();
        match Target::parse(d.to_str().unwrap()).unwrap() {
            Target::Node { command, .. } => assert_eq!(command, "npm run start"),
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn js_project_with_neither_script_nor_build_explains_itself() {
        let d = scratch("js-raw");
        std::fs::write(d.join("package.json"), r#"{"name":"x"}"#).unwrap();
        let err = Target::parse(d.to_str().unwrap()).unwrap_err();
        assert!(err.contains("build folder"), "unhelpful message: {err}");
        assert!(err.contains("port"), "unhelpful message: {err}");
    }

    #[test]
    fn plain_static_folder_is_served_as_is() {
        let d = scratch("static");
        touch(&d, "index.html");
        match Target::parse(d.to_str().unwrap()).unwrap() {
            // parse() canonicalises, and on macOS /var is a symlink to /private/var.
            Target::Dir { path } => {
                assert_eq!(path, std::fs::canonicalize(&d).unwrap().to_str().unwrap())
            }
            other => panic!("expected Dir, got {other:?}"),
        }
    }

    #[test]
    fn parses_existing_folder() {
        let t = Target::parse("/tmp").unwrap();
        assert!(matches!(t, Target::Dir { .. }));
    }
}
