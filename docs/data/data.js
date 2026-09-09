/* ==========================================================================
   Portico — single source of truth for facts appearing in MORE THAN ONE doc.
   Loaded as a plain <script> so it works over file:// (fetch would break).

   RULE: if a fact lives here, never restate it as prose. Render it.
   ========================================================================== */
window.DOCS = {

  version: '0.1.0',
  updated: '2026-09-09',

  /* Registry — rendered by docs/index.html */
  docs: [
    { file: 'handbook.html', title: 'Engineering handbook', status: 'Current', owner: 'akramchauhan',
      updated: '2026-09-09',
      blurb: 'How a request reaches your project, what Portico changes on the machine, the risks that come with that, and the platform traps that look like arbitrary complexity until you know what they defend against.',
      sources: ['core/src/caddy.rs', 'core/src/setup.rs', 'core/src/tunnel.rs', 'core/src/target.rs', 'core/src/runner.rs', 'core/src/lib.rs', 'hostsd/src/main.rs'] }
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
    { id: 'F13', s: 'todo', area: 'Public URL', t: 'Named Cloudflare tunnels',
      d: 'Would give a stable hostname without an ngrok account. Needs a domain on Cloudflare.' }
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
      how: 'Create tunnels in rapid succession from one IP and look for a 429 or an explicit limit message in the captured log.' },

    { id: 'A5', text: 'Tauri stages a sidecar into Contents/MacOS with the target triple stripped.',
      impact: 'Medium', confidence: 'Unverified',
      how: 'Run tauri build and list Portico.app/Contents/MacOS. Mitigated already: the lookup accepts both the plain and suffixed name.' }
  ],

  risks: [
    { id: 'R01', risk: 'Caddy admin API is unauthenticated on 127.0.0.1:2019 and drives a root daemon in System mode. Any local process can reconfigure it, including to serve any file root can read.',
      sev: 'High', area: 'Privilege', status: 'Accepted',
      mit: 'No clean fix: Caddy offers no way to set a unix socket’s permissions, so a socket would be unreachable by the unprivileged app. Not a privilege boundary in Standalone mode, where Caddy runs as the user.' },

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
      mit: 'Tunnels are never restored automatically: the flag is cleared at launch, so an unclean exit cannot silently republish a private site under a URL the user never saw.' }
  ],

  glossary: [
    { t: 'Loopback twin', d: 'A second plain-HTTP vhost for every site on 127.0.0.1:8880. Tunnels point here rather than at the HTTPS listener, so a tunnelled request lands on exactly the same handler as a local one and the tunnel never needs to trust the local CA.' },
    { t: 'System mode', d: 'Caddy runs as a root LaunchDaemon and binds ports 80/443, giving port-free URLs. One authorisation at install. The default.' },
    { t: 'Standalone mode', d: 'Caddy runs as a user LaunchAgent on port 8443. Touches nothing outside the home folder except an /etc/hosts line for a custom domain.' },
    { t: 'Quick tunnel', d: 'An anonymous Cloudflare tunnel. No account, no domain — but a new random hostname on every restart.' },
    { t: 'Hosts helper', d: 'portico-hostsd. A root binary launchd starts only when a specific request file changes; it rewrites one marked block of /etc/hosts and exits. Not a sudoers rule and not setuid.' },
    { t: 'Managed block', d: 'The region of /etc/hosts between Portico’s markers. Everything outside it is copied through untouched.' },
    { t: 'Sidecar', d: 'A helper binary bundled inside the .app. Tauri stages it from binaries/ with a target-triple suffix.' }
  ]
};
