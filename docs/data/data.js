/* ==========================================================================
   Portico — single source of truth for facts appearing in MORE THAN ONE doc.
   Loaded as a plain <script> so it works over file:// (fetch would break).

   RULE: if a fact lives here, never restate it as prose. Render it.
   ========================================================================== */
window.DOCS = {

  version: '0.1.0',
  updated: '2026-10-17',

  /* Registry — rendered by docs/index.html */
  docs: [
    { file: 'handbook.html', title: 'Engineering handbook', status: 'Current', owner: 'akramchauhan',
      updated: '2026-10-17',
      blurb: 'How a request reaches your project, what Portico changes on the machine, the risks that come with that, and the platform traps that look like arbitrary complexity until you know what they defend against.',
      sources: ['core/src/caddy.rs', 'core/src/setup.rs', 'core/src/tunnel.rs', 'core/src/target.rs', 'core/src/runner.rs', 'core/src/lib.rs', 'core/src/cloudflare.rs', 'core/src/secrets.rs', 'core/src/lan.rs', 'core/src/config.rs', 'core/src/hosts.rs', 'core/src/paths.rs', 'hostsd/src/main.rs'] }
  ],

  /* Capability status. s: done | doing | todo | blocked */
  features: [
    { id: 'F01', s: 'done', area: 'Serving',    t: 'Local domains with trusted HTTPS',
      d: 'Caddy internal CA, trusted in the user trust domain. Verified end to end against a real project: HTTP 200 with ssl_verify_result=0.' },
    { id: 'F02', s: 'done', area: 'Serving',    t: 'Path-only targets',
      d: 'PHP via php_fastcgi, JS dev servers supervised, static folders served directly. No port required for any of them.' },
    { id: 'F03', s: 'done', area: 'Serving',    t: 'Offline page',
      d: 'Generated per site, served on 502/503/504. Never binds the upstream port.' },
    { id: 'F04', s: 'done', area: 'Public URL', t: 'Cloudflare quick tunnels',
      d: 'No account required. Verified end to end: 200 with a publicly trusted certificate, POST bodies reaching the app.' },
    { id: 'F05', s: 'done', area: 'Public URL', t: 'ngrok as an alternative provider',
      d: 'Opt-in from Settings. Requires an account; a paid plan unlocks a fixed hostname.' },
    { id: 'F06', s: 'done', area: 'Monitoring', t: 'Request stream with real client IPs',
      d: 'Parsed from Caddy access logs. Internet traffic identified by CF-Ray, with CF-Connecting-IP as the source.' },
    { id: 'F07', s: 'done', area: 'Monitoring', t: 'In-app log viewer',
      d: 'All logs listed newest-first, readable and clearable from the app.' },
    { id: 'F08', s: 'done', area: 'Setup',      t: 'Zero prerequisites',
      d: 'caddy and cloudflared are fetched if absent. Both download paths verified.' },
    { id: 'F09', s: 'done', area: 'Setup',      t: 'Password-free /etc/hosts updates',
      d: 'A root helper started by launchd WatchPaths. After install, adding or removing a domain never prompts.' },
    { id: 'F10', s: 'done', area: 'Interface',  t: 'Ten languages',
      d: '124 keys each, parity enforced. Frontend only — see R07.' },
    { id: 'F11', s: 'done', area: 'Release',    t: 'Continuous integration',
      d: 'cargo test and the docs freshness check on macos-latest, per push and PR. '
       + 'Stages the hostsd sidecar first; binaries/ is gitignored, so without it '
       + 'tauri-build cannot resolve externalBin and portico-app will not compile.' },
    { id: 'F12', s: 'todo', area: 'Interface',  t: 'Translate Rust-originated strings',
      d: 'Diagnostics labels, errors and the onboarding tool rows are still English in every language.' },
    { id: 'F13', s: 'done', area: 'Public URL', t: 'Named Cloudflare tunnels',
      d: 'A fixed hostname per site on a domain you own. Portico creates the tunnel through the API, '
       + 'writes the CNAME, and runs it against the same loopback twin a quick tunnel uses. '
       + 'The API token lives in the login keychain and never reaches argv.' },
    { id: 'F14', s: 'done', area: 'Public URL', t: 'Fixed hostname is per site',
      d: 'Was one global setting, which two simultaneously published sites silently fought over. '
       + 'An older config migrates its value onto the first site that has none.' },
    { id: 'F16', s: 'done', area: 'Public URL', t: 'Both providers take their token in-app',
      d: 'Cloudflare and ngrok are entered in Settings and stored in the login keychain. '
       + 'Neither secret passes through argv: Cloudflare goes to curl on stdin, ngrok into a '
       + 'config file selected with --config. ngrok is verified by actually connecting, since '
       + 'it offers no validation endpoint — which also reveals the plan, so no separate check '
       + 'is needed. Neither token leaves the machine; Portico has no server to send them to.' },
    { id: 'F17', s: 'done', area: 'Serving',    t: 'Ports 80 and 443 stay free for other tools',
      d: 'Caddy runs as the user on loopback 18443/18080 in System mode. Domains resolve to 127.77.0.1, '
       + 'and a pf rule in the com.apple/portico anchor forwards that address\u2019s 443 and 80, so URLs keep '
       + 'no port while Docker binds 0.0.0.0:443. Verified with Docker holding *:443: localhost reached '
       + 'Docker, the site reached Caddy over IPv4 and IPv6.' },
    { id: 'F15', s: 'done', area: 'Serving',    t: 'Local network sharing',
      d: 'One plain-HTTP port per shared site, bound to every interface, addressed by IP so no '
       + 'device needs DNS or a certificate. Off by default and stated plainly in the interface.' }
  ],

  /* EVERY belief that is not verified fact. */
  assumptions: [
    { id: 'A1', text: 'Mac App Store distribution is impossible for Portico.',
      impact: 'High', confidence: 'Partly evidenced',
      how: 'Read guideline 2.4.5 and the sandbox entitlement docs directly, or submit a trial build. Evidence so far: sandboxed apps cannot use SMJobBless, cannot write to /Library/LaunchDaemons or /etc, and must sandbox all bundled executables.' },

    { id: 'A2', text: 'The doctor can report the CA as trusted after the certificate has been deleted.',
      impact: 'High', confidence: 'Unverified',
      how: 'Delete the Caddy CA from the login keychain, run portico doctor, then curl an https site. If doctor says OK and curl fails on verification, the check is a false green.' },

    { id: 'A3', text: 'The cloudflared process that died after passing preflight did so for an external reason, not a Portico bug.',
      impact: 'Medium', confidence: 'Unverified',
      how: 'Reproduce with the in-app log viewer open. tunnel-<site>.log now captures cloudflared stderr, which it did not when this happened.' },

    { id: 'A4', text: 'Quick-tunnel failures observed mid-session were Cloudflare rate limiting.',
      impact: 'Low', confidence: 'Unverified',
      how: 'Create tunnels in rapid succession from one IP and look for a 429 or an explicit limit message in the captured log.' }
  ],

  risks: [
    { id: 'R01', risk: 'Caddy admin API is unauthenticated on 127.0.0.1:2019. While System mode ran Caddy as root, any local process could reconfigure it to serve any file root can read.',
      sev: 'Low', area: 'Privilege', status: 'Mitigated',
      mit: 'Caddy runs as the user in both modes since pf took over ports 80 and 443, so the API grants nothing a process running as the user does not already have. Still unauthenticated; no longer a privilege boundary.' },

    { id: 'R02', risk: 'The /etc/hosts helper grants any process running as the user permanent, password-free ability to map hostnames to loopback.',
      sev: 'Medium', area: 'Privilege', status: 'Accepted',
      mit: 'Bounded by design: only 127.0.0.1/::1 mappings inside our own markers, strict hostname allowlist, capped count, atomic replace. No path to root code execution. This is the honest cost of "stop asking me for a password".' },

    { id: 'R03', risk: 'Downloaded binaries (caddy, cloudflared) are trusted on TLS plus a version check, with no checksum or signature verification.',
      sev: 'Medium', area: 'Supply chain', status: 'Open',
      mit: 'Pin checksums per release, or verify codesign for cloudflared. Currently the trust anchor is HTTPS to the vendor’s own host.' },

    { id: 'R04', risk: 'The privileged setup script is written to a user-owned directory and then executed as root, leaving a TOCTOU window.',
      sev: 'Low', area: 'Privilege', status: 'Mitigated',
      mit: 'Directory is 0700, so only the same user (already the threat actor for R02) can race it. Inherent to any installer of this shape.' },

    { id: 'R05', risk: 'Pid files record no process identity, so a recycled pid could cause Portico to signal an unrelated process group.',
      sev: 'Low', area: 'Correctness', status: 'Open',
      mit: 'Record the process start time alongside the pgid and verify both before signalling.' },

    { id: 'R06', risk: 'A tool Portico downloaded itself will never be updated, since update checking was removed.',
      sev: 'Low', area: 'Maintenance', status: 'Accepted',
      mit: 'Homebrew copies are unaffected and stay the user’s to manage. A manual "update tools" action could be added without reinstating automatic checks.' },

    { id: 'R07', risk: 'Strings originating in Rust are English in every language, including the onboarding tool rows — the first screen a new user sees.',
      sev: 'Medium', area: 'Interface', status: 'Open',
      mit: 'Have Rust return stable keys plus data and let the frontend render the sentence, keeping one catalogue rather than two.' },

    { id: 'R08', risk: 'A public tunnel exposes whatever sits behind it, including apps with no authentication.',
      sev: 'Medium', area: 'Exposure', status: 'Mitigated',
      mit: 'Tunnels are never restored automatically: the flag is cleared at launch, so an unclean exit cannot silently republish a private site under a URL the user never saw.' },

    { id: 'R09', risk: 'A LAN-shared site is reachable by anyone on the same network, over plain HTTP, and the setting survives a restart — including a move from home to a café.',
      sev: 'Medium', area: 'Exposure', status: 'Accepted',
      mit: 'Off by default and per site. Unlike a tunnel the address is stable and deliberately chosen, so clearing it at launch would break the one use it has. The interface states the café case next to the switch, and the site carries a badge while it is on.' },

    { id: 'R10', risk: 'The Cloudflare API token can edit DNS and create tunnels on a real account.',
      sev: 'Medium', area: 'Privilege', status: 'Mitigated',
      mit: 'Stored in the login keychain, never in config.json (0644) and never in argv — security and curl both receive it on stdin. Verified before it is saved. Scope is the user’s to choose: Zone:DNS:Edit plus Account:Tunnel:Edit is enough.' },

    { id: 'R11', risk: 'ngrok quotes a rejected authtoken back inside its own error text, which Portico captures into tunnel-<site>.log and shows in the app.',
      sev: 'Medium', area: 'Exposure', status: 'Mitigated',
      mit: 'Every line the agent prints is scrubbed of the token before it reaches the log file or the error surfaced in the interface. Found by feeding ngrok a deliberately bad token and reading what it printed.' },

    { id: 'R12', risk: 'ngrok’s own config file takes precedence over NGROK_AUTHTOKEN, so on a machine that already had a token configured, an invalid one entered in Portico was accepted and stored.',
      sev: 'Medium', area: 'Correctness', status: 'Fixed',
      mit: 'The agent is now run with --config pointing at a file Portico writes (0600), which replaces the default instead of merging with it. The validation probe uses the same file, so it tests the token the user actually entered.' },

    { id: 'R13', risk: 'In System mode Caddy bound 0.0.0.0:443, so every site was reachable from the local network by anyone who knew the hostname — with or without local network sharing switched on.',
      sev: 'Medium', area: 'Exposure', status: 'Fixed',
      mit: 'Every vhost binds 127.0.0.1 and ::1 in both modes; the LAN listener is the only socket on another interface. A test asserts every System-mode listener binds loopback and none binds 80 or 443.' },

    { id: 'R14', risk: 'Setting a fixed hostname on a name that already has a DNS record would overwrite it, pointing a live hostname at a laptop — and stamp it as Portico-managed, so clearing the hostname later deleted it outright.',
      sev: 'Medium', area: 'Correctness', status: 'Fixed',
      mit: 'upsert_dns now refuses any record whose comment does not mark it as ours, matching the guard delete_dns already had. The apex was the likely casualty, since an empty subdomain in the interface means the domain itself.' },

    { id: 'R15', risk: 'Forwarding depends on pf staying enabled and on /etc/pf.conf evaluating com.apple/* anchors. A customised pf.conf, or anything running pfctl -d, silently stops every System-mode site.',
      sev: 'Low', area: 'Correctness', status: 'Mitigated',
      mit: 'The doctor asks 127.77.0.1:80 for an unused hostname and requires Portico\u2019s own 404, so a stopped forward cannot pass as Docker answering. It is a fixable check: setup reloads the rules.' },

    { id: 'R16', risk: 'Another dnsmasq config claiming the same TLD — Laravel Valet maps .test to 127.0.0.1 — gives two answers. A browser that picks 127.0.0.1 reaches whatever holds 127.0.0.1:443 rather than the site.',
      sev: 'Low', area: 'Correctness', status: 'Open',
      mit: 'Invisible while both answers were 127.0.0.1. The doctor now reports it without blocking; the other config is the user\u2019s to remove. Custom domains are unaffected, since /etc/hosts answers them.' }
  ],

  glossary: [
    { t: 'Loopback twin', d: 'A second plain-HTTP vhost for every site on 127.0.0.1:8880. Tunnels point here rather than at the HTTPS listener, so a tunnelled request lands on exactly the same handler as a local one and the tunnel never needs to trust the local CA.' },
    { t: 'System mode', d: 'Caddy runs as a user LaunchAgent on loopback 18443/18080; pf forwards 127.77.0.1:443 and :80 to it, giving port-free URLs without holding 443. One authorisation at install. The default.' },
    { t: 'Loopback forwarding', d: 'Two lo0 aliases (127.77.0.1, fd70:6f72:7469:636f::1) plus pf rdr rules carrying their 443 and 80 to Caddy. Re-applied at every boot by io.portico.loopback, because neither survives a restart \u2014 verified after a reboot: the job logged success and the doctor\u2019s forwarding probe passed.' },
    { t: 'Standalone mode', d: 'Caddy runs as a user LaunchAgent on port 8443. Touches nothing outside the home folder except an /etc/hosts line for a custom domain.' },
    { t: 'Quick tunnel', d: 'An anonymous Cloudflare tunnel. No account, no domain — but a new random hostname on every restart.' },
    { t: 'Named tunnel', d: 'A Cloudflare tunnel that Portico creates on your account, with a CNAME pointing at <id>.cfargotunnel.com. The hostname is yours and survives restarts, which is what a webhook registered days ago needs.' },
    { t: 'LAN listener', d: 'One plain-HTTP port per shared site, bound to every interface. Addressed by IP because no other device can resolve a .test name or trust the local CA.' },
    { t: 'Hosts helper', d: 'portico-hostsd. A root binary launchd starts only when a specific request file changes; it rewrites one marked block of /etc/hosts and exits. Not a sudoers rule and not setuid.' },
    { t: 'Managed block', d: 'The region of /etc/hosts between Portico’s markers. Everything outside it is copied through untouched.' },
    { t: 'Sidecar', d: 'A helper binary bundled inside the .app. Tauri stages it from binaries/ with a target-triple suffix and strips the suffix in Contents/MacOS — verified in the 0.1.0 bundle.' }
  ]
};
