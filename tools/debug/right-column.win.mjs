/**
 * The right-hand column follows the tab in front, through the running app's
 * own window and a phone.
 *
 * This checkout's build in a folder of its own, with a git repository holding
 * a stand-in AI tab (a program named `claude`), a terminal and a browser tab,
 * and a second folder that is no repository with a terminal of its own.
 *
 *     cargo build
 *     node tools/debug/right-column.win.mjs [--ja]
 *
 * Checked:
 *   kinds     the strip of each: an AI has files, changes and its conversation;
 *             a terminal files and changes; a page its console; a terminal in
 *             a folder no repository holds only its files
 *   choice    the panel chosen is remembered per kind of tab, a kind moved
 *             away from and back to opens on its own choice, and the choices
 *             are written into the settings
 *   called    a panel called up by a button (the search of every
 *             conversation, over a page) joins the strip with its own ✕, stays
 *             while another tab is looked at, and goes when the ✕ is pressed;
 *             called over a panel the person chose, its ✕ goes back to it
 *   script    `show_panel` through the external API does the same
 *   phone     a phone-sized Chrome on the remote door gets the same strips
 *
 * Needs Windows, Node, git and Chrome (the phone). Photographs land in
 * target/shots. Nothing of a copy somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-right-column');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'repo');
const PLAIN = path.join(RUN, 'plain');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const JA = process.argv.includes('--ja');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end('<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Shop</title></head><body><h1>Shop</h1></body></html>');
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, REPO, PLAIN, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const git = (...a) => spawnSync('git', a, { cwd: REPO, encoding: 'utf8' });
git('init', '-q');
git('-c', 'user.email=t@example.invalid', '-c', 'user.name=t', 'commit', '-q', '--allow-empty', '-m', 'start');
// A stand-in AI: named claude so the app knows it for one, and quiet
const standIn = path.join(REPO, 'stand-in.mjs');
fs.writeFileSync(standIn, `process.stdout.write('ready> '); setInterval(() => {}, 1 << 30);\n`);
fs.writeFileSync(path.join(REPO, 'claude.cmd'), `@node "${standIn}"\r\n`);
const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});
const PHONE_PORT = await freePort();
const PHONE_KEY = 'columnphone0123456789abc';
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  side_bar_width: 420,
  external_api: { access: 'user' },
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  desks: [{ name: 'Column', id: 'column', folders: [
    { cwd: REPO, tabs: [
      { name: 'claude', id: 'ai', command: path.join(REPO, 'claude.cmd') },
      { name: 'shell', id: 'shell', command: 'cmd.exe' },
      { name: 'page', id: 'page', command: `browser http://127.0.0.1:${pagePort}/` },
    ] },
    { cwd: PLAIN, tabs: [{ name: 'plain', id: 'plain', command: 'cmd.exe' }] },
  ] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const child = spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
async function connect(target, label) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  const shot = async (name) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `right-column-${JA ? 'ja-' : ''}${label}-${name}.png`), Buffer.from(r.data, 'base64'));
  };
  // What the strip holds: the panel names in order, which are called (✕),
  // which one is standing, and how many buttons the strip really drew
  const strip = async () => JSON.parse(await run(`JSON.stringify({
    ids: sideShown(sideKind()).map(p => p[0]),
    called: [...sideCalled],
    on: sidePanel,
    drawn: document.querySelectorAll("#side .sbar button:not(.away)").length,
    xs: document.querySelectorAll("#side .sbar button.called .sx").length,
  })`));
  const front = async (id) => {
    const at = await run(`S.tabs.find(t => t.id === ${JSON.stringify(id)}).index`);
    await run(`send({kind:"select", tab:${at}})`);
    await until(() => run(`S.active === ${at}`), id + ' in front');
    await sleep(250);
  };
  return { ws, send, run, shot, strip, front };
}
async function openDoor() {
  const tokenFile = path.join(APP, 'data', 'api-token');
  await until(() => fs.existsSync(tokenFile), 'the API token', 60000);
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); }
  });
  const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
  await line({ token: fs.readFileSync(tokenFile, 'utf8').trim() });
  let n = 0;
  return async (method, ...params) => {
    const a = await line({ id: String(++n), method, params });
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
}

let chrome = null;
const PHONE_DIR = path.join(RUN, 'phone');
function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('no Chrome found; set CHROME');
}
const settings = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8').replace(/^﻿/, ''));

try {
  let boardTarget;
  await until(async () => {
    const p = portOf('shell');
    return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page'));
  }, 'the window\'s page', 40000);
  const board = await connect(boardTarget, 'window');
  await until(() => board.run('typeof sideShown === "function" && !!S && S.tabs.length >= 4 && S.tabs.some(t => t.id === "ai" && t.ai)'), 'the board and its tabs', 40000);
  await board.run('if (sideWidth() <= 0) setSideWidth(420)');

  console.log('1. each kind of tab has its own strip');
  const kinds = [
    ['ai', ['files', 'git', 'convo']],
    ['shell', ['files', 'git']],
    ['page', ['console']],
    ['plain', ['files']],
  ];
  for (const [id, want] of kinds) {
    await board.front(id);
    const s = await board.strip();
    check(same(s.ids, want) && s.drawn === want.length, `${id}: ${s.ids.join(', ')} (${s.drawn} drawn)`);
    await board.shot('1-' + id);
  }

  console.log('2. the choice is per kind, and written down');
  await board.front('ai');
  await board.run('sideChoose("convo")');
  check((await board.strip()).on === 'convo', 'the AI tab on its conversation');
  await board.front('shell');
  check((await board.strip()).on === 'files', 'a terminal opens on files, not on the AI\'s choice');
  await board.run('sideChoose("git")');
  await board.front('page');
  check((await board.strip()).on === 'console', 'a page on its console');
  await board.front('ai');
  check((await board.strip()).on === 'convo', 'back on the AI: its conversation again');
  await board.front('plain');
  check((await board.strip()).on === 'files', 'a terminal with no repository falls back to files');
  await board.front('shell');
  check((await board.strip()).on === 'git', 'and a terminal in the repository keeps its changes');
  await until(() => same(settings().side_panels, { ai: 'convo', term: 'git' }), 'the choices in the settings', 8000).catch(() => {});
  check(same(settings().side_panels, { ai: 'convo', term: 'git' }), 'written down: ' + JSON.stringify(settings().side_panels));

  console.log('3. a panel called up over a page');
  await board.front('page');
  await board.run('window.__openVault()');
  let s = await board.strip();
  check(same(s.ids, ['convo', 'console']) && s.on === 'convo' && same(s.called, ['convo']) && s.xs === 1,
    'the conversations join the page\'s strip, standing, with a ✕');
  await board.shot('3-called');
  await board.front('shell');
  s = await board.strip();
  check(s.called.includes('convo') && s.on === 'git', 'looking at another tab keeps it, and that tab keeps its own choice');
  await board.front('page');
  await board.run('document.querySelector("#side .sbar button.called .sx").click()');
  await sleep(200);
  s = await board.strip();
  check(same(s.ids, ['console']) && s.on === 'console' && s.called.length === 0, 'its ✕ takes it away again');
  // A panel called over one the person chose: its ✕ goes back to that choice,
  // not to the first panel of the strip (the call used to write over it)
  await board.front('ai');
  await board.run('sideChoose("git")');
  await board.run('sideReveal("ports")');
  s = await board.strip();
  check(s.on === 'ports' && s.called.includes('ports'), 'ports called up over the AI\'s changes');
  await board.run('document.querySelector("#side .sbar button.called .sx").click()');
  await sleep(200);
  s = await board.strip();
  check(s.on === 'git' && !s.called.includes('ports'), 'its ✕ goes back to the changes the person chose, not to files: ' + s.on);
  // Back to the choice written down in part 2, which the phone expects
  await board.run('sideChoose("convo")');

  console.log('4. show_panel from a script');
  const door = await openDoor();
  await board.front('shell');
  // A script waits while the person has just moved the view (`show` does too)
  await door('show_panel', 'convo');
  await sleep(1500);
  check(!(await board.strip()).called.includes('convo'), 'held while the person has just moved the view');
  await sleep(8500);
  await door('show_panel', 'convo');
  await until(async () => (await board.strip()).on === 'convo', 'the column on the conversation', 8000).catch(() => {});
  s = await board.strip();
  check(s.on === 'convo' && s.called.includes('convo'), 'show_panel opens the column on the conversation over a terminal');
  let refused = '';
  try { await door('show_panel', 'nothing'); } catch (e) { refused = e.message; }
  check(/no panel called/.test(refused), 'a name that is no panel is refused: ' + refused.slice(0, 80));
  await board.run('sideDismiss("convo")');

  console.log('5. the phone');
  fs.mkdirSync(PHONE_DIR, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + PHONE_DIR,
    '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(PHONE_DIR, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const pport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let pt;
  await until(async () => (pt = (await targetsOf(pport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connect(pt, 'phone');
  await phone.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
  await until(() => phone.run('typeof sideShown === "function" && !!S && S.active != null && S.tabs.length >= 4'), 'the phone\'s board', 30000);
  await phone.run('setSideWidth(SIDEW_DEF)');
  for (const [id, want] of kinds) {
    await phone.front(id);
    const p = await phone.strip();
    check(same(p.ids, want), `phone, ${id}: ${p.ids.join(', ')}`);
  }
  await phone.front('ai');
  check((await phone.strip()).on === 'convo', 'the phone opens the AI tab on the choice written down');
  await phone.shot('5-ai');
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
  server.close();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
