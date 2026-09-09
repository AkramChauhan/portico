# Portico

[![ci](https://github.com/AkramChauhan/portico/actions/workflows/ci.yml/badge.svg)](https://github.com/AkramChauhan/portico/actions/workflows/ci.yml)

**Give any local project a real HTTPS address — and a public one when you need it.**

A macOS app for local development domains. Point a name at a port or a project folder, get
a browser-trusted certificate, and optionally publish it so webhooks can reach it.

```
https://app.io                 → localhost:8000      (this Mac)
https://app.io                 → ~/Projects/my-app   (no port at all)
https://xyz.trycloudflare.com  → the same site       (the whole internet)
```

Nothing to install first. No Homebrew, no manual config, no editing `/etc/hosts` by hand.
One authorisation, once — and never again.

> **Status:** early but working. macOS only, and there are no signed releases yet — build
> from source for now. See [open items](#contributing).

---

## Why this exists

Adding a domain to DNS and signing it with a local CA gets you a real padlock — but only on
your own Mac. That distinction decides what your domain can be used for, and most tools
never make it explicit:

| Use case | Local domain | Needs a public URL |
|---|---|---|
| Browsing `https://app.io` | yes | – |
| OAuth **redirect URL** (Google, GitHub sign-in) | yes | – |
| Receiving **webhooks** (Stripe, Twilio, GitHub) | **no** | yes |

A webhook sender resolves your hostname over public DNS and validates the certificate
against public roots. Your hosts file and your local CA are invisible to it. Portico does
both halves and is explicit about which is which.

## Quick start

Requires **macOS** and a [Rust toolchain](https://rustup.rs) until signed builds exist.
Everything else Portico fetches itself.

```sh
git clone https://github.com/AkramChauhan/portico.git
cd portico
cargo build --release

./target/release/portico install          # fetches tools, then one authorisation
./target/release/portico test 8000        # is anything actually serving there?
./target/release/portico add app.io 8000  # live on http://app.io
./target/release/portico ssl app.io on    # now https://app.io, trusted
```

Or open the app and use the two-step wizard, which does the same with a progress bar.

## Port, or path?

Point the target at a project directory and the type is worked out for you:

| Stack | Path alone? | What happens |
|---|---|---|
| Laravel, Symfony, WordPress, any PHP | **yes** | served via `php_fastcgi` to PHP-FPM — no `artisan serve`, no port |
| Next, Nuxt, Vite, any `package.json` with a dev script | **yes** | Portico runs the dev server on a port it assigns |
| Built output (`dist/`, `build/`, `out/`) | **yes** | served straight off disk |
| Something already running | – | give it the port (`8000`, `localhost:8000`) |

## Public URLs

| Provider | Account | Stable hostname |
|---|---|---|
| **cloudflared** (default) | none | no — new random name each run |
| ngrok | required | with a paid plan |

A Cloudflare quick tunnel is anonymous: no login, no domain. ngrok needs an account, which
is why it is opt-in from Settings rather than the default — it would put a sign-up wall in
front of the first thing a new user tries.

## Compared with the alternatives

| | Local HTTPS | Path-only | Runs your dev server | Public URLs | Stack |
|---|---|---|---|---|---|
| **Portico** | yes | PHP, JS, static | yes | yes | any |
| [Laravel Valet](https://laravel.com/docs/valet) | yes | PHP | no | via `valet share` | PHP-first |
| [Laravel Herd](https://herd.laravel.com) | yes | PHP | no | Pro only | PHP-first |
| [localias](https://github.com/peterldowns/localias) | yes | no | no | no | any |
| [typicode/hotel](https://github.com/typicode/hotel) | no | no | yes | no | any |
| [ngrok](https://ngrok.com) | no | no | no | yes | any |

Both problems are well solved separately. Portico is the combination, without a PHP bias
and without a prerequisite list.

## Command line

```sh
portico install                    # prerequisites: tools, then access
portico doctor                     # what is and isn't working
portico mode standalone|system     # how much of the machine to use

portico test <target>              # is anything serving there?
portico add <domain> <target>      # add a vhost (HTTP)
portico ssl <domain> on|off        # install or remove SSL
portico ls / rm <domain>

portico run <domain> on|off        # start/stop a supervised dev server
portico logs / requests / health <domain>
portico tunnel <domain>            # public URL, until Ctrl-C

portico uninstall                  # undo every system change
```

## Documentation

**[akramchauhan.github.io/portico](https://akramchauhan.github.io/portico/index.html)** —
the engineering handbook covers how a request reaches your project, what Portico changes on
the machine, the risk register, and the platform traps behind the odd-looking code.

The docs are interactive HTML with no dependencies, published from `docs/` on every push to
`main`. They work equally well opened straight from disk, which is the point of having no
build step. Shared facts live in `docs/data/data.js` and are rendered rather than restated,
and `node docs/check-freshness.mjs` reports when a document has fallen behind the code it
describes.

## Security in one paragraph

Portico installs a root daemon (to bind ports 80/443) and a small root helper that rewrites
one marked block of `/etc/hosts` without asking again. That helper grants any process
running as you the permanent ability to point a hostname at loopback — bounded to exactly
that, with a strict allowlist and no path to root code execution. Caddy's admin API is
unauthenticated on loopback and drives that root daemon, which is the largest open risk.
All of it is removable with `portico uninstall`. The
[handbook](https://akramchauhan.github.io/portico/handbook.html) states the full model,
including what is accepted rather than solved.

Found something? Open an issue — or for anything sensitive, contact the maintainer directly
rather than filing publicly.

## Development

```
core/     engine — config, Caddyfile generation, DNS, hosts, processes, tunnels
cli/      portico, a thin wrapper over core
hostsd/   the root /etc/hosts helper — small on purpose
app/      Tauri desktop app (ui/ is plain HTML/CSS/JS, no build step)
```

```sh
cargo test                       # the whole suite
./scripts/build-sidecar.sh       # stage the helper for bundling
cd app && tauri build            # a real .app (needs @tauri-apps/cli)
```

System mode is the only mode that runs anything privileged, through a single generated
shell script. Read it before it runs:

```sh
cargo run --example dump_script -p portico-core -- install
```

## Contributing

Issues and pull requests welcome. Two things make review easy: **add a test for behaviour
you change**, and **anything touching a privileged path needs a test showing it is safe**,
not just that it works. `CLAUDE.md` lists the conventions that exist for a reason — each
one is there because its absence caused a bug.

Open items if you are looking for something to pick up: translating the Rust-side
strings, named Cloudflare tunnels for stable hostnames, and getting `cargo fmt --check`
and `cargo clippy -D warnings` clean enough to add to CI.

## License

MIT. See [LICENSE](LICENSE).
