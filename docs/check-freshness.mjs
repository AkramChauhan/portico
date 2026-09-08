#!/usr/bin/env node
/**
 * Living Docs freshness checker.
 *
 * Each doc in docs/ declares, in <head>:
 *   <meta name="doc:reviewed" content="YYYY-MM-DD">
 *   <meta name="doc:sources"  content="src/lib/lean/**, db/schema.sql">
 *
 * This compares the reviewed date against the last modification of the code
 * each doc claims to describe, and reports anything that has fallen behind.
 *
 * Usage:  node docs/check-freshness.mjs [--quiet]
 * Exit:   0 = all fresh, 1 = at least one stale doc
 */
import { readdirSync, readFileSync, statSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const DOCS = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(DOCS, '..');
const quiet = process.argv.includes('--quiet');

const C = process.stdout.isTTY
  ? { r:'\x1b[31m', y:'\x1b[33m', g:'\x1b[32m', d:'\x1b[2m', b:'\x1b[1m', x:'\x1b[0m' }
  : { r:'', y:'', g:'', d:'', b:'', x:'' };

const meta = (html, name) =>
  (html.match(new RegExp(`<meta\\s+name=["']${name}["']\\s+content=["']([^"']*)["']`, 'i')) || [])[1] || '';

let hasGit = false;
try {
  execFileSync('git', ['rev-parse', '--is-inside-work-tree'], { cwd: ROOT, stdio: 'ignore' });
  hasGit = true;
} catch { /* not a git repo — fall back to filesystem mtime */ }

/** Last modification time of a path (or the newest match under a glob-ish prefix). */
function lastChanged(pattern) {
  const clean = pattern.trim();
  if (!clean || clean.startsWith('(')) return null;          // e.g. "(none yet)"

  if (hasGit) {
    try {
      const out = execFileSync('git', ['log', '-1', '--format=%cI', '--', clean],
        { cwd: ROOT, encoding: 'utf8' }).trim();
      if (out) return new Date(out);
    } catch { /* fall through to mtime */ }
  }

  // Filesystem fallback. Supports a trailing /** or a plain path.
  const base = clean.replace(/\/?\*\*?$/, '');
  const abs = join(ROOT, base);
  if (!existsSync(abs)) return undefined;                     // declared but missing
  let newest = 0;
  const walk = (p) => {
    const st = statSync(p);
    if (st.isDirectory()) for (const e of readdirSync(p)) walk(join(p, e));
    else newest = Math.max(newest, st.mtimeMs);
  };
  walk(abs);
  return new Date(newest);
}

const docs = readdirSync(DOCS).filter(f => f.endsWith('.html'));
let stale = 0, missing = 0, checked = 0, undeclared = 0;

console.log(`${C.b}Docs freshness${C.x} ${C.d}(${hasGit ? 'git history' : 'file mtime'})${C.x}\n`);

for (const file of docs.sort()) {
  const html = readFileSync(join(DOCS, file), 'utf8');
  const reviewed = meta(html, 'doc:reviewed');
  const sources = meta(html, 'doc:sources').split(',').map(s => s.trim()).filter(Boolean);
  const status = meta(html, 'doc:status') || '—';

  if (!reviewed) {
    console.log(`${C.r}✗${C.x} ${file.padEnd(26)} ${C.r}no doc:reviewed date${C.x}`);
    stale++; continue;
  }
  if (!sources.length) {
    if (!quiet) console.log(`${C.d}○ ${file.padEnd(26)} ${status} · reviewed ${reviewed} · documents no code yet${C.x}`);
    undeclared++; continue;
  }

  const rev = new Date(reviewed + 'T23:59:59Z');
  const behind = [];
  for (const src of sources) {
    const t = lastChanged(src);
    if (t === undefined) { console.log(`${C.y}!${C.x} ${file.padEnd(26)} declares a path that does not exist: ${src}`); missing++; continue; }
    if (t && t > rev) behind.push(`${src} (changed ${t.toISOString().slice(0,10)})`);
  }
  checked++;
  if (behind.length) {
    stale++;
    console.log(`${C.r}✗ ${file.padEnd(26)} STALE${C.x} · reviewed ${reviewed}`);
    behind.forEach(b => console.log(`   ${C.d}└─ behind${C.x} ${b}`));
  } else if (!quiet) {
    console.log(`${C.g}✓${C.x} ${file.padEnd(26)} ${status} · reviewed ${reviewed}`);
  }
}

console.log(`\n${C.d}${docs.length} docs · ${checked} tracked · ${undeclared} not yet tracking code · ${missing} bad paths${C.x}`);
if (stale) {
  console.log(`${C.r}${C.b}${stale} document(s) need review.${C.x} Update the content, then bump doc:reviewed.`);
  process.exit(1);
}
console.log(`${C.g}All documents current.${C.x}`);
