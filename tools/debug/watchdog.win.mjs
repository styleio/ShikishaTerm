/**
 * Noticing that the app stopped answering, through a real copy of the app.
 *
 * This checkout's build (a debug build: it is the one that can be told to
 * hold its work loop still, SHIKISHA_DEBUG_STALL) in a folder of its own.
 * Checked:
 *   - a work loop held still past the limit is written down -- the hang file,
 *     the crash log, and a dump of the program's threads
 *   - when it moves again, that is written down with how long it was gone,
 *     and the board says so once
 *   - a run ended while it was held still (as a person ending a window that
 *     does not answer would) is told at the next start: the board's account
 *     of the last run says it had stopped answering, for how long, and where
 *     the dump is
 *   - the page throws nothing
 *
 *     cargo build
 *     node tools/debug/watchdog.win.mjs
 *
 * Takes about two minutes. Needs Windows, Node and Chrome. Nothing of a copy
 * somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-watchdog');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const PORT = 9489;
const TOKEN = 'watchdog-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-watchdog*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [path.join(APP, 'config'), WORK]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en',
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'One', id: 'one', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe'] }] }] }],
}, null, 2));
const start = (stall) => {
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX|OPENAI)/i.test(k)));
  env.LOCALAPPDATA = path.join(RUN, 'localappdata');
  if (stall) env.SHIKISHA_DEBUG_STALL = stall;
  spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
};
// The app's own files, wherever its data folder is
const find = (name) => {
  const walk = (dir) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) { const f = walk(p); if (f) return f; } else if (e.name === name) return p;
    }
    return null;
  };
  return walk(RUN);
};
const dumps = () => {
  const out = [];
  const walk = (dir) => { for (const e of fs.readdirSync(dir, { withFileTypes: true })) { const p = path.join(dir, e.name); if (e.isDirectory()) walk(p); else if (e.name.endsWith('.dmp')) out.push(p); } };
  walk(RUN);
  return out;
};
const jar = new Map();
const send = async (url) => {
  const r = await fetch(BASE + url, { redirect: 'manual', headers: { Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') }, signal: AbortSignal.timeout(15000) });
  for (const kv of r.headers.getSetCookie().map((c) => c.split(';')[0])) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return r;
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};

let chrome;
try {
  // Held still for 25 seconds, 8 seconds in
  start('8:25');
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500, 'the board', 60000);
  chrome = await startChrome();
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  await until(() => chrome.run(`typeof S !== "undefined" && !!S`), 'the board in the browser', 30000);
  // A toast is up for a few seconds: what it said is kept as it comes
  await chrome.run(`(() => { window.__toasts = []; const m = document.getElementById("toastmsg");
    new MutationObserver(() => window.__toasts.push(m.textContent)).observe(m, {childList: true, characterData: true, subtree: true}); })()`);
  // The work part's account in the hang file, which holds one for each part
const work = () => { const f = find('hang.json'); return f ? JSON.parse(fs.readFileSync(f, 'utf8')).find((h) => h.part === 'work') : null; };
  const hang = await until(() => { const h = work(); return h && h.dump && h; }, 'the hang to be written down with its dump', 40000).catch(() => work());
  check(hang && hang.part === 'work' && !hang.back && hang.secs >= 20, `a work loop held still is written down (${JSON.stringify(hang)})`);
  check(hang && hang.dump && !hang.dump.startsWith(APP) && fs.existsSync(hang.dump) && fs.statSync(hang.dump).size > 10000 && fs.readFileSync(hang.dump).subarray(0, 4).toString() === "MDMP", `with a dump of the threads, kept outside the program's folder (${hang && hang.dump})`);
  const back = await until(() => { const h = work(); return h && h.back && h; }, 'it to come back', 40000).catch(() => null);
  check(back && back.secs >= 20 && back.secs <= 40, `its coming back is written down, with how long (${back && back.secs} s)`);
  const toasts = await until(() => chrome.run(`window.__toasts.find(t => /did not respond for/.test(t)) || ""`), 'the board to say so', 20000).catch(() => '');
  check(/app did not respond for \d+ seconds/.test(toasts), `the board says it once (${toasts})`);
  const log = fs.readFileSync(find('crash.log'), 'utf8');
  check(/has not answered for/.test(log) && /answers again after/.test(log), 'the crash log has both');
  check(chrome.thrown.length === 0, `the page threw nothing (${chrome.thrown.join(' | ')})`);
  chrome.stop();
  chrome = null;

  // Ended while held still: told at the next start
  stopApp();
  await sleep(1500);
  const before = dumps().length;
  start('6:120');
  await until(() => { const h = work(); return h && !h.back && h.secs >= 20 && h.dump && h; }, 'the second hang', 60000);
  check(dumps().length === before + 1, 'the second hang has its own dump');
  check(!fs.readdirSync(path.dirname(dumps()[0])).some((n) => n.endsWith('.part')), 'nothing is left half written');
  await sleep(4000);
  stopApp();
  await sleep(1500);
  start('');
  jar.clear();
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500, 'the board again', 60000);
  chrome = await startChrome();
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  const told = await until(() => chrome.run(`(S && S.last_exit) ? JSON.stringify(S.last_exit) : ""`), 'the account of the last run', 60000).catch(() => '');
  const x = told ? JSON.parse(told) : {};
  check(x.hung >= 20, `the next start says the last run had stopped answering, and for how long (${x.hung} s)`);
  check(x.dump && /hang-work-/.test(x.dump), `and where its dump is (${x.dump})`);
  const bar = await until(() => chrome.run(`(() => { const b = document.getElementById("crashbar"); return b && !b.hidden ? b.textContent : ""; })()`), 'the bar', 20000).catch(() => '');
  check(/It stopped responding last time/.test(bar) && /stopped responding \d+ seconds before it ended/.test(bar) && /Record of where it stopped/.test(bar), `the bar says both (${bar.slice(0, 200)})`);
  if (process.env.SHOT) {
    const shot = await chrome.send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(process.env.SHOT, 'watchdog-bar.png'), Buffer.from(shot.data, 'base64'));
  }
  check(chrome.thrown.length === 0, `the page threw nothing (${chrome.thrown.join(' | ')})`);
  chrome.stop();
  chrome = null;

  // A native crash: written down as a dump before Windows takes it, and told
  // at the next start
  stopApp();
  await sleep(1500);
  const crashesBefore = dumps().filter((d) => /crash-/.test(d)).length;
  start('6:0');
  const crashed = await until(() => dumps().filter((d) => /crash-/.test(d)).length > crashesBefore, 'the crash dump', 60000).catch(() => false);
  check(crashed, 'a native crash is written down as a dump');
  const crashDump = dumps().filter((d) => /crash-/.test(d)).pop() || '';
  check(crashDump && fs.readFileSync(crashDump).subarray(0, 4).toString() === 'MDMP', `the dump is one (${crashDump})`);
  await until(() => !ps('-Command', `@(Get-Process | Where-Object { $_.Path -like '${RUN}\\*' -and $_.Name -eq 'SHIKISHA-TERM' }).Count`).stdout.trim().startsWith('1'), 'the crashed run to go', 30000).catch(() => {});
  stopApp();
  await sleep(1500);
  start('');
  jar.clear();
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500, 'the board after the crash', 60000);
  chrome = await startChrome();
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  const after = await until(() => chrome.run(`(S && S.last_exit) ? JSON.stringify(S.last_exit) : ""`), 'the account of the crash', 60000).catch(() => '');
  const c = after ? JSON.parse(after) : {};
  check(/crash-/.test(c.dump || ''), `the next start names the crash's dump (${c.dump}, code ${c.code})`);
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (chrome) chrome.stop();
  if (!process.env.KEEP) stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
