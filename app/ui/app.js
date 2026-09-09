import { LANGUAGES, resolveLanguage, setLanguage, t, applyTranslations } from "./i18n.js";

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const $ = (id) => document.getElementById(id);
const $$ = (sel) => [...document.querySelectorAll(sel)];
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
};

let sites = [];
let currentMode = "standalone";
let testedTarget = null;   // exact target string that last passed a test
let pollTimer = null;
let pendingDelete = null;
let installed = false;
let searchTerm = "";

/* ───────────────────────── toast ───────────────────────── */
let toastTimer = null;
/* Run an async action while the button says it is working.
 *
 * Several of these take real time — reaching the Cloudflare API, opening a
 * tunnel to find out whether ngrok accepts a token, asking for a password.
 * Without a mark on the control itself the only feedback was the pointer, and
 * a window that looks frozen reads as a hang rather than as progress.
 *
 * The label is kept in place and merely made invisible, so the button does
 * not change width and the row does not jump. */
/* The switch equivalent. The knob is swapped for a spinner in place, so the
 * row keeps its exact size while a tunnel comes up or Caddy reloads. */
async function withBusySwitch(wrap, cb, fn) {
  if (wrap.dataset.busy === "1") return;
  wrap.dataset.busy = "1";
  cb.disabled = true;
  wrap.classList.add("busy");
  try {
    return await fn();
  } finally {
    wrap.classList.remove("busy");
    wrap.dataset.busy = "";
    cb.disabled = false;
  }
}

async function withBusy(btn, fn) {
  if (!btn) return fn();
  if (btn.dataset.busy === "1") return;      // ignore a double click
  const wasDisabled = btn.disabled;
  btn.dataset.busy = "1";
  btn.disabled = true;
  btn.classList.add("busy");
  try {
    return await fn();
  } finally {
    btn.classList.remove("busy");
    btn.dataset.busy = "";
    btn.disabled = wasDisabled;
  }
}

function toast(msg) {
  // Not named `t` — that is the translation function, and shadowing it inside
  // a render path has already cost one blanked screen.
  const box = $("toast");
  box.textContent = msg;
  box.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => box.classList.add("hidden"), 3200);
}

/* ───────────────────────── theme ───────────────────────── */
function applyTheme(theme) {
  // "system" means no attribute, so the OS preference governs.
  if (theme === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", theme);
  $$("#theme-seg button").forEach((b) => b.classList.toggle("active", b.dataset.theme === theme));
}

$$("#theme-seg button").forEach((b) => {
  b.onclick = async () => {
    applyTheme(b.dataset.theme);           // instant, before the round trip
    try { await invoke("set_theme", { theme: b.dataset.theme }); }
    catch (e) { toast(String(e)); }
  };
});

/* ───────────────────────── language ───────────────────────── */
let langSetting = "auto";

/// Every `.langpicker` on the page. There are two — one in the top bar and one
/// on the onboarding screen, which is shown while the shell is hidden.
const pickers = () => $$(".langpicker");
const partOf = (picker, role) => picker.querySelector(`[data-role="${role}"]`);

function buildMenu(picker) {
  const menu = partOf(picker, "menu");
  menu.replaceChildren();

  const item = (code, flag, label) => {
    const b = el("button", "lang-item");
    b.setAttribute("role", "menuitemradio");
    b.append(el("span", "lang-flag", flag), el("span", "lang-label", label));
    if (langSetting === code) {
      b.classList.add("active");
      b.setAttribute("aria-checked", "true");
      b.append(el("span", "lang-tick", "✓"));
    }
    b.onclick = () => chooseLanguage(code);
    return b;
  };

  menu.append(item("auto", "🌐", t("settings.language.auto")));
  menu.append(el("div", "lang-sep"));
  for (const l of LANGUAGES) menu.append(item(l.code, l.flag, l.label));
}

/// Apply a language to the whole interface. Static markup is translated by
/// attribute; anything rendered from JS re-renders afterwards.
function applyLanguage(setting) {
  langSetting = setting || "auto";
  const resolved = resolveLanguage(langSetting);
  setLanguage(resolved);

  const shown = LANGUAGES.find((l) => l.code === resolved);
  for (const p of pickers()) {
    // The button always shows the language actually in use, even on "auto" —
    // a globe alone would not tell you which language you are looking at.
    partOf(p, "flag").textContent = shown ? shown.flag : "🌐";
    partOf(p, "code").textContent = resolved.toUpperCase();
    partOf(p, "btn").title = shown ? shown.label : t("settings.language.auto");
    buildMenu(p);
  }
  applyTranslations();
}

async function chooseLanguage(code) {
  closeMenus();
  applyLanguage(code);
  try {
    await invoke("set_language", { language: code });
  } catch (err) {
    toast(String(err));
  }
  await refreshAll();
}

function closeMenus() {
  for (const p of pickers()) {
    partOf(p, "menu").classList.add("hidden");
    partOf(p, "btn").setAttribute("aria-expanded", "false");
  }
}

for (const p of pickers()) {
  const btn = partOf(p, "btn");
  const menu = partOf(p, "menu");
  btn.onclick = (e) => {
    e.stopPropagation();
    const wasOpen = !menu.classList.contains("hidden");
    closeMenus();
    if (!wasOpen) {
      menu.classList.remove("hidden");
      btn.setAttribute("aria-expanded", "true");
    }
  };
  menu.addEventListener("click", (e) => e.stopPropagation());
}
document.addEventListener("click", closeMenus);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeMenus();
});

/* ───────────────────────── navigation ───────────────────────── */
function showView(name) {
  $$(".view").forEach((v) => v.classList.toggle("hidden", v.dataset.view !== name));
  $$(".nav").forEach((n) => n.classList.toggle("active", n.dataset.view === name));
  if (name === "logs") refreshLogs().catch(() => {});
}
$$(".nav").forEach((n) => (n.onclick = () => showView(n.dataset.view)));
$("health-pill").onclick = () => showView("diagnostics");
$("site-back").onclick = () => { openSiteId = null; showView("sites"); };

/* ───────────────────────── onboarding wizard ───────────────────────── */
const TOOL_ORDER = ["caddy", "cloudflared", "dnsmasq"];
const TOTAL_STEPS = 2;
const toolRows = new Map();
let wizStep = 1;
let toolsReady = false;

function obToolRow(name) {
  if (toolRows.has(name)) return toolRows.get(name);
  const li = el("li", "ob-tool");
  const bar = el("div", "bar indet");
  bar.append(el("i"));
  bar.classList.add("hidden");
  li.append(el("span", "tname", name), el("span", "tdetail", "…"), bar, el("span", "tstate", ""));
  $("ob-tools").append(li);
  toolRows.set(name, li);
  return li;
}

function showStep(n) {
  wizStep = n;
  for (const panel of $$(".wiz-panel")) {
    panel.classList.toggle("hidden", Number(panel.dataset.panel) !== n);
  }
  for (const dot of $$(".wiz-dots i")) {
    const d = Number(dot.dataset.dot);
    dot.classList.toggle("on", d === n);
    dot.classList.toggle("done", d < n);
  }
  $("wiz-progress").textContent = t("onboarding.progress")
    .replace("{n}", n)
    .replace("{total}", TOTAL_STEPS);
  $("ob-back").classList.toggle("hidden", n === 1);

  const next = $("ob-next");
  // Step 1 only downloads when something is missing; otherwise it just moves on.
  next.dataset.i18n = n === 1 && !toolsReady ? "onboarding.install" : "onboarding.continue";
  next.textContent = t(next.dataset.i18n);
}

function resetOnboarding() {
  $("ob-tools").replaceChildren();
  toolRows.clear();
  TOOL_ORDER.forEach(obToolRow);
  $("ob-access").textContent = "";
  showStep(1);
}

/// Fill the checklist with what is actually on this machine, before the user
/// commits to anything.
async function loadToolStatus() {
  let tools;
  try { tools = await invoke("tools_status"); }
  catch { return; }

  toolsReady = true;
  for (const tool of tools) {
    const li = obToolRow(tool.name);
    li.querySelector(".tdetail").textContent = tool.detail;
    li.querySelector(".bar").classList.add("hidden");
    const state = li.querySelector(".tstate");
    if (tool.path) state.textContent = "✓";
    else if (tool.name === "dnsmasq") state.textContent = "optional";
    else { state.textContent = "↓"; toolsReady = false; }
  }
  showStep(wizStep);
}

/// Step 2's wording depends on the mode: Standalone never asks for a password,
/// System asks once. Saying the wrong one is worse than saying nothing.
function describeAccessStep() {
  const standalone = currentMode === "standalone";
  for (const b of $$("#ob-modes .mode")) {
    b.classList.toggle("active", b.dataset.mode === currentMode);
  }
  const title = $("ob-step2-title");
  title.dataset.i18n = standalone
    ? "onboarding.step2.title.standalone" : "onboarding.step2.title";
  title.textContent = t(title.dataset.i18n);
  $("ob-step2-lead").textContent = t(
    standalone ? "onboarding.step2.lead.standalone" : "onboarding.step2.lead.system");
  $("ob-step2-ask").textContent = t(
    standalone ? "onboarding.step2.ask.standalone" : "onboarding.step2.ask.system");
}

listen("install-progress", (event) => {
  const p = event.payload;
  const li = obToolRow(p.tool);
  const detail = li.querySelector(".tdetail");
  const bar = li.querySelector(".bar");
  const state = li.querySelector(".tstate");

  detail.textContent = p.detail;
  if (p.phase === "downloading") {
    bar.classList.remove("hidden");
    if (p.total > 0) {
      bar.classList.remove("indet");
      const pct = Math.min(100, Math.round((p.downloaded / p.total) * 100));
      bar.querySelector("i").style.width = pct + "%";
      state.textContent = pct + "%";
    } else {
      bar.classList.add("indet");
      state.textContent = p.downloaded ? (p.downloaded / 1048576).toFixed(1) + " MB" : "";
    }
  } else {
    bar.classList.add("hidden");
    state.textContent = { ready: "✓", verifying: "…", skipped: "optional" }[p.phase] ?? "";
  }
});

// Choosing here only records the preference — nothing is installed until
// Continue, so switching back and forth costs nothing.
for (const b of $$("#ob-modes .mode")) {
  b.onclick = async () => {
    currentMode = b.dataset.mode;
    describeAccessStep();
    try { await invoke("set_mode_preference", { mode: currentMode }); }
    catch (e) { toast(String(e)); }
  };
}

$("ob-back").onclick = () => showStep(1);

$("ob-next").onclick = () => withBusy($("ob-next"), async () => {
  const next = $("ob-next");
  try {
    if (wizStep === 1) {
      if (!toolsReady) {
        next.textContent = t("onboarding.installing");
        await invoke("install_tools");
        await loadToolStatus();
      }
      describeAccessStep();
      showStep(2);
    } else {
      next.textContent = t("onboarding.installing");
      $("ob-access").textContent = t("onboarding.step2.waiting");
      await invoke("install_access");
      toast(t("toast.installed"));
      await refreshAll();     // clears the blockers, which reveals the app
    }
  } catch (e) {
    $("ob-access").className = "ob-access error";
    $("ob-access").textContent = String(e);
    toast(String(e));
  } finally {
    // The spinner covers the label, so it is only reset once, at the end.
    next.textContent = t(next.dataset.i18n || "onboarding.continue");
  }
});

/* ───────────────────────── diagnostics ───────────────────────── */
async function refreshDoctor() {
  const checks = await invoke("doctor");
  const failing = checks.filter((c) => !c.ok);
  const blocking = failing.filter((c) => c.fixable);

  installed = blocking.length === 0;
  $("onboarding").classList.toggle("hidden", installed);
  $("shell").classList.toggle("hidden", !installed);
  $("statusbar").classList.toggle("hidden", !installed);

  const pill = $("health-pill");
  pill.classList.toggle("ok", failing.length === 0);
  pill.classList.toggle("bad", failing.length > 0);
  $("pill-text").textContent =
    failing.length === 0 ? t("status.ready") : `${failing.length} issue${failing.length > 1 ? "s" : ""}`;

  const badge = $("nav-issues");
  badge.classList.toggle("hidden", failing.length === 0);
  badge.textContent = failing.length;

  const list = $("checks");
  list.replaceChildren();
  for (const c of checks) {
    const li = el("li");
    li.append(
      el("span", `dot ${c.ok ? "ok" : "bad"}`),
      el("span", "check-label", c.label),
      el("span", "check-detail", c.detail)
    );
    list.append(li);
  }
}

/* ───────────────────────── sites ───────────────────────── */
async function refreshSites() {
  sites = await invoke("list_sites");
  const list = $("site-list");

  const term = searchTerm.trim().toLowerCase();
  const shown = term
    ? sites.filter(
        (s) =>
          s.domain.toLowerCase().includes(term) ||
          s.target_label.toLowerCase().includes(term)
      )
    : sites;

  $("empty").classList.toggle("hidden", shown.length > 0);
  if (sites.length && !shown.length) {
    $("empty").querySelector("h3").textContent = t("sites.nomatch.title");
    $("empty").querySelector("p").textContent = `Nothing matches "${searchTerm}".`;
  } else {
    $("empty").querySelector("h3").textContent = t("sites.empty.title");
    $("empty").querySelector("p").textContent =
      t("sites.empty.body");
  }

  $("nav-count").textContent = sites.length || "";
  $("status-sites").textContent = sites.length
    ? `${sites.length} site${sites.length > 1 ? "s" : ""}`
    : "";
  // Build first, swap once. Clearing the list up front meant any render error
  // left the user staring at an empty page with their sites apparently gone.
  const frag = document.createDocumentFragment();
  try {
    for (const s of shown) frag.append(renderSite(s));
  } catch (e) {
    toast(String(e));
    console.error("rendering sites failed", e);
    return;                 // leave the previous list on screen
  }
  const scroller = document.querySelector(".content");
  const listY = scroller ? scroller.scrollTop : 0;
  list.replaceChildren(frag);
  if (scroller && openSiteId === null) scroller.scrollTop = listY;
  renderOpenSite();

  schedulePoll();
  invoke("refresh_tray").catch(() => {});
}

function renderSite(s) {
  const card = el("div", "site");

  const top = el("div", "site-top");
  const url = el("button", "site-url", s.local_url);
  url.onclick = () => invoke("open_url", { url: s.local_url });
  top.append(url);

  const badges = el("div", "badges");
  badges.append(s.ssl ? el("span", "badge ssl", "HTTPS") : el("span", "badge plain", "HTTP"));
  if (s.spa) badges.append(el("span", "badge", "SPA"));
  if (!s.wildcard_dns) {
    const b = el("span", "badge hosts", "hosts");
    b.title = "Pinned in /etc/hosts — updated without a password.";
    badges.append(b);
  }
  if (s.public_domain) {
    const b = el("span", "badge public", s.public_domain);
    b.title = t("site.fixedUrl.hint");
    badges.append(b);
  }
  if (s.lan) {
    const b = el("span", "badge lan", t("badge.lan"));
    b.title = s.lan_url || "";
    badges.append(b);
  }
  top.append(badges);

  const target = el("div", "site-target mono", "→ " + s.target_label);
  const bottom = el("div", "site-bottom");

  if (s.managed) bottom.append(runToggle(s));
  bottom.append(tunnelToggle(s), renderTunnelState(s));

  // Only the call to action stays on the card. Removing SSL, reading the
  // request stream and deleting the site all belong to the one site, so they
  // live on its page rather than competing for room in a list.
  if (!s.ssl) {
    const ssl = el("button", "btn primary small", t("site.installSsl"));
    ssl.onclick = () => withBusy(ssl, async () => {
      try { await invoke("set_ssl", { id: s.id, on: true }); toast(t("toast.sslOn")); }
      catch (e) { toast(String(e)); }
      await refreshAll();
    });
    bottom.append(ssl);
  }

  const manage = el("button", "btn ghost small", t("site.manage"));
  manage.onclick = () => openSite(s.id);
  bottom.append(manage);

  card.append(top, target, bottom);
  return card;
}

/* ───────────────────────── one site ─────────────────────────

   Everything belonging to a single site, including the one control that
   reaches outside this Mac. Exposure lives here rather than on the card so
   there is room to say plainly what it does before it is switched on. */
let openSiteId = null;
let currentProvider = "cloudflare";

function openSite(id) {
  openSiteId = id;
  showView("site");
  // The zone list drives the hostname dropdown. If Settings has not been
  // visited yet it will still be empty, so fetch it and redraw.
  if (!cfZones.length) refreshCloudflare().then(renderOpenSite).catch(() => {});
  refreshSites().catch(() => {});
}

/* Split a hostname into the part the user chooses and the domain they own.
 *
 * Longest zone wins, so hooks.api.example.com belongs to api.example.com when
 * the account holds both. A hostname under no known zone returns null, which
 * keeps the plain text field for it — an ngrok custom domain is not a
 * Cloudflare zone and must not be silently rewritten. */
function splitHostname(host, zones) {
  const h = (host || "").trim().toLowerCase();
  if (!h) return { sub: "", zone: zones[0] ? zones[0].name : "" };

  let best = "";
  for (const z of zones) {
    const n = z.name.toLowerCase();
    if ((h === n || h.endsWith("." + n)) && n.length > best.length) best = n;
  }
  if (!best) return null;
  return { sub: h === best ? "" : h.slice(0, -(best.length + 1)), zone: best };
}

/* Redraw the open site page, or fall back to the list if it went away. */
function renderOpenSite() {
  if (!openSiteId) return;
  const s = sites.find((x) => x.id === openSiteId);
  if (!s) {
    openSiteId = null;
    showView("sites");
    return;
  }
  // The whole page is rebuilt on every refresh, and the poll refreshes while
  // a tunnel starts. Without this, saving a hostname threw you back to the
  // top of the page you were reading.
  const scroller = document.querySelector(".content");
  const y = scroller ? scroller.scrollTop : 0;
  $("site-page").replaceChildren(renderSitePage(s));
  if (scroller) scroller.scrollTop = y;
}

function renderSitePage(s) {
  const page = el("div", "sitepage");

  const head = el("div", "sitepage-head");
  const title = el("button", "site-url lg", s.local_url);
  title.onclick = () => invoke("open_url", { url: s.local_url });
  const badges = el("div", "badges");
  badges.append(s.ssl ? el("span", "badge ssl", "HTTPS") : el("span", "badge plain", "HTTP"));
  if (s.spa) badges.append(el("span", "badge", "SPA"));
  if (!s.wildcard_dns) badges.append(el("span", "badge hosts", "hosts"));
  head.append(title, badges);
  page.append(head, el("p", "muted", "\u2192 " + s.target_label));

  /* Local address */
  const local = section(t("sitepage.local"), t("sitepage.local.body"));
  local.append(row(
    t("sitepage.ssl"), t("sitepage.ssl.body"),
    (() => {
      const b = el("button", s.ssl ? "btn ghost small" : "btn primary small",
                   s.ssl ? t("site.removeSsl") : t("site.installSsl"));
      b.onclick = () => withBusy(b, async () => {
        try { await invoke("set_ssl", { id: s.id, on: !s.ssl }); toast(s.ssl ? t("toast.sslOff") : t("toast.sslOn")); }
        catch (e) { toast(String(e)); }
        await refreshAll();
      });
      return b;
    })()
  ));
  page.append(local);

  /* Dev server, only where there is one to supervise */
  if (s.managed) {
    const server = section(t("sitepage.server"), t("sitepage.server.body"));
    server.append(row(t("sitepage.server.run"), t("sitepage.server.run.body"), runToggle(s)));
    page.append(server);
  }

  /* Local network — reachable from other devices here, but not the internet */
  const net = section(t("sitepage.lan"), t("sitepage.lan.body"));
  net.classList.add("sitepage-lan");

  const lanToggle = el("label", "switch");
  const lanCb = el("input");
  lanCb.type = "checkbox";
  lanCb.checked = !!s.lan;
  lanCb.onchange = () => withBusySwitch(lanToggle, lanCb, async () => {
    try {
      await invoke("set_lan", { id: s.id, on: lanCb.checked });
      toast(lanCb.checked ? t("toast.lanOn") : t("toast.lanOff"));
    } catch (e) { lanCb.checked = !lanCb.checked; toast(String(e)); }
    finally { await refreshSites(); }
  });
  lanToggle.append(lanCb, el("span", "track"), el("span", null, t("sitepage.lan.toggle")));

  const lanAddress = el("div", "expose-state");
  if (s.lan && s.lan_url) {
    const link = el("button", "tunnel-url", s.lan_url);
    link.onclick = () => invoke("open_url", { url: s.lan_url });
    const copy = el("button", "btn ghost small", t("site.copy"));
    copy.onclick = async () => {
      await navigator.clipboard.writeText(s.lan_url);
      toast(t("toast.copied"));
    };
    lanAddress.append(link, copy);
  } else if (s.lan) {
    lanAddress.append(el("span", "muted", t("sitepage.lan.noAddress")));
  }
  net.append(row(t("sitepage.lan.share"), t("sitepage.lan.share.body"), lanToggle, lanAddress));
  page.append(net);

  /* Public access — the part that leaves this Mac */
  const pub = section(t("sitepage.public"), t("sitepage.public.body"));
  pub.classList.add("sitepage-public");

  const exposeState = el("div", "expose-state");
  exposeState.append(renderTunnelState(s));
  pub.append(row(t("sitepage.expose"), t("sitepage.expose.body"), tunnelToggle(s), exposeState));

  const save = el("button", "btn small", t("site.save"));
  const apply = (value, btn) => withBusy(btn || save, async () => {
    // Changing the name releases the old one, which means stopping whatever
    // is serving it. Say so: the toggle flicking off on its own otherwise
    // reads as the app losing the site.
    const wasExposed = s.tunnel;
    try {
      await invoke("set_public_domain", { id: s.id, domain: value });
      if (!value) toast(t("toast.fixedUrlCleared"));
      else toast(wasExposed ? t("toast.fixedUrlSetStopped") : t("toast.fixedUrlSet"));
    } catch (e) { toast(String(e)); }
    finally { await refreshSites(); }
  });

  const clear = el("button", "btn ghost small", t("site.clear"));
  clear.disabled = !s.public_domain;

  // With a connected Cloudflare account the domain half is not free text — it
  // has to be one of the zones the token can write to. Offering the list
  // removes both the typing and the "that is not inside any of your domains"
  // failure that used to come after it.
  const split = currentProvider === "cloudflare" && cfZones.length
    ? splitHostname(s.public_domain, cfZones)
    : null;

  // One bordered field either way, so the two shapes do not read as two
  // different controls: what changes is whether the domain half is a fixed
  // list or part of the same free text.
  const field = el("div", "host-input");
  let read;

  if (split) {
    const sub = el("input");
    sub.type = "text";
    sub.placeholder = "hooks";
    sub.spellcheck = false;
    sub.value = split.sub;

    const zone = el("select", "host-zone");
    for (const z of cfZones) {
      // The leading dot lives in the label, so the field reads as one name.
      const opt = el("option", null, "." + z.name);
      opt.value = z.name;
      if (z.name === split.zone) opt.selected = true;
      zone.append(opt);
    }

    // An empty first part means the domain itself, which is a valid choice.
    read = () => {
      const left = sub.value.trim().replace(/^\.+|\.+$/g, "");
      return left ? left + "." + zone.value : zone.value;
    };
    const suffix = el("div", "host-suffix");
    suffix.append(zone);
    field.append(sub, suffix);
    sub.oninput = () => redraw();
    zone.onchange = () => redraw();
    sub.onkeydown = (e) => { if (e.key === "Enter") apply(read()); };
  } else {
    // No account connected, or a hostname under no zone we know — an ngrok
    // custom domain, say. Keep it editable as plain text rather than
    // silently rewriting it into somebody else's domain.
    const input = el("input");
    input.type = "text";
    input.placeholder = "hooks.example.com";
    input.spellcheck = false;
    input.value = s.public_domain || "";
    read = () => input.value.trim();
    field.append(input);
    input.oninput = () => redraw();
    input.onkeydown = (e) => { if (e.key === "Enter") apply(read()); };
  }

  save.onclick = () => apply(read());
  clear.onclick = () => apply("", clear);

  const preview = el("div", "host-preview");
  function redraw() {
    const v = read();
    preview.textContent = v ? "https://" + v : t("sitepage.fixed.none");
    preview.classList.toggle("empty", !v);
  }
  redraw();

  const controls = el("div", "host-controls");
  controls.append(field, save, clear);
  const wrap = el("div", "host-field");
  wrap.append(controls, preview);
  pub.append(stackedRow(t("sitepage.fixed"), t("sitepage.fixed.body"), wrap));
  page.append(pub);

  /* Requests and health, the same panel the card can open */
  const mon = section(t("site.monitor"), t("sitepage.monitor.body"));
  mon.append(renderMonitor(s));
  page.append(mon);

  /* Removal */
  const danger = section(t("sitepage.danger"), t("sitepage.danger.body"));
  danger.classList.add("sitepage-danger");
  const del = el("button", "btn danger small", pendingDelete === s.id ? t("site.confirm") : t("site.remove"));
  del.onclick = async () => {
    if (pendingDelete !== s.id) {
      pendingDelete = s.id;
      await refreshSites();
      setTimeout(() => { if (pendingDelete === s.id) { pendingDelete = null; refreshSites(); } }, 4000);
      return;
    }
    pendingDelete = null;
    await withBusy(del, async () => {
      try {
        await invoke("remove_site", { id: s.id });
        toast(`Removed ${s.domain}`);
        openSiteId = null;
        showView("sites");
      } catch (e) { toast(String(e)); }
      await refreshAll();
    });
  };
  danger.append(del);
  page.append(danger);

  return page;
}

/* A titled block on the site page. */
function section(title, body) {
  const box = el("div", "card sitepage-section");
  const head = el("div", "sitepage-section-head");
  head.append(el("h3", null, title));
  if (body) head.append(el("p", "muted", body));
  box.append(head);
  return box;
}

/* A setting whose control needs the full width of the card rather than a
 * corner of it — a field with its own parts, rather than a single switch. */
function stackedRow(label, body, control) {
  const r = el("div", "setting-row stacked");
  const text = el("div");
  text.append(el("strong", null, label));
  if (body) text.append(el("span", "muted", body));
  r.append(text, control);
  return r;
}

/* One labelled setting inside a section. */
function row(label, body, ...controls) {
  const r = el("div", "setting-row");
  const text = el("div");
  text.append(el("strong", null, label));
  if (body) text.append(el("span", "muted", body));
  const actions = el("div", "row-actions");
  for (const c of controls) if (c) actions.append(c);
  r.append(text, actions);
  return r;
}

function runToggle(s) {
  const wrap = el("label", "switch");
  const cb = el("input");
  cb.type = "checkbox";
  cb.checked = s.run_state.status === "running" || s.run_state.status === "starting";
  cb.onchange = () => withBusySwitch(wrap, cb, async () => {
    try {
      await invoke("set_run", { id: s.id, on: cb.checked });
      toast(cb.checked ? t("toast.runOn") : t("toast.runOff"));
    } catch (e) { cb.checked = !cb.checked; toast(String(e)); }
    finally { await refreshSites(); }
  });
  const label = { running: t("site.running"), starting: t("site.starting"), failed: t("site.failed"), stopped: t("site.run") }[s.run_state.status];
  wrap.append(cb, el("span", "track"), el("span", null, label));
  if (s.run_state.status === "failed") wrap.classList.add("run-failed");
  return wrap;
}

function tunnelToggle(s) {
  const wrap = el("label", "switch");
  const cb = el("input");
  cb.type = "checkbox";
  cb.checked = s.tunnel;
  cb.onchange = () => withBusySwitch(wrap, cb, async () => {
    try {
      await invoke("set_tunnel", { id: s.id, on: cb.checked });
      toast(cb.checked ? t("toast.tunnelOn") : t("toast.tunnelOff"));
    } catch (e) { cb.checked = !cb.checked; toast(String(e)); }
    finally { await refreshSites(); }
  });
  wrap.append(cb, el("span", "track"), el("span", null, t("site.public")));
  return wrap;
}

function renderTunnelState(s) {
  const wrap = el("div", "tunnel-info");
  // Named `tun`, not `t`: `t` is the translation function, and shadowing it
  // here threw "t is not a function" the moment a public URL appeared —
  // which blanked the whole site list mid-render.
  const tun = s.tunnel_state;

  if (tun.status === "running" && tun.url) {
    const link = el("button", "tunnel-url", tun.url.replace("https://", ""));
    link.onclick = () => invoke("open_url", { url: tun.url });
    const copy = el("button", "btn ghost small", t("site.copy"));
    copy.onclick = async () => {
      await navigator.clipboard.writeText(tun.url);
      toast(t("toast.copied"));
    };
    wrap.append(link, copy);
  } else if (tun.status === "starting") {
    wrap.append(el("span", "spinner"), el("span", "muted", t("toast.tunnelOn")));
  } else if (tun.status === "failed") {
    wrap.append(el("span", "error", tun.error || t("site.failed")));
  }
  return wrap;
}

function renderMonitor(s) {
  const panel = el("div", "monitor");

  const head = el("div", "monitor-head");
  const health = el("span", "muted", "Checking…");
  head.append(el("strong", null, t("monitor.health")), health);
  panel.append(head);

  invoke("site_health", { id: s.id })
    .then((h) => {
      health.className = h.ok ? "hstat up" : "hstat down";
      health.textContent = `${h.ok ? t("monitor.up") : t("monitor.down")} · ${h.detail} · ${h.ms} ms`;
    })
    .catch((e) => { health.className = "hstat down"; health.textContent = String(e); });

  panel.append(el("div", "monitor-head", t("monitor.requests")));
  const table = el("div", "reqs");
  table.append(el("div", "muted", t("monitor.loading")));
  panel.append(table);

  invoke("site_requests", { id: s.id, limit: 15 }).then((rows) => {
    table.replaceChildren();
    if (!rows.length) {
      table.append(el("div", "muted", t("monitor.noRequests")));
      return;
    }
    for (const r of rows) {
      const row = el("div", "req");
      row.append(
        el("span", "method", r.method),
        el("span", "code-" + Math.floor(r.status / 100), String(r.status)),
        el("span", "path mono", r.path),
        el("span", "muted dur", r.duration_ms.toFixed(1) + " ms"),
        el("span", r.via_tunnel ? "src tunnel" : "src", r.via_tunnel ? t("monitor.public") : t("monitor.local"))
      );
      row.title = r.remote ? "from " + r.remote : "";
      table.append(row);
    }
  });

  if (s.managed) {
    panel.append(el("div", "monitor-head", t("monitor.devlog")));
    const pre = el("pre", "devlog", t("monitor.loading"));
    panel.append(pre);
    invoke("site_dev_log", { id: s.id, lines: 20 }).then((t) => {
      pre.textContent = t.trim() || t("monitor.noOutput");
      pre.scrollTop = pre.scrollHeight;
    });
  }
  return panel;
}

function schedulePoll() {
  clearTimeout(pollTimer);
  const busy = sites.some(
    (s) => (s.tunnel && s.tunnel_state.status !== "running") ||
           (s.managed && s.run_state.status === "starting")
  );
  if (busy) pollTimer = setTimeout(refreshSites, 1200);
  else if (openSiteId) pollTimer = setTimeout(refreshSites, 4000);
}

/* ───────────────────────── add a site ───────────────────────── */
function setTestState(cls, html) {
  const box = $("test-result");
  box.className = "testresult " + cls;
  box.innerHTML = html;
  box.classList.remove("hidden");
}
function resetTest() {
  testedTarget = null;
  $("test-result").classList.add("hidden");
}
$("target").addEventListener("input", resetTest);

$("show-add").onclick = () => {
  $("add-panel").classList.remove("hidden");
  $("show-add").classList.add("hidden");
  $("domain").focus();
};
$("cancel-add").onclick = () => {
  $("add-panel").classList.add("hidden");
  $("show-add").classList.remove("hidden");
  $("add-error").classList.add("hidden");
  resetTest();
};

$("test-btn").onclick = () => withBusy($("test-btn"), async () => {
  const target = $("target").value.trim();
  if (!target) return setTestState("fail", "Enter a target first.");
  setTestState("busy", '<span class="spinner"></span> Testing ' + target + '…');
  try {
    const r = await invoke("test_target", { target });
    if (r.ok) {
      testedTarget = target;
      const code = r.status ? '<span class="code">' + r.status + "</span> · " : "";
      setTestState("pass", "&#10003; " + code + r.message + " (" + r.elapsed_ms + " ms)");
    } else {
      resetTest();
      setTestState("fail", "&#10007; " + r.message);
    }
  } catch (e) { resetTest(); setTestState("fail", String(e)); }
});

$("add-form").onsubmit = async (e) => {
  e.preventDefault();
  $("add-error").classList.add("hidden");
  const btn = $("add-btn");
  const target = $("target").value.trim();
  await withBusy(btn, async () => {
  try {
    // Test on the way in, so the button is never dead but a broken target is
    // still caught. Skipped when this exact target already passed.
    if (testedTarget !== target) {
      setTestState("busy", '<span class="spinner"></span> ' + t("add.test") + "…");
      const r = await invoke("test_target", { target });
      if (!r.ok) {
        setTestState("fail", "&#10007; " + r.message);
        return;
      }
      testedTarget = target;
      const code = r.status ? '<span class="code">' + r.status + "</span> · " : "";
      setTestState("pass", "&#10003; " + code + r.message + " (" + r.elapsed_ms + " ms)");
    }

    // Sites are created on plain HTTP; SSL is a deliberate second step.
    await invoke("add_site", {
      domain: $("domain").value,
      target,
      ssl: false,
      spa: $("spa").checked,
    });
    $("domain").value = "";
    $("target").value = "";
    resetTest();
    $("cancel-add").click();
    toast(t("toast.added"));
    await refreshAll();
  } catch (err) {
    $("add-error").textContent = String(err);
    $("add-error").classList.remove("hidden");
  }
  });
};

/* ───────────────────────── public URL provider ───────────────────────── */
async function refreshNgrok() {
  let st;
  try { st = await invoke("ngrok_status"); }
  catch { return; }

  $("ngrok-detail").textContent = st.detail;
  // These are install instructions, so they go once ngrok is on the machine —
  // telling someone to `brew install ngrok` under the words "Installed" was
  // the kind of detail that makes a panel feel untrustworthy.
  $("ngrok-steps").classList.toggle("hidden", st.installed);

  // These key off token_stored, not configured. A token in ngrok's own config
  // makes the agent work but is not ours to remove — keying the field off
  // `configured` hid the input and offered a Disconnect with nothing behind
  // it, leaving no way to enter a token at all.
  $("ngrok-token").classList.toggle("hidden", st.token_stored);
  $("ngrok-connect").classList.toggle("hidden", st.token_stored);
  $("ngrok-disconnect").classList.toggle("hidden", !st.token_stored);
  $("ngrok-token").disabled = !st.installed;
  $("ngrok-connect").disabled = !st.installed;

  // Selecting ngrok is pointless until it can actually run.
  const ngrokBtn = document.querySelector('#provider-seg [data-provider="ngrok"]');
  ngrokBtn.disabled = !(st.installed && st.configured);
}

$("ngrok-token-link").onclick = (e) => {
  e.preventDefault();
  invoke("open_url", { url: "https://dashboard.ngrok.com/get-started/your-authtoken" });
};

$("ngrok-connect").onclick = async () => {
  const field = $("ngrok-token");
  const token = field.value.trim();
  if (!token) { toast(t("ngrok.needToken")); return; }

  // Connecting really does open a tunnel — the only way to find out whether
  // ngrok accepts the token — so this one is genuinely slow.
  await withBusy($("ngrok-connect"), async () => {
    try {
      await invoke("connect_ngrok", { token });
      field.value = "";
      toast(t("ngrok.connected"));
    } catch (e) { toast(String(e)); }
    finally { await refreshNgrok(); }
  });
};

$("ngrok-disconnect").onclick = () => withBusy($("ngrok-disconnect"), async () => {
  try { await invoke("disconnect_ngrok"); toast(t("ngrok.disconnected")); }
  catch (e) { toast(String(e)); }
  await refreshNgrok();
});

for (const b of $$("#provider-seg button")) {
  b.onclick = async () => {
    $$("#provider-seg button").forEach((x) => x.classList.toggle("active", x === b));
    try { await invoke("set_tunnel_provider", { provider: b.dataset.provider }); }
    catch (e) { toast(String(e)); }
  };
}

$("ngrok-signup").onclick = (e) => {
  e.preventDefault();
  invoke("open_url", { url: "https://dashboard.ngrok.com/signup" });
};


/* ───────────────────────── Cloudflare account ─────────────────────────

   A token here is what turns a throwaway trycloudflare.com name into a
   hostname on a domain the user owns. It is stored in the keychain by the
   Rust side; nothing on this page ever holds it beyond the input event. */
let cfZones = [];

async function refreshCloudflare() {
  let st;
  try { st = await invoke("cloudflare_status"); }
  catch { return; }

  $("cf-detail").textContent = st.detail;
  $("cf-token").classList.toggle("hidden", st.connected);
  $("cf-connect").classList.toggle("hidden", st.connected);
  $("cf-disconnect").classList.toggle("hidden", !st.connected);

  // Kept for the per-site hostname field, which offers these as a dropdown
  // rather than asking someone to retype a domain they already own.
  cfZones = st.zones || [];

  const zones = $("cf-zones");
  zones.textContent = "";
  for (const z of cfZones) zones.append(el("span", "zone-chip", z.name));
  return st;
}

$("cf-tokens-link").onclick = (e) => {
  e.preventDefault();
  invoke("open_url", { url: "https://dash.cloudflare.com/profile/api-tokens" });
};

$("cf-connect").onclick = async () => {
  const field = $("cf-token");
  const token = field.value.trim();
  if (!token) { toast(t("cf.needToken")); return; }

  await withBusy($("cf-connect"), async () => {
    try {
      await invoke("connect_cloudflare", { token });
      // Clear it the moment it is accepted — the keychain has it now, and a
      // token sitting in a DOM node is one screenshot away from being shared.
      field.value = "";
      toast(t("cf.connected"));
    } catch (e) { toast(String(e)); }
    finally { await refreshCloudflare(); }
  });
};

$("cf-disconnect").onclick = () => withBusy($("cf-disconnect"), async () => {
  try { await invoke("disconnect_cloudflare"); toast(t("cf.disconnected")); }
  catch (e) { toast(String(e)); }
  await refreshCloudflare();
});

/* ───────────────────────── logs ───────────────────────── */
let openLog = null;

function humanAge(seconds) {
  if (seconds === null || seconds === undefined) return "";
  if (seconds < 60) return seconds + "s ago";
  if (seconds < 3600) return Math.floor(seconds / 60) + "m ago";
  if (seconds < 86400) return Math.floor(seconds / 3600) + "h ago";
  return Math.floor(seconds / 86400) + "d ago";
}

function humanSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1048576) return (bytes / 1024).toFixed(0) + " KB";
  return (bytes / 1048576).toFixed(1) + " MB";
}

async function refreshLogs() {
  const files = await invoke("logs");
  const list = $("log-list");
  list.replaceChildren();

  if (!files.length) {
    list.append(el("li", "muted", t("logs.empty")));
    $("log-body").textContent = "";
    return;
  }

  // Default to the most recently written, which is nearly always the one you
  // came here to read.
  if (!openLog || !files.some((f) => f.name === openLog)) openLog = files[0].name;

  for (const f of files) {
    const li = el("li");
    const b = el("button", "log-item" + (f.name === openLog ? " active" : ""));
    b.append(
      document.createTextNode(f.label),
      el("small", null, `${humanSize(f.bytes)} · ${humanAge(f.age)}`)
    );
    b.onclick = async () => { openLog = f.name; await refreshLogs(); };
    li.append(b);
    list.append(li);
  }

  const body = $("log-body");
  body.textContent = await invoke("read_log", { name: openLog, lines: 400 });
  body.scrollTop = body.scrollHeight;
}

$("logs-refresh").onclick = () => refreshLogs().catch((e) => toast(String(e)));

function humanFreed(bytes) {
  return bytes > 1048576 ? (bytes / 1048576).toFixed(1) + " MB" : Math.round(bytes / 1024) + " KB";
}

$("logs-clear").onclick = () => withBusy($("logs-clear"), async () => {
  if (!openLog) return;
  const freed = await invoke("clear_logs", { name: openLog });
  toast(`${t("logs.cleared")} · ${humanFreed(freed)}`);
  await refreshLogs();
});

// Two-step, like the other destructive actions.
$("logs-clear-all").onclick = async (e) => {
  if (e.target.textContent !== t("logs.confirm")) {
    e.target.textContent = t("logs.confirm");
    setTimeout(() => (e.target.textContent = t("logs.clearAll")), 4000);
    return;
  }
  e.target.textContent = t("logs.clearAll");
  await withBusy(e.target, async () => {
    const freed = await invoke("clear_logs", { name: null });
    toast(`${t("logs.cleared")} · ${humanFreed(freed)}`);
    await refreshLogs();
  });
};

/* ───────────────────────── settings ───────────────────────── */
async function refreshSettings() {
  const s = await invoke("settings");
  currentMode = s.mode;
  applyTheme(s.theme);
  applyLanguage(s.language);

  describeAccessStep();
  $("about-version").textContent = "v" + s.app_version;
  currentProvider = s.tunnel_provider;
  $$("#provider-seg button").forEach((b) =>
    b.classList.toggle("active", b.dataset.provider === s.tunnel_provider));
  refreshNgrok().catch(() => {});
  refreshCloudflare().catch(() => {});
  $("side-version").textContent = "v" + s.app_version;
  $("status-mode").textContent = s.mode === "system" ? t("status.mode.system") : t("status.mode.standalone");

  $$(".mode").forEach((b) => b.classList.toggle("active", b.dataset.mode === currentMode));
  const standalone = currentMode === "standalone";
  $("domain").placeholder = standalone ? "myapp.localhost" : "mysite.test";
  $("domain-hint").innerHTML = standalone
    ? "A <code>.localhost</code> name needs no setup at all; anything else needs one hosts entry."
    : "<code>.test</code>, <code>.localhost</code>, or any domain you own.";
}

$$(".mode").forEach((b) => {
  b.onclick = async () => {
    if (b.dataset.mode === currentMode) return;
    const all = $$(".mode");
    all.forEach((x) => (x.disabled = true));
    toast(b.dataset.mode === "system"
      ? "Switching to System mode — you'll be asked for your password once"
      : "Switching to Standalone mode…");
    await withBusy(b, async () => {
      try { await invoke("set_mode", { mode: b.dataset.mode }); toast(t("toast.modeSwitched")); }
      catch (e) { toast(String(e)); }
      finally { all.forEach((x) => (x.disabled = false)); await refreshAll(); }
    });
  };
});

/// Fill the About panel with what is installed. Versions only — Portico does
/// not check third-party tools for updates; they are often managed by Homebrew
/// and it is not our place to police them.
async function refreshVersions() {
  let r;
  try { r = await invoke("check_updates"); }
  catch { return; }

  const find = (n) => r.tools.find((t) => t.name === n)?.detail ?? "—";
  $("about-serving").textContent = find("caddy");
  $("about-tunnel").textContent = find("cloudflared");
}

$("uninstall").onclick = async (e) => {
  if (e.target.textContent !== t("settings.remove.confirm")) {
    e.target.textContent = t("settings.remove.confirm");
    setTimeout(() => (e.target.textContent = t("settings.remove")), 4000);
    return;
  }
  e.target.textContent = t("settings.remove");
  await withBusy(e.target, async () => {
    try { await invoke("run_uninstall"); toast(t("toast.removed")); }
    catch (err) { toast(String(err)); }
    await refreshAll();
  });
};

$("recheck").onclick = () => refreshDoctor();

$("search").addEventListener("input", (e) => {
  searchTerm = e.target.value;
  refreshSites();
});
// Escape clears the filter, as it does everywhere else on the platform.
$("search").addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.target.value = "";
    searchTerm = "";
    refreshSites();
  }
});

/* ───────────────────────── boot ───────────────────────── */
async function refreshAll() {
  await refreshSettings();
  await refreshDoctor();
  if (installed) {
    await refreshSites();
    refreshVersions().catch(() => {});
  } else {
    await loadToolStatus();
  }
}

applyLanguage("auto");   // something readable before settings arrive
resetOnboarding();
showView("sites");
refreshAll();
