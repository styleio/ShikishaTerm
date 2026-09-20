/**
 * How far behind is the picture, and is video further behind than stills?
 *
 *     cargo build --release --bin SHIKISHA-TERM
 *     node tools/debug/relay-delay.win.mjs [--seconds 40]
 *
 * Needs Windows, Chrome and Node. Isolated the way the other tools here are:
 * its own folder, its own settings, its own port, started from its own copy.
 * Nothing of a copy somebody is using is read, written or stopped. The
 * RELEASE build, because a debug build spends 70ms on a picture that the
 * release build spends 2ms on and every number below would be that instead.
 *
 * Why it exists. Video costs a third of the CPU and a ninth of the bytes, and
 * none of that is worth anything if the picture arrives later than it used
 * to: this screen is watched while somebody types into it. An encoder holds
 * frames back to compress them and a receiver holds them again to smooth the
 * network out, and neither shows up in a screenshot or in any log.
 *
 * How it measures. The copy it starts is pointed at a page served from here
 * that flips between near-black and near-white and writes down the instant
 * the browser finished painting each flip. A Chrome stands in for the phone,
 * opens the relay, and watches the middle pixel of the picture it is sent --
 * out of the canvas when it arrives as stills, out of the video element when
 * it arrives as video. The delay is when the viewer saw the change minus when
 * the page painted it. Both ends are on this machine, so both read the same
 * clock and there is nothing to synchronise.
 *
 * Flat colours on purpose: an encoder spends its effort where the detail is,
 * and a whole screen changing colour is the one thing it cannot send late
 * without it showing.
 *
 * The middle number is the one to compare. The worst is the one a person
 * notices, so it is printed too.
 */
import fs from 'node:fs';
import os from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-delay');
const APP = path.join(RUN, 'app');
const CONFIG = path.join(APP, 'config', 'config.json');
const RELAY_PORT = 8796;     // the relay a viewer opens
const PAGE_PORT = 8901;      // the page being watched, served from here
const TOKEN = 'delay-token-0123456789abcdef';
const SECONDS = Number((process.argv[process.argv.indexOf('--seconds') + 1]) || 40);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

// ── The page being watched ────────────────────
const FLIPPER = `<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>flip</title><style>html,body{margin:0;height:100%;overflow:hidden}
#f{width:100%;height:100%;background:#0a0a0c}</style></head><body>
<div id="f"></div>
<script>
  const f = document.getElementById('f');
  let dark = true;
  function flip() {
    dark = !dark;
    f.style.background = dark ? '#0a0a0c' : '#f4f4f8';
    // Reported after the browser has painted it, not when it was asked: the
    // delay being measured starts at the picture existing. Sent back here
    // rather than left in the page, because a page the app placed is a
    // browser of its own and does not appear on any DevTools port
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const at = Date.now();
      fetch('/flip?at=' + at + '&dark=' + (dark ? 1 : 0), { keepalive: true }).catch(() => {});
    }));
    setTimeout(flip, 1000 + Math.round(Math.random() * 500));
  }
  setTimeout(flip, 1200);
</script></body></html>`;

const flips = [];
const server = http.createServer((req, res) => {
  if (req.url.startsWith('/flip')) {
    const q = new URL(req.url, 'http://127.0.0.1');
    flips.push({ at: Number(q.searchParams.get('at')), dark: q.searchParams.get('dark') === '1' });
    res.writeHead(204).end();
    return;
  }
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(FLIPPER);
}).listen(PAGE_PORT, '127.0.0.1');

// ── A copy of the app, its own ────────────────
const exe = path.join(ROOT, 'target', 'release', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no release build -- cargo build --release --bin SHIKISHA-TERM');

console.log('starting a copy of this build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  // Loopback and a token of its own: a measurement must not put a door on
  // the network, and the viewer below is on this machine anyway
  remote: { enabled: true, bind: '127.0.0.1', port: RELAY_PORT, sticky_token: true, fixed_token: TOKEN },
  // One tab, and it is the page being watched. A page that is not the tab on
  // screen is not being drawn -- no animation frames, no screen relay -- so a
  // desk with anything else in it can start on the wrong one and measure a
  // still picture very accurately
  desks: [{ name: 'Delay', id: 'delay', browsers: [{ id: 'flip', url: `http://127.0.0.1:${PAGE_PORT}/` }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

// The app is up once its own door answers
for (let i = 0; i < 160; i++) {
  try { if ((await fetch(`http://127.0.0.1:${RELAY_PORT}/?t=${TOKEN}`)).ok) break; } catch { /* not yet */ }
  await sleep(250);
}
console.log('the copy is up');

// ── A viewer, standing in for the phone ───────
function chromeAt(dbgPort) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'delay-'));
  const chrome = process.env.CHROME
    || path.join(process.env.PROGRAMFILES, 'Google', 'Chrome', 'Application', 'chrome.exe');
  const p = spawn(chrome, [
    '--remote-debugging-port=' + dbgPort, '--user-data-dir=' + profile,
    '--window-size=412,915', '--autoplay-policy=no-user-gesture-required',
    '--no-first-run', '--no-default-browser-check', 'about:blank',
  ], { stdio: 'ignore' });
  return { p, profile };
}

// The little of the DevTools protocol a viewer needs
async function pageAt(port) {
  for (let i = 0; i < 160; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const t = list.find((x) => x.type === 'page');
      if (t) return t;
    } catch { /* not up yet */ }
    await sleep(250);
  }
  return null;
}

async function talk(target) {
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
  return {
    send,
    run: async (expression) => {
      const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
      return r.result.value;
    },
    close: () => ws.close(),
  };
}

// `stills` is measured by taking WebRTC away from the viewer, which is what a
// browser that cannot do it looks like -- the page checks for exactly that
// before it offers anything. Nothing in the product is switched
async function measure(how, dbg) {
  const { p, profile } = chromeAt(dbg);
  const target = await pageAt(dbg);
  if (!target) { p.kill(); die('the viewer never opened'); }
  const view = await talk(target);
  await view.send('Page.enable');
  await view.send('Runtime.enable');
  await view.send('Emulation.setDeviceMetricsOverride',
    { width: 412, height: 915, deviceScaleFactor: 2, mobile: true });
  if (how === 'stills') {
    await view.send('Page.addScriptToEvaluateOnNewDocument', { source: 'delete window.RTCPeerConnection;' });
  }
  await view.send('Page.navigate', { url: `http://127.0.0.1:${RELAY_PORT}/?t=${TOKEN}` });
  await sleep(4000);

  // Ask for the tab with the page in it, the way a person taps it. The app
  // opens on its home screen, and a tab that is not on screen is not drawn --
  // no animation frames, no relay -- so without this the measurement is of a
  // page that is not running
  await view.run(`(() => {
    const t = (S.tabs || []).find((x) => x.name === 'flip');
    if (t) send({ kind: 'select', tab: t.index });
    return !!t;
  })()`);
  // Then start again, now that the right tab is on screen. Asking for video
  // happens once, when the page opens; a tab chosen after that would be
  // measured on a connection that was agreed for the tab before it
  await view.send('Page.navigate', { url: `http://127.0.0.1:${RELAY_PORT}/?t=${TOKEN}` });
  // Long enough for the picture to start and, where it is going to, for video
  // to take over from the line it opened first
  await sleep(12000);

  await view.run(`
    window.__seen = [];
    window.__scratch = document.createElement('canvas');
    window.__scratch.width = 8; window.__scratch.height = 8;
    window.__last = null;
    window.__watch = setInterval(() => {
      const v = document.getElementById('castv');
      const c = document.getElementById('cast');
      const g = window.__scratch.getContext('2d', { willReadFrequently: true });
      try {
        if (v && !v.hidden && v.videoWidth) {
          g.drawImage(v, v.videoWidth / 2 - 4, v.videoHeight / 2 - 4, 8, 8, 0, 0, 8, 8);
        } else if (c && c.width) {
          g.drawImage(c, c.width / 2 - 4, c.height / 2 - 4, 8, 8, 0, 0, 8, 8);
        } else return;
      } catch (e) { return; }
      const d = g.getImageData(3, 3, 1, 1).data;
      const dark = (d[0] + d[1] + d[2]) / 3 < 128;
      if (window.__last === null) { window.__last = dark; return; }
      if (dark !== window.__last) { window.__last = dark; window.__seen.push({ at: Date.now(), dark }); }
    }, 4); true`);

  await sleep(SECONDS * 1000);

  const got = await view.run(`(() => {
    clearInterval(window.__watch);
    const v = document.getElementById('castv');
    return JSON.stringify({ seen: window.__seen,
      as: v && !v.hidden && v.videoWidth ? 'video' : 'stills',
      size: v && v.videoWidth ? v.videoWidth + 'x' + v.videoHeight : null });
  })()`);
  view.close();
  p.kill();
  await sleep(300);
  fs.rmSync(profile, { recursive: true, force: true });
  return { ...JSON.parse(got), flips: flips.slice() };
}

// Each change the viewer saw, against the flip to that colour just before it
const delays = (flips, seen) => seen.flatMap((s) => {
  const before = flips.filter((f) => f.at <= s.at && f.dark === s.dark);
  if (!before.length) return [];
  const d = s.at - before[before.length - 1].at;
  return d >= 0 && d < 3000 ? [d] : [];
});
const middle = (a) => (a.length ? [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)] : null);

const rows = [];
try {
  for (const [how, dbg] of [['stills', 9341], ['video', 9342]]) {
    const got = await measure(how, dbg);
    if (got.flips.length < 2) {
      console.log(`${how}: the page being watched never painted -- is its tab on screen?`);
    }
    const ds = delays(got.flips, got.seen);
    rows.push({ how, arrived: got.as, n: ds.length, middle: middle(ds), worst: ds.length ? Math.max(...ds) : null });
    console.log(`${how.padEnd(6)} arrived as ${got.as}${got.size ? ' ' + got.size : ''}: ` +
      `${ds.length} changes seen, middle ${middle(ds)}ms, worst ${ds.length ? Math.max(...ds) : '-'}ms`);
  }
} finally {
  server.close();
  stopApp();
}

const s = rows.find((r) => r.how === 'stills');
const v = rows.find((r) => r.how === 'video');
if (!v || v.arrived !== 'video') {
  console.log('\nthe video path never carried anything, so there is nothing to compare');
  process.exit(1);
}
if (s && s.middle != null && v.middle != null) {
  const d = v.middle - s.middle;
  console.log(`\nvideo is ${d >= 0 ? d + 'ms behind' : -d + 'ms ahead of'} stills, middle to middle`);
}
