# Portico

macOS app giving local projects real HTTPS addresses, plus public URLs for webhooks.
Rust workspace (`core`, `cli`, `hostsd`, `app/src-tauri`) with a plain HTML/CSS/JS frontend.

## Documentation

All human-facing documentation is interactive HTML in `docs/`, never Markdown. Use the
shared design system in `docs/assets/`; no CDN dependencies. Shared facts live in
`docs/data/data.js` and are rendered, not restated. Docs are updated in the same change
as the code, with `doc:reviewed` bumped. Run `node docs/check-freshness.mjs`.

Keep it consolidated: one handbook, important items only. Resist adding documents.

## Conventions that exist for a reason

- **Perform the system change first, persist config only on success.** Saving before the
  act succeeded caused four separate bugs (`add_site`, `remove_site`, `set_tunnel`,
  `set_run`) where config and reality disagreed permanently.
- **Never name a local variable `t`** in the frontend — that is the translation function,
  and shadowing it in a render path blanked the site list once already.
- **Values reaching the Caddyfile are quoted; values reaching the offline page are HTML
  escaped.** Both are injection sinks, one into a root-loaded config.
- **launchd**: retire a job with a retry loop before bootstrapping it, and set
  `ThrottleInterval` on `WatchPaths` jobs. Both defaults will bite silently.
- **i18n**: every language carries the same key set. Generate new blocks from the English
  key list rather than hand-writing them.

## Checks

```sh
cargo test                       # whole workspace
node docs/check-freshness.mjs    # docs vs the code they describe
```
