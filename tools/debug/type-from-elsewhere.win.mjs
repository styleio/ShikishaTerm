/**
 * Can somebody looking at this board from another machine just type?
 *
 *     cargo build
 *     node tools/debug/type-from-elsewhere.win.mjs
 *
 * Needs Windows, Node and Chrome. Starts this checkout's build in a folder of
 * its own with the relay on, then opens that relay in a headless Chrome twice:
 * once as a laptop (a mouse and real keys) and once as a phone (a touch screen).
 * Nothing of a copy somebody is using is read, written or stopped.
 *
 * Why it exists. The sub-input bar was written for the phone, whose soft
 * keyboard would otherwise stand over the screen being typed at -- and it
 * quietly became the only way in for everything that is not the window. On a
 * laptop that meant keys pressed at the terminal went nowhere at all, with no
 * error and nothing in any log: the page simply had no caret anywhere. Only a
 * second device can see that, and nothing in the test suite can: the page's own
 * tests read the source, and what is wrong here is where the caret is at
 * runtime. So this is a browser, on the other end of the real relay, typing.
 *
 * What it checks, in order:
 *   laptop  the page decides by itself that this machine types into the pane,
 *           and keys pressed with nothing clicked reach the terminal
 *   phone   the page decides the other way, and a tap brings the bar up without
 *           pulling the caret into the pane (which is what would throw the soft
 *           keyboard over the screen), and the bar's keyboard button hands
 *           typing back to the screen, and the pen takes it back again
 *
 * What it cannot stand in for: a soft keyboard. Keys sent to a browser over
 * DevTools arrive the way a real keyboard's do -- so a phone driven from here
 * is a phone with a keyboard beside it, which the page is meant to notice and
 * does. The part a phone alone can answer is the tap.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-typing');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const BOARD = 9347;          // the app's relay; 93xx is this folder's band
const CHROME_CDP = 9349;     // the stand-in device's own DevTools port

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
  '--window-size=1280,860',
  '--no-first-run',
  '--no-default-browser-check',
  'about:blank',
], { stdio: 'ignore' });

// One conversation with the stand-in device's browser. Raw CDP over a socket:
// what is being checked is a page served over the network, so nothing here
// should need a driver library installed to see it.
async function attach() {
  let list;
  for (let i = 0; i < 80 && !list; i++) {
    try { list = (await (await fetch(`http://127.0.0.1:${CHROME_CDP}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { await sleep(250); }
    if (list && !list.length) list = null;
  }
  if (!list) die('the stand-in browser never opened its DevTools port');
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

// A person pressing keys: real key events, nothing clicked into first. `text`
// is what makes Chrome produce the keypress a character needs
async function type(dev, text) {
  for (const ch of text) {
    const enter = ch === '\n';
    const base = enter
      ? { key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, text: '\r' }
      : { key: ch, code: 'Key' + ch.toUpperCase(), text: ch };
    await dev.call('Input.dispatchKeyEvent', { type: 'keyDown', ...base });
    await dev.call('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
    await sleep(25);
  }
}

// What the terminal is showing right now, as text
const shown = (dev) => dev.run(`(document.getElementById("screen") || {}).innerText || ""`);
// Put the shell tab in front and wait until the page agrees it is there
async function toShell(dev) {
  await until(() => dev.run(`!!(S && S.tabs && S.tabs.length)`), 'the board');
  await dev.run(`send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true`);
  await until(() => dev.run(`!!(S && S.tabs && S.tabs.some(t => t.index === S.active && t.name === "shell"))`), 'the shell tab in front');
  await sleep(1200);
}

let dev = null;
try {
  // ---- a laptop: a mouse, real keys, no touch screen ----
  console.log('\n1. a laptop looking at the board from elsewhere');
  dev = await attach();
  await dev.call('Page.enable');
  await dev.call('Runtime.enable');
  await dev.call('Page.navigate', { url: `http://127.0.0.1:${BOARD}/?t=${token}` });
  await toShell(dev);
  check(await dev.run(`typingDirect()`), 'the page works out for itself that typing belongs in the pane');
  await type(dev, 'echo laptop-typed-this\n');
  let ok = false;
  try { await until(async () => (await shown(dev)).includes('laptop-typed-this'), 'the typing to arrive', 8000); ok = true; } catch {}
  check(ok, 'keys pressed with nothing clicked reach the terminal');
  check(await dev.run(`document.activeElement === document.getElementById("kbd")`),
    'the caret sits in the pane, as it does at the window');
  dev.close();

  // ---- a phone: a touch screen, nothing to press ----
  console.log('\n2. a phone looking at the same board');
  dev = await attach();
  await dev.call('Page.enable');
  await dev.call('Runtime.enable');
  await dev.call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
  await dev.call('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  await dev.call('Page.navigate', { url: `http://127.0.0.1:${BOARD}/?t=${token}` });
  await toShell(dev);
  await dev.run(`localStorage.removeItem("shikishaTypeDirect"); localStorage.removeItem("shikishaCastClosed2"); true`);
  await dev.call('Page.reload');
  await toShell(dev);
  check(await dev.run(`matchMedia("(pointer: coarse)").matches`), 'this stand-in says it is a touch screen');
  check(await dev.run(`!typingDirect()`), 'the page leaves the caret alone on a touch screen');
  // A tap on the terminal: the bar, and NOT the caret in the pane -- the caret
  // there is what raises the soft keyboard over the very screen being read
  const box = await dev.run(`(() => { const r = document.getElementById("screen").getBoundingClientRect();
    return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2)}; })()`);
  for (const half of ['mousePressed', 'mouseReleased']) {
    await dev.call('Input.dispatchMouseEvent', { type: half, x: box.x, y: box.y, button: 'left', clickCount: 1 });
  }
  await sleep(600);
  check(await dev.run(`!!(castDock && castDock.style.display === "flex")`), 'a tap brings the sub-input bar up');
  check(await dev.run(`document.activeElement !== document.getElementById("kbd")`),
    'a tap does not raise the keyboard over the screen');
  // ...and the keyboard button in that bar hands typing back to the screen
  await dev.run(`[...document.querySelectorAll("#castpanel .castgear")].find(b => b.textContent.startsWith("⌨")).click(); true`);
  await sleep(600);
  check(await dev.run(`localStorage.getItem("shikishaTypeDirect") === "1"`), 'the choice is remembered for this machine');
  check(await dev.run(`!castDock || castDock.style.display !== "flex"`), 'the bar steps out of the way');
  await type(dev, 'echo phone-direct\n');
  let ok2 = false;
  try { await until(async () => (await shown(dev)).includes('phone-direct'), 'the typing to arrive', 8000); ok2 = true; } catch {}
  check(ok2, 'after the keyboard button, typing goes into the screen');
  // The pen is the way back, and it takes the choice back with it
  await dev.run(`document.getElementById("composerfab").click(); true`);
  await sleep(600);
  check(await dev.run(`localStorage.getItem("shikishaTypeDirect") !== "1"`), 'the pen brings the bar back for good');
  dev.close();
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
