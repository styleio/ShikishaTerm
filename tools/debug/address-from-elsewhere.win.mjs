/**
 * Can what is typed in the address bar be opened from a phone?
 *
 *     cargo build
 *     node tools/debug/address-from-elsewhere.win.mjs
 *
 * Needs Windows, Node and Chrome. Starts this checkout's build in a folder of
 * its own with the relay on and one browser tab whose row of controls carries
 * an address bar, then opens that relay in a headless Chrome standing in for a
 * phone. Nothing of a copy somebody is using is read, written or stopped.
 *
 * Why it exists. The address bar had one door: the Enter key. On a phone that
 * key is as often the keystroke that ends a conversion as it is an answer to
 * the box it is in, and the two cannot be told apart in time to act on one of
 * them -- so what was typed opened nothing. Worse, the row is rebuilt several
 * times a second, and what was typed was kept only while the field held the
 * focus: the keyboard closing put the page's own URL back in the field. From
 * the machine it happens on there is nothing to see. Every piece has tests of
 * its own; what none of them can answer is whether a finger can open a page.
 *
 * What it checks, in order:
 *   the row     the button beside the field is there for a device, and is not
 *               drawn in the window, which has real keys
 *   the text    an address typed and then left alone -- the keyboard closing --
 *               is still there a second later, and Esc gives the field back
 *   the press   pressing the button really opens it, and words that are no
 *               address are searched for instead
 *
 * What it cannot stand in for: a soft keyboard. Keys dispatched over DevTools
 * arrive as a real keyboard's do, which is exactly the thing that is not true
 * on a phone -- so the button, not the key, is what is pressed here.
 */
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-address');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const BOARD = 9361;          // the app's relay; 93xx is this folder's band
const WINDOW_CDP = 9362;     // the app's own window: a port of its own
const CHROME_CDP = 9363;     // the stand-in device's own DevTools port
const PAGE_PORT = 9364;      // the pages being opened, served from here

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

// Two pages of this script's own serving, so that opening one proves itself
// with no network: the tab starts on /one and the address bar is sent to /two
const server = http.createServer((req, res) => {
  const at = new URL(req.url, 'http://127.0.0.1');
  const which = at.pathname === '/two' ? 'two' : 'one';
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(`<!doctype html><meta charset="utf-8"><title>page ${which}</title>` +
    `<body style="margin:0;font:48px system-ui;display:grid;place-items:center;height:100vh">${which}</body>`);
});
await new Promise((r) => server.listen(PAGE_PORT, '127.0.0.1', r));

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
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [
    { name: 'page', id: 'page', command: `browser http://127.0.0.1:${PAGE_PORT}/one`,
      nav: { back: true, forward: true, reload: true, url: true } },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${WINDOW_CDP}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const tokenFile = path.join(APP, 'data', 'remote-token');
let token = '';
for (let i = 0; i < 160 && !token; i++) {
  try { token = fs.readFileSync(tokenFile, 'utf8').trim(); } catch { await sleep(250); }
}
if (!token) { stopApp(); die('the relay never opened -- see ' + path.join(APP, 'logs')); }

// What the app wrote down about where it was told to go. The decision is made
// there, not in the page, so this is where "words become a search" is read
const navigates = () => {
  try {
    return fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8')
      .split(/\r?\n/).filter((l) => l.includes('Navigate page:'));
  } catch { return []; }
};

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-address-'));
const chrome = spawn(findChrome(), [
  '--headless=new',
  '--remote-debugging-port=' + CHROME_CDP,
  '--user-data-dir=' + profile,
  '--window-size=412,915',
  '--no-first-run',
  '--no-default-browser-check',
  'about:blank',
], { stdio: 'ignore' });

// One conversation with a browser -- the stand-in device's, or the app's own
// window. Raw CDP over a socket, so nothing here needs a driver installed
async function attach(port = CHROME_CDP, pick = null) {
  let hit;
  for (let i = 0; i < 80 && !hit; i++) {
    try {
      const list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter((t) => t.type === 'page');
      // The window holds more than one page: the board, and a webview for every
      // browser tab in it. Which one is wanted is the caller's to say
      hit = pick ? list.find(pick) : list[0];
    } catch {}
    if (!hit) await sleep(250);
  }
  if (!hit) die('nothing answered the DevTools port ' + port);
  const ws = new WebSocket(hit.webSocketDebuggerUrl);
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

// Move to a tab by name and wait until the page agrees it is in front
async function toTab(dev, name) {
  await until(() => dev.run(`!!(S && S.tabs && S.tabs.length)`), 'the board');
  await dev.run(`send({kind:"select", tab: S.tabs.find(t => t.name === ${JSON.stringify(name)}).index}); true`);
  await until(() => dev.run(`!!(S && S.tabs && S.tabs.some(t => t.index === S.active && t.name === ${JSON.stringify(name)}))`),
    'the ' + name + ' tab in front');
}

// Write into the field the way a keyboard does, then leave it: the press that
// follows is the whole point, and on a phone it comes after the field has lost
// the focus to the keyboard closing
async function typeAddress(dev, what) {
  await dev.run(`(() => { const i = document.querySelector("#nav input"); i.focus(); i.select(); return true; })()`);
  await dev.call('Input.insertText', { text: what });
  await dev.run(`document.querySelector("#nav input").dispatchEvent(new Event("input", {bubbles:true})); true`);
  await dev.run(`document.querySelector("#nav input").blur(); true`);
}

let dev = null;
try {
  console.log('\n1. the row, as a phone is served it');
  dev = await attach();
  await dev.call('Page.enable');
  await dev.call('Runtime.enable');
  await dev.call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
  await dev.call('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  await dev.call('Page.navigate', { url: `http://127.0.0.1:${BOARD}/?t=${token}` });
  await toTab(dev, 'page');
  await until(() => dev.run(`!!document.querySelector("#nav input")`), 'the address bar');
  check(await dev.run(`!!document.querySelector("#nav .navgo")`),
    'the field has a button beside it, which is the door a finger has');
  check(await dev.run(`document.querySelector("#nav input").getAttribute("enterkeyhint") === "go"`),
    'the keyboard is asked for a key that says go');

  console.log('\n2. what is typed stays typed');
  await typeAddress(dev, `http://127.0.0.1:${PAGE_PORT}/two`);
  await sleep(1500);   // several rebuilds, the field no longer holding the focus
  check(await dev.run(`document.querySelector("#nav input").value === "http://127.0.0.1:${PAGE_PORT}/two"`),
    'the address is still in the field after the keyboard closed, found ' +
      JSON.stringify(await dev.run(`document.querySelector("#nav input").value`)));

  console.log('\n3. the press opens it');
  await dev.run(`document.querySelector("#nav .navgo").click(); true`);
  await until(() => dev.run(`!!(S && S.nav && S.nav.at && S.nav.at.indexOf("/two") > 0)`),
    'the page to be the one that was typed');
  check(true, 'pressing the button opens what was typed');
  check(await dev.run(`document.querySelector("#nav input").value.indexOf("/two") > 0`),
    'the field shows where the page now is');

  console.log('\n4. words that are no address');
  await typeAddress(dev, 'YouTube');
  await dev.run(`document.querySelector("#nav .navgo").click(); true`);
  await until(async () => navigates().some((l) => l.includes('search?q=YouTube')),
    'the words to be handed over as a search');
  check(true, 'words are searched for rather than refused');

  console.log('\n5. Esc gives the field back');
  await typeAddress(dev, 'something else');
  await dev.run(`(() => { const i = document.querySelector("#nav input"); i.focus();
    i.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true})); return true; })()`);
  await sleep(600);
  check(await dev.run(`document.querySelector("#nav input").value === S.nav.at`),
    'Esc puts the page\'s own address back');
  dev.close(); dev = null;

  console.log('\n6. the window, which has real keys');
  dev = await attach(WINDOW_CDP, (t) => t.title === 'SHIKISHA-TERM');
  await dev.call('Runtime.enable');
  await until(() => dev.run(`!!document.querySelector("#nav input")`), 'the address bar in the window');
  check(!(await dev.run(`!!document.querySelector("#nav .navgo")`)),
    'the window is given no button it has no use for');
  // ...and the key it has instead still opens what is typed
  await dev.run(`(() => { const i = document.querySelector("#nav input"); i.focus(); i.select();
    i.value = "http://127.0.0.1:${PAGE_PORT}/one"; i.dispatchEvent(new Event("input", {bubbles:true})); return true; })()`);
  for (const type of ['keyDown', 'keyUp']) {
    await dev.call('Input.dispatchKeyEvent', { type, key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
  }
  await until(() => dev.run(`!!(S && S.nav && S.nav.at && S.nav.at.indexOf("/one") > 0)`),
    "the window's own Enter to open what was typed");
  check(true, 'the key opens what was typed, as it did before there was a button');
} catch (e) {
  console.error('\n' + (e && e.stack || e));
  failures += 1;
} finally {
  try { if (dev) dev.close(); } catch {}
  chrome.kill();
  server.close();
  await sleep(500);
  fs.rmSync(profile, { recursive: true, force: true });
  stopApp();
}
console.log(failures ? `\n${failures} FAILED` : '\nall good');
process.exit(failures ? 1 : 0);
