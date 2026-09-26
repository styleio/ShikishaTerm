/**
 * Do the phone's page buttons step aside when left alone, and come back on a
 * touch?
 *
 *     cargo build
 *     node tools/debug/pager-idle.win.mjs
 *
 * Needs Windows, Node and Chrome. Starts this checkout's build in a folder of
 * its own with the relay on, and opens that relay in a headless Chrome made to
 * look like a phone. Nothing of a copy somebody is using is read, written or
 * stopped.
 *
 * Why it exists. The 📖 ▲ ▼ buttons stand over the right edge of the terminal,
 * on the rows being read, so they put themselves away after a few seconds
 * untouched and come back on any touch. That is a matter of timers and real
 * touches at runtime, which the page's own tests -- reading the source -- do
 * not see.
 *
 * What it checks, in order:
 *   the buttons are there on arriving at a terminal tab
 *   left alone past the idle time, they are hidden (not merely faded)
 *   a touch on the terminal brings them back, and the count starts again
 *   a tap on ▲ keeps them up while that move is on its way, then they go again
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-pager-idle');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const BOARD = 9371;          // the app's relay
const CHROME_CDP = 9372;     // the stand-in phone's own DevTools port
const IDLE_MS = 3000;        // PAGER_IDLE_MS in shell.rs

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  die('no Chrome found; set CHROME');
}

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated, with the relay on');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: true, bind: '127.0.0.1', port: BOARD },
  desks: [{ name: 'Check', id: 'check', folders: [
    { cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] },
  ] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

// The relay writes its token as it opens: its arrival is what says the copy is up
const tokenFile = path.join(APP, 'data', 'remote-token');
let token = '';
for (let i = 0; i < 160 && !token; i++) {
  try { token = fs.readFileSync(tokenFile, 'utf8').trim(); } catch { await sleep(250); }
}
if (!token) { stopApp(); die('the relay never opened -- see ' + path.join(APP, 'logs')); }

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-standin-'));
const chrome = spawn(findChrome(), [
  '--headless=new',
  '--remote-debugging-port=' + CHROME_CDP,
  '--user-data-dir=' + profile,
  '--no-first-run',
  '--no-default-browser-check',
  'about:blank',
], { stdio: 'ignore' });

async function attach(port = CHROME_CDP) {
  let list;
  for (let i = 0; i < 80 && !list; i++) {
    try { list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { await sleep(250); }
    if (list && !list.length) list = null;
  }
  if (!list) die('nothing answered the DevTools port ' + port);
  const ws = new WebSocket(list[0].webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const call = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  return { call, run, close: () => ws.close() };
}

const until = async (test, what, ms = 25000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};

// Whether a person can see the buttons: the pager is on, and not put away
const pagerShown = (dev) => dev.run(`(() => { const p = document.getElementById("pageui");
  return p.classList.contains("on") && getComputedStyle(document.getElementById("pageUp")).visibility === "visible"; })()`);
// A finger, the way a touch screen delivers one: pointer events and all
async function touch(dev, where) {
  const box = await dev.run(`(() => { const r = document.getElementById(${JSON.stringify(where)}).getBoundingClientRect();
    return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2)}; })()`);
  await dev.call('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: box.x, y: box.y }] });
  await dev.call('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
}

let dev = null;
try {
  dev = await attach();
  await dev.call('Page.enable');
  await dev.call('Runtime.enable');
  await dev.call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
  await dev.call('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  await dev.call('Page.navigate', { url: `http://127.0.0.1:${BOARD}/?t=${token}` });
  await until(() => dev.run(`!!(S && S.tabs && S.tabs.length)`), 'the board');
  await until(() => dev.run(`document.getElementById("pageui").classList.contains("on")`), 'the page buttons');

  check(await pagerShown(dev), 'the buttons are up on arriving at a terminal tab');
  await sleep(IDLE_MS + 700);
  check(!(await pagerShown(dev)), 'left alone past the idle time, they are hidden');

  await touch(dev, 'screen');
  await sleep(150);
  check(await pagerShown(dev), 'a touch on the terminal brings them back');
  await sleep(IDLE_MS - 1200);
  check(await pagerShown(dev), 'they stay while the new count runs');
  await touch(dev, 'screen');
  await sleep(IDLE_MS - 1200);
  check(await pagerShown(dev), 'another touch starts the count again');
  await sleep(1900);
  check(!(await pagerShown(dev)), 'and when it runs out they go again');

  // A move that has not landed keeps them up: the count and spinner between
  // the buttons are the answer to the tap. Held here by pretending the screen
  // never arrives, which is what a slow link looks like
  await touch(dev, 'screen');
  await sleep(150);
  await touch(dev, 'pageUp');
  await sleep(400);
  await dev.run(`pgArrived = () => {}; clearTimeout(pgWaitTimer); pgWaiting = true; true`);
  await sleep(IDLE_MS + 700);
  check(await pagerShown(dev), 'a page turn still on its way keeps them up');
  await dev.run(`pgWaiting = false; pgCount(); true`);
  await sleep(IDLE_MS + 700);
  check(!(await pagerShown(dev)), 'once it has landed they go as usual');
} catch (e) {
  console.error('\n' + (e && e.stack || e));
  failures += 1;
} finally {
  try { if (dev) dev.close(); } catch {}
  chrome.kill();
  await sleep(500);
  fs.rmSync(profile, { recursive: true, force: true });
  stopApp();
}
console.log(failures ? `\n${failures} FAILED` : '\nall good');
process.exit(failures ? 1 : 0);
