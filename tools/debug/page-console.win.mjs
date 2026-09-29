/**
 * A page's console, read in the column's Console tab, through the running
 * app's own window.
 *
 * This checkout's build in a folder of its own, with a small page served here
 * in a browser tab and a stand-in AI tab beside it (a program named `claude`
 * that turns bracketed paste on and writes down every byte it is given).
 *
 *     cargo build
 *     node tools/debug/page-console.win.mjs [--ja]
 *
 * Checked:
 *   before    what the page said since it loaded, before the tab was opened,
 *             is there, and the panel says where its list starts
 *   heard     a log, a warning, an error nobody caught and a file that did not
 *             load each arrive, as the kind they are
 *   kinds     switching a kind off takes its lines out of the list
 *   script    the external API's `browser_console` reads the same lines
 *   handed    "Hand to an AI" puts the shown lines into the stand-in's input as
 *             one bracketed paste, with no Enter after it
 *   cleared   Clear empties the list
 *   phone     a phone-sized Chrome on the remote door reads the same console
 *
 * Needs Windows, Node and Chrome (the phone). Photographs land in
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
const RUN = path.join(os.tmpdir(), 'sk-console');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const HEARD = path.join(RUN, 'heard.bin');
const JA = process.argv.includes('--ja');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
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
  if (req.url.startsWith('/missing')) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Shop</title></head>
<body><h1>Shop</h1><script>console.log("said before anybody listened")</script></body></html>`);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const standIn = path.join(WORK, 'stand-in.mjs');
fs.writeFileSync(standIn, `import fs from 'node:fs';
process.stdout.write('\\x1b[?2004hready> ');
process.stdin.setRawMode && process.stdin.setRawMode(true);
process.stdin.on('data', (d) => fs.appendFileSync(${JSON.stringify(HEARD)}, d));
setInterval(() => {}, 1 << 30);
`);
fs.writeFileSync(path.join(WORK, 'claude.cmd'), `@node "${standIn}"\r\n`);
const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});
const PHONE_PORT = await freePort();
const PHONE_KEY = 'consolephone0123456789ab';
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  external_api: { access: 'user' },
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  desks: [{ name: 'Console', id: 'console', folders: [{ cwd: WORK, tabs: [
    { name: 'claude', id: 'ai', command: path.join(WORK, 'claude.cmd') },
    { name: 'page', id: 'page', command: `browser http://127.0.0.1:${pagePort}/` },
  ] }] }],
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
async function connect(target) {
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
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `page-console-${JA ? 'ja-' : ''}${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, send, run, shot };
}
// The external API: the pipe, with the token the copy wrote beside itself
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

try {
  let boardTarget, pageTarget;
  await until(async () => {
    const p = portOf('shell');
    return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page'));
  }, 'the window\'s page', 40000);
  const board = await connect(boardTarget);
  await until(() => board.run('typeof drawConsole === "function" && !!S && S.tabs.length >= 2'), 'the board and its tabs', 30000);
  const pageTab = await board.run('S.tabs.find(t => t.kind === "browser").index');
  await board.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => board.run(`S.active === ${pageTab}`), 'the page in front');
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    return p && (pageTarget = (await targetsOf(p)).find((t) => t.type === 'page' && t.url.includes(String(pagePort))));
  }, 'the page\'s own browser', 30000);
  const page = await connect(pageTarget);
  await until(() => page.run('document.readyState === "complete"'), 'the page loaded');
  const lines = () => board.run('JSON.stringify(CN.lines)').then(JSON.parse);

  console.log('1. the Console tab opened on the page');
  await board.run('sidePanel = "console"; setSideWidth(420);');
  await until(() => board.run('CN.listening === true'), 'listening');
  // Turning the protocol on brings what this document already said with it
  await until(async () => (await lines()).some((l) => l.text.includes('said before')), 'the page\'s earlier line', 5000).catch(() => {});
  check((await lines()).some((l) => l.text.includes('said before')), 'what the page said since it loaded is there');
  check(await board.run('!!document.querySelector("#consolepanel .cnow")'), 'the panel says where the list starts');

  console.log('2. the page says four kinds of things');
  await page.run(`console.log("hello", 3); console.warn("running low");
    setTimeout(() => { throw new Error("boom") }, 0);
    const i = new Image(); i.src = "/missing.png"; document.body.append(i); true`);
  await until(async () => {
    const l = await lines();
    return l.some((x) => x.text === 'hello 3') && l.some((x) => x.level === 'warn') &&
      l.some((x) => x.from === 'thrown') && l.some((x) => x.from === 'browser');
  }, 'the four lines', 15000);
  const l = await lines();
  check(l.find((x) => x.text === 'hello 3').level === 'log', 'a log is a log');
  check(l.find((x) => x.text === 'running low').level === 'warn', 'a warning is a warning');
  check(/boom/.test(l.find((x) => x.from === 'thrown').text) && l.find((x) => x.from === 'thrown').level === 'error', 'an uncaught error is an error');
  check(/missing\.png/.test(l.find((x) => x.from === 'browser').at), 'the file that did not load names itself');
  await sleep(400);
  await board.shot('1-heard');

  console.log('3. logs switched off');
  const rows = () => board.run('document.querySelectorAll("#consolepanel .crow").length');
  const before = await rows();
  await board.run('CN.show.delete("log"); drawConsole();');
  check((await rows()) === before - l.filter((x) => x.level === 'log').length, 'the logs leave the list');

  console.log('4. the same console through the external API');
  const door = await openDoor();
  const read = await door('browser_console', 'page');
  const got = Array.isArray(read) && Array.isArray(read[0]) ? read[0] : read;
  check(Array.isArray(got) && got.some((x) => x.text === 'hello 3'), 'browser_console reads the lines');

  console.log('5. handed to the AI tab');
  await board.run('consoleAsk("send", {to: "ai", levels: [...CN.show]})');
  await until(() => fs.existsSync(HEARD) && fs.readFileSync(HEARD, 'utf8').includes('\x1b[201~'), 'the draft in the stand-in', 20000);
  await sleep(1500);
  const heard = fs.readFileSync(HEARD, 'utf8');
  check(heard.startsWith('\x1b[200~') && heard.endsWith('\x1b[201~'), 'one bracketed paste and no Enter');
  check(heard.includes('Console of the web page') && heard.includes('[warn] running low'), 'with the lines shown');
  check(!heard.includes('hello 3'), 'and not the kind switched off');

  console.log('6. the phone');
  fs.mkdirSync(PHONE_DIR, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + PHONE_DIR,
    '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(PHONE_DIR, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const pport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let pt;
  await until(async () => (pt = (await targetsOf(pport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connect(pt);
  await phone.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
  await until(() => phone.run('typeof drawConsole === "function" && !!S && S.active != null'), 'the phone\'s board', 30000);
  await phone.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => phone.run(`S.active === ${pageTab}`), 'the page in front on the phone');
  await phone.run('sidePanel = "console"; setSideWidth(SIDEW_DEF);');
  await until(() => phone.run('CN.lines.some(l => l.text === "running low")'), 'the phone reading the console');
  check(true, 'the phone reads the same console');
  await sleep(500);
  await phone.shot('2-phone');

  console.log('7. cleared');
  await board.run('consoleAsk("clear")');
  await until(() => board.run('CN.lines.length === 0'), 'the list emptied');
  check(true, 'Clear empties it');
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
