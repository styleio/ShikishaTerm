/**
 * A new worktree trusted by an AI CLI as its checkout is, through a real copy
 * of the app and the board at a phone's width.
 *
 * This checkout's build in a folder of its own with a home folder of its own:
 * Gemini CLI's list of trusted folders there trusts the project, and nothing
 * else. Worktrees are made the way the board's button makes them. Checked:
 *   - the first worktree puts the question up, naming Gemini CLI, the project
 *     and the worktree; nothing is written before the answer
 *   - yes is kept in the settings and writes the worktree into Gemini CLI's
 *     list as Gemini CLI writes it, the rest of the list left as it was
 *   - the next worktree is written with no question
 *   - the settings show the answer as the value it is, and "Do not write"
 *     there is kept; a worktree made after it is not written
 *   - a project Gemini CLI does not trust is not written, and asks nothing
 *   - the page throws nothing
 *
 *     cargo build
 *     node tools/debug/worktree-trust.win.mjs [--real]
 *
 * --real also starts the real Gemini CLI (signed in on this machine, its
 * updates turned off for the run) in the first worktree and checks it does
 * not ask whether to trust it -- and, as a control, that it does ask in a
 * worktree that was not written. Spends no turn. Needs Windows, Node, git and
 * Chrome; about a minute (two with --real). Nothing of a copy somebody is
 * using is read, written or stopped, and the person's own ~/.gemini is only
 * read for the sign-in with --real.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const REAL_GEMINI = process.argv.includes('--real');
const RUN = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-wtrust-'));
const INSTANCE = path.join(RUN, 'copy');
const APP = path.join(INSTANCE, 'app');
const HOME = path.join(RUN, 'home');
const PROJ = path.join(RUN, 'proj');
const OTHER = path.join(RUN, 'other');
const CONFIG = path.join(RUN, 'config.json');
const TRUSTED = path.join(HOME, '.gemini', 'trustedFolders.json');
const PORT = 9491;
const TOKEN = 'worktree-trust-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8', windowsHide: true });
const launcher = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(400); }
  throw new Error('timed out waiting for ' + what);
};
// A folder as Gemini CLI writes it on Windows
const key = (p) => fs.realpathSync.native(p).replace(/\\/g, '/').toLowerCase();
const listed = () => JSON.parse(fs.readFileSync(TRUSTED, 'utf8'));
// The settings the copy runs on, which the launcher writes into its folder
const settings = () => JSON.parse(fs.readFileSync(path.join(APP, 'config', 'config.json'), 'utf8'));

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
for (const d of [APP, path.join(HOME, '.gemini'), PROJ, OTHER]) fs.mkdirSync(d, { recursive: true });
for (const repo of [PROJ, OTHER]) {
  const git = (...a) => spawnSync('git', ['-c', 'user.name=Trust check', '-c', 'user.email=check@example.invalid', ...a], { cwd: repo, encoding: 'utf8' });
  git('init', '-q', '-b', 'main');
  fs.writeFileSync(path.join(repo, 'a.txt'), 'a\n');
  git('add', '.');
  git('commit', '-qm', 'first');
}
// The person's own rule for another folder, kept as it is
const before = { [key(PROJ)]: 'TRUST_FOLDER', 'z:/somewhere/else': 'DO_NOT_TRUST' };
fs.writeFileSync(TRUSTED, JSON.stringify(before, null, 2));
const geminiSettings = { general: { disableAutoUpdate: true, disableUpdateNag: true } };
if (REAL_GEMINI) {
  const real = path.join(os.homedir(), '.gemini');
  for (const f of ['google_accounts.json', 'installation_id', 'oauth_creds.json']) {
    try { fs.copyFileSync(path.join(real, f), path.join(HOME, '.gemini', f)); } catch {}
  }
  try { Object.assign(geminiSettings, JSON.parse(fs.readFileSync(path.join(real, 'settings.json'), 'utf8')), { general: geminiSettings.general }); } catch {}
}
fs.writeFileSync(path.join(HOME, '.gemini', 'settings.json'), JSON.stringify(geminiSettings, null, 2));
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en', keep_terminals: false, external_api: { access: 'user' },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'One', id: 'one', folders: [
    { cwd: PROJ, tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe'] }] },
    { cwd: OTHER, tabs: [{ name: 'shell2', id: 'shell2', command: ['cmd.exe'] }] },
  ] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX)/i.test(k) && !/^GEMINI_CLI/i.test(k)));
Object.assign(env, { USERPROFILE: HOME, HOME, LOCALAPPDATA: path.join(RUN, 'localappdata') });
const started = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', launcher,
  '-At', INSTANCE, '-Work', PROJ, '-Exe', exe, '-Config', CONFIG, '-Port', String(PORT), '-Cdp', '0'], { env, encoding: 'utf8', windowsHide: true });
if (started.status !== 0) die('launch failed: ' + started.stdout + started.stderr);
const pid = Number(/^pid=(\d+)/m.exec(started.stdout)?.[1]);

const jar = new Map();
const http = async (url, init = {}) => {
  const r = await fetch(BASE + url, { redirect: 'manual', ...init, headers: { ...(init.headers || {}), Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') }, signal: AbortSignal.timeout(15000) });
  for (const kv of r.headers.getSetCookie().map((c) => c.split(';')[0])) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return r;
};
// The settings screen's own requests, made from the settings page itself the
// way its buttons make them; the board is opened again after
const cfg = async (p, body) => {
  await chrome.send('Page.navigate', { url: `${BASE}/cfg?t=${TOKEN}` });
  await until(() => chrome.run('typeof settingsFetch === "function"'), 'the settings page', 30000);
  const asked = JSON.stringify(body ? { method: 'POST', json: body } : {});
  const got = await chrome.run(`settingsFetch(${JSON.stringify(p)}, ${asked}).then((r) => r.json())`);
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  await until(() => chrome.run('typeof S !== "undefined" && !!S && (S.tabs || []).length > 0'), 'the board again', 30000);
  return got;
};

let chrome;
try {
  await until(async () => (await http(`/?t=${TOKEN}`)).status < 500, 'the board', 60000);
  chrome = await startChrome();
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  await until(() => chrome.run(`typeof S !== "undefined" && !!S && (S.tabs || []).length > 0`), 'the board in the browser', 30000).catch(async (e) => { console.log(await chrome.run('location.href + " " + document.title + " " + document.body.innerText.slice(0, 300) + " S=" + (typeof S) + " THROWN=" + JSON.stringify(window.__thrown||null) + " " + (typeof S !== "undefined" && S ? JSON.stringify(Object.keys(S)).slice(0,300) : "")')); console.log("thrown:", chrome.thrown.join(" | ")); throw e; });
  const make = (from, branch) => chrome.run(`send(${JSON.stringify({ kind: 'branch', from, branch, base: 'main', make: true, carry: [], seq: Date.now() % 100000 })})`);
  // Where git says the worktree on that branch is, once it is there
  const made = (name) => until(() => {
    for (const repo of [PROJ, OTHER]) {
      const out = spawnSync('git', ['worktree', 'list', '--porcelain'], { cwd: repo, encoding: 'utf8' }).stdout.split(/\r?\n/);
      const i = out.indexOf('branch refs/heads/' + name);
      if (i > 0) return out.slice(0, i).reverse().find((l) => l.startsWith('worktree ')).slice('worktree '.length);
    }
    return null;
  }, `the worktree ${name}`, 60000);

  // The first worktree: the question, and nothing written before the answer
  await make(PROJ, 'feat-a');
  const a = await made('feat-a');
  const asked = await until(() => chrome.run(`(() => { const b = document.getElementById("sask"); return b && !b.hidden && (S.trust_asks || []).length ? b.querySelector(".vtitle").textContent + " | " + b.querySelector(".vsay").textContent + " | " + b.querySelector(".bwhere").textContent : ""; })()`), 'the question', 30000).catch(() => '');
  check(/Gemini CLI/.test(asked) && asked.toLowerCase().includes(path.basename(PROJ)) && /feat-a/.test(asked), `the question names Gemini CLI, the project and the worktree (${asked.slice(0, 160)})`);
  check(!(key(a) in listed()), 'nothing is written before the answer');
  // The worktree waits for the answer, so its tabs start with the yes written
  check(await chrome.run(`(S.making || []).some(m => m.name === "feat-a" && !m.error)`), 'the first worktree waits for the answer before it is done');
  if (process.env.SHOT) fs.writeFileSync(path.join(process.env.SHOT, 'worktree-trust-ask.png'), Buffer.from((await chrome.send('Page.captureScreenshot', { format: 'png' })).data, 'base64'));
  await chrome.run(`document.querySelector("#sask .go").click()`);
  await until(() => listed()[key(a)] === 'TRUST_FOLDER', 'the yes to be written', 20000).catch(() => {});
  const after = listed();
  check(after[key(a)] === 'TRUST_FOLDER', 'yes writes the worktree into Gemini CLI\'s list');
  check(after['z:/somewhere/else'] === 'DO_NOT_TRUST' && after[key(PROJ)] === 'TRUST_FOLDER', 'the rest of the list is as it was');
  check(fs.readFileSync(TRUSTED, 'utf8').startsWith('{\n  "'), 'written the way Gemini CLI writes it');
  check(settings().worktree_trust?.['Gemini CLI'] === 'on', 'the answer is kept in the settings');
  await until(() => chrome.run(`document.getElementById("sask").hidden`), 'the question to go', 10000).catch(() => {});

  // The next one: no question
  await make(PROJ, 'feat-b');
  const b = await made('feat-b');
  await until(() => listed()[key(b)] === 'TRUST_FOLDER', 'the second to be written', 20000).catch(() => {});
  check(listed()[key(b)] === 'TRUST_FOLDER', 'the next worktree is written with no question');
  check(await chrome.run(`document.getElementById("sask").hidden && !(S.trust_asks || []).length`), 'and nothing is asked');

  // The settings: the value as it is, and changed there
  const rows = await cfg('/api/worktree-trust');
  check(Array.isArray(rows) && rows[0]?.ai === 'Gemini CLI' && rows[0]?.answer === 'on' && /trustedFolders\.json$/.test(rows[0]?.file || ''), `the settings show the answer (${JSON.stringify(rows)})`);
  if (process.env.SHOT) {
    await chrome.send('Page.navigate', { url: `${BASE}/cfg?t=${TOKEN}&section=ai` });
    await until(() => chrome.run('(() => { const c = document.getElementById("ai-trust"); if (!c || !/Gemini CLI/.test(c.textContent)) return false; c.scrollIntoView(); window.scrollBy(0, -260); document.querySelectorAll("main, .content, #content").forEach((m) => m.scrollBy && m.scrollBy(0, -260)); return [...c.querySelectorAll("button")].map((b) => b.textContent + ":" + b.disabled).join(","); })()'), 'the settings card', 30000).then((b) => console.log('  buttons ' + b));
    fs.writeFileSync(path.join(process.env.SHOT, 'worktree-trust-settings.png'), Buffer.from((await chrome.send('Page.captureScreenshot', { format: 'png' })).data, 'base64'));
  }
  const off = await cfg('/api/worktree-trust', { ai: 'Gemini CLI', on: false });
  check(off.ok && settings().worktree_trust?.['Gemini CLI'] === 'off', `"Do not write" is kept (${JSON.stringify(off)})`);
  await make(PROJ, 'feat-c');
  const c = await made('feat-c');
  await sleep(2500);
  check(!(key(c) in listed()), 'a worktree made after it is not written');

  // A project it does not trust: nothing, and no question
  await cfg('/api/worktree-trust', { ai: 'Gemini CLI', on: true });
  await make(OTHER, 'feat-d');
  const d = await made('feat-d');
  await sleep(2500);
  check(!(key(d) in listed()) && await chrome.run(`!(S.trust_asks || []).length`), 'a project it does not trust is left alone');
  check(chrome.thrown.length === 0, `the page threw nothing (${chrome.thrown.join(' | ')})`);

  if (REAL_GEMINI) {
    // The real CLI in the written worktree, and in one that was not written
    const dataDir = () => [path.join(APP, 'data'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data')].find((x) => fs.existsSync(path.join(x, 'api-token')));
    const call = async (method, ...params) => {
      const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
      await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
      let buf = ''; const waiting = [];
      sock.on('data', (x) => { buf += x.toString('utf8'); let i; while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); } });
      const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
      await line({ token: fs.readFileSync(path.join(dataDir(), 'api-token'), 'utf8').trim() });
      const ans = await line({ id: '1', method, params }); sock.destroy();
      if (!ans.ok) throw new Error(`${method}: ${ans.error}`); return ans.result;
    };
    const asks = /trust the files in this folder|Do you trust/i;
    for (const [where, want] of [[a, false], [c, true]]) {
      const id = await call('open_tab', { command: 'gemini', folder: where }).then((r) => (typeof r === 'string' ? r : r && (r.id || r.uid))).catch((e) => { console.log('  (could not open a tab: ' + e.message + ')'); return null; });
      if (!id) { failures += 1; continue; }
      const s = await until(async () => { const t = String(await call('tab_screen', id)); return /Type your message|trust the files/i.test(t) && t; }, 'Gemini CLI to come up', 90000).catch(() => '');
      check(asks.test(s) === want, `the real Gemini CLI ${want ? 'asks' : 'does not ask'} in ${path.basename(where)}`);
      await call('close_tab', id).catch(() => {});
      await sleep(2000);
    }
  }
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (chrome) chrome.stop();
  if (!process.env.KEEP) ps('-File', launcher, '-At', INSTANCE, '-Stop');
  if (!process.env.KEEP) fs.rmSync(RUN, { recursive: true, force: true });
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
