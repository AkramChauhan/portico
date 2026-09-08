use portico_core as st;

const USAGE: &str = "\
Portico — local vhosts with trusted HTTPS, plus public tunnels for webhooks

USAGE
  portico install                        Install prerequisites: fetch tools, then take access once
  portico setup                          Alias of install
  portico mode standalone|system         Switch how much of the machine is used
  portico doctor                         Show what is and is not working
  portico trust                          Re-install the local certificate authority
  portico fetch caddy|cloudflared         Force-download a tool into ~/.portico/bin
  portico test <target>                  Check something is actually serving there
  portico add <domain> <target> [opts]   Add a vhost (HTTP; add SSL after)
  portico ls                             List vhosts
  portico rm <domain>                    Remove a vhost
  portico ssl <domain> on|off            Install or remove SSL
  portico run <domain> on|off            Start/stop a dev server we supervise
  portico logs <domain>                  Tail that dev server's output
  portico requests <domain>              Recent requests the site handled
  portico health <domain>                Probe the site now
  portico tunnel <domain>                Expose it publicly; runs until Ctrl-C
  portico uninstall                      Undo every system change

ADD OPTIONS
  --ssl         Enable HTTPS immediately (default: HTTP, add SSL after testing)
  --spa         Fall back to index.html (folder targets)

TARGET can be a port (8000), a host:port (localhost:8000), or a folder (/path/to/site).

EXAMPLES
  portico test 8000
  portico add mysite.test 8000
  portico ssl mysite.test on
  portico add docs.test ~/Projects/docs-build --spa
  portico tunnel mysite.test
";

fn main() {
    // Rust aborts on SIGPIPE by default, so `portico ls | head` panics instead
    // of exiting quietly. Restore the conventional shell behaviour.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");

    match cmd {
        "setup" | "install" => {
            println!("1. Tools");
            let r = st::run_setup()?;
            for t in &r.tools {
                println!("   {} {:<12} {}", if t.downloaded { "downloaded" } else { "found     " }, t.name, t.detail);
            }
            println!("2. Access");
            println!("   {}", r.message.lines().last().unwrap_or(&r.message));
        }
        "mode" => mode(&args[1..])?,
        "uninstall" => println!("{}", st::run_uninstall()?),
        "doctor" => doctor(),
        "trust" => println!("{}", st::trust_ca()?),
        "fetch" => {
            let which = args.get(1).map(String::as_str).unwrap_or("");
            match which {
                "caddy" => println!("Installed: {}", st::bins::install_caddy()?.detail),
                "cloudflared" => println!("Installed: {}", st::tunnel::install_cloudflared()?),
                _ => anyhow::bail!("Usage: portico fetch caddy|cloudflared"),
            }
        }
        "ls" | "list" => list(),
        "add" => add(&args[1..])?,
        "test" => {
            let t = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: portico test <target>"))?;
            let r = st::test_target(t);
            println!("{} {} ({} ms)", if r.ok { "OK  " } else { "FAIL" }, r.message, r.elapsed_ms);
            if !r.ok {
                std::process::exit(1);
            }
        }
        "rm" | "remove" => {
            let d = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: portico rm <domain>"))?;
            st::remove_site(&st::config::slug(d))?;
            println!("Removed {d}");
        }
        "ssl" => {
            let d = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: portico ssl <domain> on|off"))?;
            let on = args.get(2).map(String::as_str) == Some("on");
            st::set_ssl(&st::config::slug(d), on)?;
            println!("{d}: SSL {}", if on { "on" } else { "off" });
        }
        "run" => {
            let d = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: portico run <domain> on|off"))?;
            let on = args.get(2).map(String::as_str) != Some("off");
            st::set_run(&st::config::slug(d), on)?;
            println!("{d}: dev server {}", if on { "starting" } else { "stopped" });
        }
        "logs" => {
            let d = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: portico logs <domain>"))?;
            println!("{}", st::site_dev_log(&st::config::slug(d), 40));
        }
        "requests" => requests(&args[1..])?,
        "tunnel-log" => {
            let d = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: portico tunnel-log <domain>"))?;
            println!("{}", st::site_tunnel_log(&st::config::slug(d), 40));
        }
        "health" => {
            let d = args.get(1).ok_or_else(|| anyhow::anyhow!("Usage: portico health <domain>"))?;
            let h = st::site_health(&st::config::slug(d))?;
            println!("{} {} ({} ms)", if h.ok { "UP  " } else { "DOWN" }, h.detail, h.ms);
        }
        "tunnel" => tunnel(&args[1..])?,
        _ => print!("{USAGE}"),
    }
    Ok(())
}

fn mode(args: &[String]) -> anyhow::Result<()> {
    use st::config::Mode;
    match args.first().map(String::as_str) {
        None => println!("{:?}", st::mode()),
        Some("standalone") => println!("{}", st::set_mode(Mode::Standalone)?),
        Some("system") => println!("{}", st::set_mode(Mode::System)?),
        Some(other) => anyhow::bail!("Unknown mode \"{other}\" — use standalone or system"),
    }
    Ok(())
}

fn doctor() {
    println!("Mode: {:?}\n", st::mode());
    for c in st::doctor() {
        println!("{} {:<38} {}", if c.ok { "OK  " } else { "FAIL" }, c.label, c.detail);
    }
}

fn list() {
    let sites = st::list_sites();
    if sites.is_empty() {
        println!("No sites yet. Try: portico add mysite.test 8000");
        return;
    }
    for s in sites {
        let dns = if s.wildcard_dns { "" } else { "  (/etc/hosts)" };
        println!("{:<32} -> {}{}", s.local_url, s.target_label, dns);
        if let Some(url) = s.tunnel_state.url {
            println!("{:<32}    public: {}", "", url);
        }
    }
}

fn requests(args: &[String]) -> anyhow::Result<()> {
    let d = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("Usage: portico requests <domain>"))?;
    let entries = st::site_requests(&st::config::slug(d), 25);
    if entries.is_empty() {
        println!("No requests recorded yet for {d}.");
        return Ok(());
    }
    println!("{:<6} {:>4} {:>9}  {:<7} {:<16} {}", "METHOD", "CODE", "TIME", "SOURCE", "FROM", "PATH");
    for e in entries {
        println!(
            "{:<6} {:>4} {:>7.1}ms  {:<7} {:<16} {}",
            e.method,
            e.status,
            e.duration_ms,
            if e.via_tunnel { "tunnel" } else { "local" },
            e.remote,
            e.path
        );
    }
    Ok(())
}

fn add(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() < 2 {
        anyhow::bail!("Usage: portico add <domain> <target> [--no-ssl] [--spa]");
    }
    // Sites start on plain HTTP so the target can be verified first; SSL is a
    // deliberate second step.
    let ssl = args.iter().any(|a| a == "--ssl");
    let spa = args.iter().any(|a| a == "--spa");

    let s = st::add_site(positional[0], positional[1], ssl, spa)?;
    println!("Added {} -> {}", s.local_url, s.target_label);
    if s.managed {
        println!("Starting the dev server — watch it with: portico logs {}", s.site.domain);
    }
    if !s.wildcard_dns {
        println!("Mapped {} in /etc/hosts.", s.site.domain);
    }
    Ok(())
}

fn tunnel(args: &[String]) -> anyhow::Result<()> {
    let domain = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("Usage: portico tunnel <domain>"))?;
    let id = st::config::slug(domain);
    st::set_tunnel(&id, true)?;

    println!("Starting public tunnel for {domain}...");
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let info = st::tunnel::status(&id);
        if let Some(url) = info.url {
            println!("\nPublic URL: {url}");
            println!("Webhooks and OAuth redirects sent here reach {domain}.");
            println!("Press Ctrl-C to stop.\n");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        if info.status == st::tunnel::TunnelStatus::Failed {
            anyhow::bail!("{}", info.error.unwrap_or_else(|| "tunnel failed".into()));
        }
    }
    anyhow::bail!("Timed out waiting for a public URL")
}
