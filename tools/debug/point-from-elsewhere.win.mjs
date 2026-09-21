/**
 * Where does a press on a relayed page land -- for a laptop with a mouse, and
 * for a phone with a finger?
 *
 *     cargo build
 *     node tools/debug/point-from-elsewhere.win.mjs
 *
 * Needs Windows, Node and Chrome. Starts this checkout's build in a folder of
 * its own with the relay on, shows it a page of this script's own serving
 * (four quadrants that write down every press they receive), and then opens
 * the relay in a headless Chrome twice -- once as a laptop and once as a touch
 * screen. Nothing of a copy somebody is using is read, written or stopped.
 *
 * Why it exists. The relayed screen could only be pointed at one way: a finger
 * dragged a pointer around and a tap clicked wherever that pointer had got to.
 * That is right for a phone, where the finger covers what it is aiming at, and
 * wrong for every machine with a mouse -- there a click landed at the drawn
 * arrow's position rather than under the pointer, and the arrow drifted, so
 * there was no way to hit anything on purpose. Nothing in the test suite can
 * see that: the page's own tests read the source, and what is wrong here is
 * where a press comes out at the far end, three processes away. So this is a
 * browser, on the other end of the real relay, pressing -- and the page being
 * pressed says where it was hit.
 *
 * What it checks, in order:
 *   laptop  the page works out for itself that this machine has a mouse, the
 *           switch for it stands in the page's own bar, a click lands where it
 *           was made, a double click arrives as one, the pointer passing over
 *           the page is seen there, and the switch really switches -- after it,
 *           a press clicks where the drawn arrow stands, not where it was made
 *   phone   the page works out the other way round, the first tap only takes
 *           hold of the screen (it must not click anything), and where the tab
 *           has no bar of its own the switch rides on the "in control" banner
 *
 * What it cannot stand in for: a hand. A finger that is 9mm across covering the
 * link it is aiming at is the whole reason the trackpad way exists, and no
 * dispatched touch is that.
 */
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-pointing');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const BOARD = 9357;          // the app's relay; 93xx is this folder's band
const WINDOW_CDP = 9358;     // the app's own window: a port of its own, so it takes none of Chrome's
const CHROME_CDP = 9359;     // the stand-in device's own DevTools port
const PAGE_PORT = 9360;      // the page being pressed, served from here

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

// The page that is pressed. Four quadrants, each saying what reached it --
// which one was clicked, which was double clicked, and which the pointer last
// passed over. Quadrants rather than coordinates, because the page is re-shaped
// to the viewer's screen and scaled on the way; a half is still a half.
//
// It says so by asking this script for a URL, which is the only way back: the
// app's browser tabs are webviews of their own and are not on the DevTools port
// its window is on, so nothing outside can read that page's variables
const PAGE = `<!doctype html><meta charset="utf-8"><title>press me</title>
<style>
 html,body { margin:0; height:100%; font:14px system-ui; }
 #g { display:grid; grid-template-columns:1fr 1fr; grid-template-rows:1fr 1fr; height:100%; }
 .q { display:flex; align-items:center; justify-content:center; font-size:40px; user-select:none; }
 #tl { background:#fde68a } #tr { background:#bfdbfe } #bl { background:#bbf7d0 } #br { background:#fecaca }
</style>
<div id="g"><div class="q" id="tl">TL</div><div class="q" id="tr">TR</div>
<div class="q" id="bl">BL</div><div class="q" id="br">BR</div></div>
<script>
 const say = (kind, q) => { try { fetch("/hit?kind=" + kind + "&q=" + q); } catch (e) {} };
 for (const q of document.querySelectorAll(".q")) {
   q.addEventListener("click", () => say("click", q.id));
   q.addEventListener("dblclick", () => say("dbl", q.id));
   // Only the move that crosses into a quadrant: a pointer sliding across one
   // is one thing happening, not forty
   q.addEventListener("mouseover", () => say("over", q.id));
 }
</script>`;

let heard = [];
const server = http.createServer((req, res) => {
  const at = new URL(req.url, "http://127.0.0.1");
  if (at.pathname === "/hit") {
    heard.push({ kind: at.searchParams.get("kind"), q: at.searchParams.get("q") });
    res.writeHead(204).end();
    return;
  }
  res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
  res.end(PAGE);
});
await new Promise((r) => server.listen(PAGE_PORT, "127.0.0.1", r));
// What the page has heard since it was last asked, and a clean sheet for the
// next press. Said out loud in a failure, so it reads as what it is
const said = (kind) => heard.filter((h) => h.kind === kind).map((h) => h.q);
const clear = () => { heard = []; };

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
    { name: 'shell', id: 'shell', command: 'cmd.exe' },
    // One page with the row of controls, and one without: the switch has a home
    // in each case, and which home is the thing being checked
    { name: 'page', id: 'page', command: `browser http://127.0.0.1:${PAGE_PORT}/`,
      nav: { back: true, forward: true, reload: true, url: true } },
    { name: 'plain', id: 'plain', command: `browser http://127.0.0.1:${PAGE_PORT}/` },
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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-pointer-'));
const chrome = spawn(findChrome(), [
  '--headless=new',
  '--remote-debugging-port=' + CHROME_CDP,
  '--user-data-dir=' + profile,
  '--window-size=1280,860',
  '--no-first-run',
  '--no-default-browser-check',
  'about:blank',
], { stdio: 'ignore' });

// One conversation with a browser -- the stand-in device's, or one of the app's
// own webviews. Raw CDP over a socket: what is being checked is a page served
// over the network, so nothing here needs a driver library installed to see it
async function attach(port = CHROME_CDP, wanted = null) {
  let hit;
  for (let i = 0; i < 80 && !hit; i++) {
    try {
      const list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter((t) => t.type === 'page');
      hit = wanted ? list.find((t) => t.url.startsWith(wanted)) : list[0];
    } catch {}
    if (!hit) await sleep(250);
  }
  if (!hit) die('nothing answered the DevTools port ' + port + (wanted ? ' with ' + wanted : ''));
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
// ...and, for a page, until its picture has actually arrived: the canvas is
// sized by the first frame, and every coordinate below is measured off it
async function toPage(dev, name) {
  await toTab(dev, name);
  await until(() => dev.run(`(() => { const c = document.getElementById("cast"); return !!c && !c.hidden && c.width > 1; })()`),
    'the first frame of ' + name);
  await sleep(700);
}

// Where on this screen the given fraction of the relayed page is, in the terms
// a press is dispatched in. Asked of the page itself, through the same castRect
// the cursor is placed with
const spotOf = (dev, fx, fy) => dev.run(
  `(() => { const r = castRect(document.getElementById("cast"));
     return {x: Math.round(r.ox + ${fx} * r.dw), y: Math.round(r.oy + ${fy} * r.dh)}; })()`);

let dev = null;
try {
  // ---- a laptop: a mouse, and no touch screen ----
  console.log('\n1. a laptop looking at a relayed page');
  dev = await attach();
  await dev.call('Page.enable');
  await dev.call('Runtime.enable');
  await dev.call('Page.navigate', { url: `http://127.0.0.1:${BOARD}/?t=${token}` });
  await toPage(dev, 'page');
  check(await dev.run(`castPoint === "direct"`),
    'the page works out for itself that this machine points with a mouse');
  check(await dev.run(`!!document.querySelector("#nav .castpoint")`),
    'the switch stands in the page\'s own row of controls');
  check(!(await dev.run(`!!(cursorEl && cursorEl.style.display === "block")`)),
    'no second pointer is drawn over the one this machine has');

  // A click, where it was made
  clear();
  let at = await spotOf(dev, 0.75, 0.75);
  for (const half of ['mousePressed', 'mouseReleased']) {
    await dev.call('Input.dispatchMouseEvent', { type: half, x: at.x, y: at.y, button: 'left', clickCount: 1, pointerType: 'mouse' });
  }
  await sleep(1200);
  check(said('click').includes('br') && !said('click').includes('tl'),
    'a click lands where it was made, found ' + JSON.stringify(said('click')));

  // A double click, as one act rather than two
  clear();
  at = await spotOf(dev, 0.25, 0.25);
  for (let n = 1; n <= 2; n++) {
    for (const half of ['mousePressed', 'mouseReleased']) {
      await dev.call('Input.dispatchMouseEvent', { type: half, x: at.x, y: at.y, button: 'left', clickCount: n, pointerType: 'mouse' });
    }
    await sleep(60);
  }
  await sleep(1200);
  check(said('dbl').includes('tl'), 'a double click arrives at the page as one, found ' + JSON.stringify(said('dbl')));

  // The pointer passing over the page is seen there: a menu that opens under a
  // pointer is half of what a page can do
  clear();
  at = await spotOf(dev, 0.75, 0.25);
  await dev.call('Input.dispatchMouseEvent', { type: 'mouseMoved', x: at.x, y: at.y, pointerType: 'mouse' });
  await sleep(1200);
  check(said('over').includes('tr'), 'the page sees the pointer pass over it, found ' + JSON.stringify(said('over')));

  // The switch. After it, a press means what a finger means: the drawn arrow is
  // what clicks, and it is standing where the last press left it (top left)
  await dev.run(`document.querySelector("#nav .castpoint").click(); true`);
  await sleep(300);
  check(await dev.run(`castPoint === "pad" && localStorage.getItem("shikishaCastPoint") === "pad"`),
    'the switch changes the way of pointing, and this machine remembers it');
  check(await dev.run(`!!(cursorEl && cursorEl.style.display === "block")`),
    'the drawn arrow comes back with the trackpad way');
  clear();
  // Where the arrow is standing, and the quadrant furthest from it: the whole
  // of the trackpad way is that those two are not the same place
  const arrow = JSON.parse(await dev.run(`JSON.stringify([cx, cy])`));
  const stands = (arrow[1] < 0.5 ? 't' : 'b') + (arrow[0] < 0.5 ? 'l' : 'r');
  const away = (arrow[1] < 0.5 ? 'b' : 't') + (arrow[0] < 0.5 ? 'r' : 'l');
  at = await spotOf(dev, arrow[0] < 0.5 ? 0.75 : 0.25, arrow[1] < 0.5 ? 0.75 : 0.25);
  for (const half of ['mousePressed', 'mouseReleased']) {
    await dev.call('Input.dispatchMouseEvent', { type: half, x: at.x, y: at.y, button: 'left', clickCount: 1, pointerType: 'mouse' });
  }
  await sleep(1200);
  check(said('click').includes(stands) && !said('click').includes(away),
    `after the switch a press clicks where the arrow stands (${stands}), not where it was made (${away}), found `
      + JSON.stringify(said('click')));
  // ...and back, so the choice is a switch and not a one-way door
  await dev.run(`document.querySelector("#nav .castpoint").click(); true`);
  await sleep(300);
  check(await dev.run(`castPoint === "direct"`), 'the switch goes back the other way');
  dev.close(); dev = null;

  // ---- a phone: a touch screen ----
  console.log('\n2. a phone looking at the same page');
  dev = await attach();
  await dev.call('Page.enable');
  await dev.call('Runtime.enable');
  await dev.call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
  await dev.call('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  await dev.call('Page.navigate', { url: `http://127.0.0.1:${BOARD}/?t=${token}` });
  await toPage(dev, 'page');
  await dev.run(`localStorage.removeItem("shikishaCastPoint"); true`);
  await dev.call('Page.reload');
  await toPage(dev, 'page');
  check(await dev.run(`matchMedia("(pointer: coarse)").matches`), 'this stand-in says it is a touch screen');
  check(await dev.run(`castPoint === "pad"`), 'the page leaves a touch screen on the trackpad way');

  clear();
  const tap = async (fx, fy) => {
    const p = await spotOf(dev, fx, fy);
    await dev.call('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: p.x, y: p.y }] });
    await sleep(80);
    await dev.call('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  };
  await tap(0.75, 0.75);
  await sleep(1200);
  check(said('click').length === 0,
    'the first tap takes hold of the screen and clicks nothing, found ' + JSON.stringify(said('click')));
  check(await dev.run(`castMode === true`), 'the first tap does take hold of the screen');

  // A page whose tab shows no row of controls: the switch rides on the banner
  await toPage(dev, 'plain');
  check(await dev.run(`document.getElementById("nav").hidden`), 'this tab really has no row of controls');
  check(await dev.run(`!!document.querySelector("#castmode .castpoint")`),
    'the switch rides on the banner where the tab has no controls of its own');
  check(!(await dev.run(`!!document.querySelector("#nav .castpoint")`)),
    'the switch has the one home and not both');
  dev.close(); dev = null;
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
