/**
 * The ports panel in the right-hand column, through the running app's own
 * window and a phone.
 *
 * This checkout's build in a folder of its own, with one folder whose tab runs
 * a small web server (node, on a free port of this PC's loopback) beside a
 * plain terminal, and a second folder where nothing listens.
 *
 *     cargo build
 *     node tools/debug/ports-panel.win.mjs [--ja]
 *
 * Checked:
 *   chip      the port on the folder's card is pressed with the mouse: the
 *             column opens on the ports panel, called up with its own ✕, and
 *             lists the port with the program holding it and the tab it came from
 *   open      pressing the row opens a browser tab of that folder on the port,
 *             written into the settings, and the page is what the server serves
 *   menu      a folder's right-click menu offers the ports and calls the panel up
 *   empty     a folder where nothing listens says so, and what starts a port
 *   script    `show_panel("ports")` through the external API calls it up
 *   phone     a phone-sized Chrome on the remote door gets the same rows, with
 *             no way to open this PC's localhost in the phone's own browser,
 *             and pressing a row brings up the page already open, not a second
 *
 * Needs Windows, Node and Chrome (the phone). Photographs land in
 * target/shots. Nothing of a copy somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-ports-panel');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const QUIET = path.join(RUN, 'quiet');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const JA = process.argv.includes('--ja');
const L = JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', 'en.json'), 'utf8'));
if (JA) Object.assign(L, JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', 'ja.json'), 'utf8')));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};
const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, QUIET, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

// The server a tab runs: what it serves is a word only it knows
const PORT = await freePort();
const WORD = 'served-by-the-tab-' + PORT;
fs.writeFileSync(path.join(WORK, 'serve.mjs'),
  `import http from 'node:http';\n` +
  `http.createServer((q, s) => { s.writeHead(200, {'content-type': 'text/html'}); s.end('<title>${WORD}</title><h1>${WORD}</h1>'); })` +
  `.listen(${PORT}, '127.0.0.1', () => console.log('listening on ${PORT}'));\n`);
const PHONE_PORT = await freePort();
const PHONE_KEY = 'portsphone0123456789abcd';
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  side_bar_width: 420,
  external_api: { access: 'user' },
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  desks: [{ name: 'Ports', id: 'ports', folders: [
    { cwd: WORK, tabs: [
      { name: 'serve', id: 'serve', command: 'node serve.mjs' },
      { name: 'shell', id: 'shell', command: 'cmd.exe' },
    ] },
    { cwd: QUIET, tabs: [{ name: 'quiet', id: 'quiet', command: 'cmd.exe' }] },
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
    fs.writeFileSync(path.join(SHOTS, `ports-panel-${JA ? 'ja-' : ''}${label}-${name}.png`), Buffer.from(r.data, 'base64'));
  };
  // A real press where an element is: the mouse, not a click() in the page
  const press = async (selectorJs, button = 'left') => {
    const b = await run(`(() => { const e = ${selectorJs}; if (!e) return null; e.scrollIntoView({block: "nearest"}); const r = e.getBoundingClientRect(); return {x: r.left + r.width / 2, y: r.top + r.height / 2}; })()`);
    if (!b) throw new Error('nothing to press: ' + selectorJs);
    for (const type of ['mousePressed', 'mouseReleased']) {
      await send('Input.dispatchMouseEvent', { type, x: b.x, y: b.y, button, clickCount: 1 });
    }
  };
  const front = async (id) => {
    const at = await run(`S.tabs.find(t => t.id === ${JSON.stringify(id)}).index`);
    await run(`send({kind:"select", tab:${at}})`);
    await until(() => run(`S.active === ${at}`), id + ' in front');
    await sleep(250);
  };
  // The ports panel as it is drawn: whether it stands, and its rows
  const panel = async () => JSON.parse(await run(`JSON.stringify({
    on: sidePanel, called: [...sideCalled],
    shown: !!document.getElementById("portpanel") && !document.getElementById("portpanel").hidden,
    rows: [...document.querySelectorAll("#portpanel .prt")].map(r => ({
      port: r.querySelector(".pp").textContent, program: r.querySelector(".pg").textContent,
      tab: r.querySelector(".po").textContent, acts: r.querySelectorAll(".pa").length })),
    text: (document.getElementById("portpanel") || {}).textContent || "",
  })`));
  return { ws, send, run, shot, press, front, panel };
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
const pageTabs = () => settings().desks[0].folders[0].tabs.filter((t) => String(t.command).startsWith('browser '));
const chip = `[...document.querySelectorAll("#tabs .tab.folder .pt.go")].find(e => e.textContent === ":${PORT}")`;

try {
  let boardTarget;
  await until(async () => {
    const p = portOf('shell');
    return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page'));
  }, 'the window\'s page', 40000);
  const board = await connect(boardTarget, 'window');
  await until(() => board.run('typeof sideShown === "function" && !!S && S.tabs.length >= 3'), 'the board and its tabs', 40000);
  await board.run('if (sideWidth() <= 0) setSideWidth(420)');

  console.log('1. the port on the folder\'s card calls the panel up');
  await board.front('shell');
  await until(() => board.run(`!!(S.tabs.find(t => t.id === "serve").place || {ports: []}).ports.includes(${PORT})`), 'the server\'s port under its tab', 30000);
  await until(() => board.run(`!!${chip}`), 'the port drawn on the tab\'s row')
    .catch(async (e) => { console.log('    (the page has: ' + await board.run('JSON.stringify({pts: [...document.querySelectorAll(".pt")].map(e => e.outerHTML), rows: [...document.querySelectorAll("#tabs .tab")].map(e => e.className + ":" + e.textContent.slice(0, 40))})') + ')'); throw e; });
  await board.press(chip);
  await until(async () => (await board.panel()).rows.length > 0, 'the ports panel with a row');
  let p = await board.panel();
  check(p.on === 'ports' && p.shown && p.called.includes('ports'), 'the column stands on the ports panel, called up: ' + JSON.stringify({ on: p.on, called: p.called }));
  const row = p.rows.find((r) => r.port === ':' + PORT);
  check(!!row, `the port is a row: ${JSON.stringify(p.rows)}`);
  check(!!row && /^node(\.exe)?$/i.test(row.program), 'with the program holding it: ' + (row && row.program));
  check(!!row && row.tab.includes('serve'), 'and the tab it came from: ' + (row && row.tab));
  check(!!row && row.acts === 2, 'on the PC, its own browser and a copy of the address are offered: ' + (row && row.acts));
  check(await board.run('document.querySelectorAll("#side .sbar button.called .sx").length === 1'), 'the panel carries its own ✕');
  await board.shot('1-panel');

  console.log('2. a row opens a browser tab of the folder on the port');
  await board.press(`[...document.querySelectorAll("#portpanel .prt")].find(r => r.querySelector(".pp").textContent === ":${PORT}")`);
  await until(() => pageTabs().length === 1, 'a browser tab written into the folder', 20000)
    .catch(async (e) => { console.log('    (the board says: ' + await board.run('S.flash || ""') + ')'); throw e; });
  check(pageTabs()[0].command === `browser http://localhost:${PORT}/`, 'written down on the port: ' + pageTabs()[0].command);
  await until(() => board.run('S.tabs.some(t => t.kind === "browser")'), 'the page among the tabs', 15000).catch(() => {});
  const pageAt = await board.run(`(S.tabs.find(t => t.kind === "browser") || {}).index`);
  check(pageAt != null && await board.run(`S.tabs.find(t => t.index === ${pageAt}).group === S.tabs.find(t => t.id === "serve").group`), 'the page is a tab of the same folder');
  // What the page shows is the server's own word, read off the page itself
  const door = await openDoor();
  const pageId = await board.run('S.tabs.find(t => t.kind === "browser").id');
  let text = '';
  await until(async () => (text = String(await door('browser_text', pageId, { xpath: '//h1' }).catch((e) => e.message))).includes(WORD), 'the page loaded', 20000).catch(() => {});
  check(text.includes(WORD), 'the browser tab shows what the server serves: ' + text.slice(0, 80));
  // The page in front stands on its own choice (its console); the panel that
  // was called stays in the strip until its ✕
  await sleep(500);
  check((await board.panel()).called.includes('ports'), 'the ports panel stays in the strip after the page opens');

  console.log('3. a folder\'s menu offers the ports');
  await board.run('sideDismiss("ports")');
  check(!(await board.panel()).called.includes('ports'), 'the ✕ took the panel away');
  const cardJs = `[...document.querySelectorAll("#tabs .tab.folder")].find(r => r.classList.contains("front"))`;
  await board.press(cardJs, 'right');
  await until(() => board.run(`[...document.querySelectorAll(".fmenu div")].some(d => d.textContent === ${JSON.stringify(L['tui.menu.ports.here'])})`), 'the menu with the ports entry', 8000);
  await board.press(`[...document.querySelectorAll(".fmenu div")].find(d => d.textContent === ${JSON.stringify(L['tui.menu.ports.here'])})`);
  await until(async () => { const q = await board.panel(); return q.on === 'ports' && q.rows.length > 0; }, 'the panel from the menu');
  check(true, 'the menu\'s entry calls the panel up on the folder');

  console.log('4. a folder where nothing listens');
  await board.front('quiet');
  p = await board.panel();
  check(p.on === 'ports' && p.rows.length === 0 && p.text.includes(L['tui.ports.here.none']), 'it says nothing listens, and what starts a port: ' + JSON.stringify(p));
  await board.shot('4-empty');

  console.log('5. show_panel from a script');
  await board.run('sideDismiss("ports")');
  await sleep(9000);
  await door('show_panel', 'ports');
  await until(async () => (await board.panel()).called.includes('ports'), 'the panel called by the script', 8000).catch(() => {});
  check((await board.panel()).called.includes('ports'), 'show_panel("ports") calls it up');

  console.log('6. the phone');
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
  await until(() => phone.run('typeof sideShown === "function" && !!S && S.active != null && S.tabs.length >= 3'), 'the phone\'s board', 30000);
  await phone.front('serve');
  await phone.run('showPorts(S.groups[S.tabs.find(t => t.id === "serve").group])');
  await until(async () => (await phone.panel()).rows.length > 0, 'the phone\'s ports panel');
  p = await phone.panel();
  const prow = p.rows.find((r) => r.port === ':' + PORT);
  check(!!prow && prow.acts === 0, 'the phone gets the row, with nothing that would open the phone\'s own localhost: ' + JSON.stringify(prow));
  await phone.shot('6-panel');
  await phone.press(`[...document.querySelectorAll("#portpanel .prt")].find(r => r.querySelector(".pp").textContent === ":${PORT}")`);
  await until(() => phone.run(`S.tabs.find(t => t.index === S.active).kind === "browser"`), 'the page in front on the phone', 15000).catch(() => {});
  check(await phone.run(`S.tabs.find(t => t.index === S.active).kind === "browser"`), 'the phone\'s press brings the page up');
  check(pageTabs().length === 1, 'the page already open is used, not a second one: ' + pageTabs().length);
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
