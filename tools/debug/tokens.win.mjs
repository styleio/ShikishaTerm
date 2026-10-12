/**
 * Tokens and cost: the settings card counts what Claude Code and Codex used
 * from their records on this PC, prices it, and reads only what was added
 * when asked again.
 *
 * This checkout's build in a folder of its own, with a home folder of its
 * own holding made-up records: a Claude Code conversation (one call written
 * twice while it streamed, one in fast mode) and a Codex one. Checked
 * through the settings page at a phone's width:
 *   - the card reads the records and shows the calls, the tokens and the
 *     cost at API prices, worked out here by hand from the price lists
 *   - a call written twice is counted once, and so is a Codex count said
 *     again when only the plan's numbers changed
 *   - an archived Codex conversation is counted, its cache writes priced as
 *     writes
 *   - by model, by folder and the latest conversations are listed
 *   - "Read again" after a record grew counts the new call, and only it
 *   - choosing one AI shows that AI's alone
 *   - the record is kept in the app's data folder, and the page throws nothing
 *
 *     cargo build
 *     node tools/debug/tokens.win.mjs
 *
 * Takes about a minute. Needs Windows, Node and Chrome. Nothing of a copy
 * somebody is using, nor this PC's own AI folders, is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-tokens');
const APP = path.join(RUN, 'app');
const HOME = path.join(RUN, 'home');
const WORK = path.join(RUN, 'work');
const PORT = 9485;
const TOKEN = 'tokens-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-tokens*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
const claudeDir = path.join(HOME, '.claude', 'projects', 'C--work');
const codexDir = path.join(HOME, '.codex', 'sessions', '2026', '10', '12');
const archivedDir = path.join(HOME, '.codex', 'archived_sessions');
for (const d of [path.join(APP, 'config'), WORK, claudeDir, codexDir, archivedDir]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// Made-up records, made now: today, by any clock
const now = new Date().toISOString();
const claude = (id, usage, model = 'claude-opus-5-5') => JSON.stringify({
  type: 'assistant', timestamp: now, cwd: WORK, sessionId: 'claude-conv-1', requestId: 'req-' + id,
  message: { id, model, usage },
}) + '\n';
const claudeFile = path.join(claudeDir, 'claude-conv-1.jsonl');
fs.writeFileSync(claudeFile,
  JSON.stringify({ type: 'user', timestamp: now, message: { role: 'user', content: 'fix the login page' } }) + '\n'
  // Written twice while it streamed: the second, larger, is the call
  + claude('msg-a', { input_tokens: 1000, cache_read_input_tokens: 2000, cache_creation_input_tokens: 3000, cache_creation: { ephemeral_1h_input_tokens: 1000 }, output_tokens: 10 })
  + claude('msg-a', { input_tokens: 1000, cache_read_input_tokens: 2000, cache_creation_input_tokens: 3000, cache_creation: { ephemeral_1h_input_tokens: 1000 }, output_tokens: 500 })
  + claude('msg-b', { input_tokens: 1000, output_tokens: 1000, speed: 'fast' }));
const codexFile = path.join(codexDir, 'rollout-2026-10-12T00-00-00-codex-conv-1.jsonl');
fs.writeFileSync(codexFile, [
  { timestamp: now, type: 'session_meta', payload: { id: 'codex-conv-1', cwd: WORK } },
  { timestamp: now, type: 'turn_context', payload: { model: 'gpt-5.5', cwd: WORK } },
  { timestamp: now, type: 'event_msg', payload: { type: 'token_count', info: {
    total_token_usage: { input_tokens: 10000, cached_input_tokens: 4000, output_tokens: 2000, reasoning_output_tokens: 100 },
    last_token_usage: { input_tokens: 10000, cached_input_tokens: 4000, output_tokens: 2000, reasoning_output_tokens: 100 } } } },
  { timestamp: now, type: 'event_msg', payload: { type: 'token_count', info: null } },
  // Said again when only the plan changed: the same numbers, a new time
  { timestamp: new Date(Date.now() + 5000).toISOString(), type: 'event_msg', payload: { type: 'token_count', info: {
    total_token_usage: { input_tokens: 10000, cached_input_tokens: 4000, output_tokens: 2000, reasoning_output_tokens: 100 },
    last_token_usage: { input_tokens: 10000, cached_input_tokens: 4000, output_tokens: 2000, reasoning_output_tokens: 100 } } } },
].map((l) => JSON.stringify(l)).join('\n') + '\n');
fs.writeFileSync(path.join(archivedDir, 'rollout-2026-10-12T00-00-00-codex-conv-2.jsonl'), [
  { timestamp: now, type: 'session_meta', payload: { id: 'codex-conv-2', cwd: WORK } },
  { timestamp: now, type: 'turn_context', payload: { model: 'gpt-6-astra', cwd: WORK } },
  { timestamp: now, type: 'event_msg', payload: { type: 'token_count', info: {
    total_token_usage: { input_tokens: 1000, cached_input_tokens: 0, cache_write_input_tokens: 200, output_tokens: 100, reasoning_output_tokens: 0 },
    last_token_usage: { input_tokens: 1000, cached_input_tokens: 0, cache_write_input_tokens: 200, output_tokens: 100, reasoning_output_tokens: 0 } } } },
].map((l) => JSON.stringify(l)).join('\n') + '\n');
// By hand, from the price lists ($ per million tokens):
//   Opus 5.5: 1000 in x 4 + 2000 cache read x 0.2 + 2000 five-minute writes x 5
//             + 1000 hour writes x 8 + 500 out x 20                 = 0.0324
//   Opus 5.5 fast: (1000 x 4 + 1000 x 20) x 2                       = 0.048
//   GPT-5.5: 6000 in x 5 + 4000 cached x 0.5 + 2000 out x 30        = 0.092
//   GPT-6 Astra (archived): 800 in x 10 + 200 cache writes x 12.5
//             + 100 out x 50                                        = 0.0155
const claudeCost = 0.0324 + 0.048;
const codexCost = 0.092 + 0.0155;

fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en',
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'One', id: 'one', folders: [{ cwd: WORK, tabs: [{ name: 'shell', command: ['cmd.exe'] }] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX|OPENAI)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.USERPROFILE = HOME;
env.HOME = HOME;
spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const jar = new Map();
const send = async (url, init = {}) => {
  const r = await fetch(BASE + url, { redirect: 'manual', ...init,
    headers: { ...(init.headers || {}), Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') },
    signal: AbortSignal.timeout(15000) });
  for (const kv of r.headers.getSetCookie().map((c) => c.split(';')[0])) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return r;
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(800); }
  throw new Error('timed out waiting for ' + what);
};
const near = (a, b) => Math.abs(a - b) < 1e-9;

let chrome;
try {
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500 && jar.has('rs'), 'the board', 60000);
  await send(`/cfg?t=${TOKEN}`);
  // Through the settings' own door, as the card asks
  const settled = async (q) => until(async () => {
    const r = await send('/api/tokens?' + q);
    const j = r.status === 200 && (await r.json());
    return j && j.looking && !j.looking.running && j.looking.ended && j;
  }, 'the records to be read', 60000);
  const first = await settled('days=7');
  const t = first.report.total;
  check(t.calls === 4, `four calls are counted, each written twice once (${t.calls})`);
  check(t.out === 500 + 1000 + 2000 + 100 && t.fresh_in === 1000 + 1000 + 6000 + 800 && t.cache_read === 2000 + 4000 && t.cache_write === 3000 + 200,
    `each kind of token is added up (${JSON.stringify({ fresh_in: t.fresh_in, cache_read: t.cache_read, cache_write: t.cache_write, out: t.out })})`);
  check(near(t.cost, claudeCost + codexCost), `the cost is what the price lists say, fast mode doubled (${t.cost})`);
  check(first.report.models.length === 3 && first.report.models.some((m) => m.name === 'gpt-5.5' && m.cli === 'codex') && first.report.models.some((m) => m.name === 'gpt-6-astra'),
    'by model lists every model, the archived conversation\'s too');
  check(first.report.folders.length === 1 && path.resolve(first.report.folders[0].name) === path.resolve(WORK), 'by folder names the folder they worked in');
  check(first.report.sessions.length === 3, 'every conversation is among the latest');
  check(fs.existsSync(path.join(APP, 'data', 'tokens.db')), 'the record is in the app\'s data folder');
  const one = (await (await send('/api/tokens?days=7&cli=codex')).json()).report.total;
  check(one.calls === 2 && near(one.cost, codexCost), `one AI alone is that AI's (${one.calls}, ${one.cost})`);

  // Grown: read again, only the new call is added
  fs.appendFileSync(claudeFile, claude('msg-c', { input_tokens: 0, output_tokens: 100000 }, 'claude-haiku-5-5'));
  const before = (await (await send('/api/tokens?days=7')).json()).looking.ended;
  await send('/api/tokens?days=7&again=1');
  const grown = await until(async () => {
    const j = await (await send('/api/tokens?days=7')).json();
    return !j.looking.running && j.looking.ended >= before && j.report.total.calls === 5 && j;
  }, 'the new call', 30000).catch(() => null);
  check(!!grown && near(grown.report.total.cost, claudeCost + codexCost + 0.05), `"Read again" counts the call added, and nothing twice (${grown && grown.report.total.cost})`);

  // The card, at a phone's width
  chrome = await startChrome();
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 420, height: 1400, deviceScaleFactor: 2, mobile: true });
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  await until(() => chrome.run(`typeof S !== "undefined" && !!S`), 'the board in the browser', 30000);
  await chrome.send('Page.navigate', { url: `${BASE}/cfg?section=tokens` });
  const shown = await until(() => chrome.run(`(() => { const s = document.querySelector(".tkstats"); return s ? s.innerText : ""; })()`), 'the card', 30000).catch(() => '');
  check(/\$0\.24/.test(shown) && /\b5\b/.test(shown), `the card shows the cost and the calls (${shown.replace(/\s+/g, ' ').slice(0, 160)})`);
  const lists = await chrome.run(`[...document.querySelectorAll(".tkhead")].map(h => h.textContent).join(" | ")`);
  check(/By model/.test(lists) && /By folder/.test(lists) && /Latest conversations/.test(lists) && /by day/.test(lists), `the card lists by day, model, folder and conversation (${lists})`);
  const bars = await chrome.run(`document.querySelectorAll(".tkbars i").length`);
  check(bars === 30, `one bar a day of the period (${bars})`);
  const wide = await chrome.run(`document.documentElement.scrollWidth <= innerWidth`);
  check(wide, 'nothing runs off the side at a phone\'s width');
  // A year: 365 bars, all within the card
  await chrome.run(`(() => { const s = document.querySelector(".tkctl select"); s.value = "365"; s.dispatchEvent(new Event("change")); })()`);
  const year = await until(() => chrome.run(`(() => { const b = document.querySelector(".tkbars"); if (!b || b.children.length !== 365) return null;
    const box = b.getBoundingClientRect(), last = b.lastElementChild.getBoundingClientRect();
    return {fits: last.right <= box.right + 0.5 && b.scrollWidth <= b.clientWidth + 1}; })()`), 'the year', 20000).catch(() => null);
  check(year && year.fits, 'a year of bars fits the card, the newest day in sight');
  if (process.env.SHOT) {
    const shot = await chrome.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
    fs.writeFileSync(path.join(process.env.SHOT, 'tokens-card.png'), Buffer.from(shot.data, 'base64'));
  }
  check(chrome.thrown.length === 0, `the page threw nothing (${chrome.thrown.join(' | ')})`);
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (chrome) chrome.stop();
  if (!process.env.KEEP) stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
