/**
 * What a phone watching a page is actually sent: which way the picture comes
 * (video or stills), how big the pictures are against the phone's own screen,
 * how many arrive a second, how many bytes that costs, and what the PC spends
 * making them -- for an ordinary page and for that page's DevTools.
 *
 * Measures; checks nothing but that there was a picture to measure. Written
 * after a phone said the DevTools picture was "blurry and heavy", so that the
 * answer is numbers rather than a guess, and so that a change to the relay can
 * be put side by side with the numbers from before it.
 *
 *     cargo build
 *     node tools/debug/cast-measure.win.mjs [--seconds 10] [--dpr 3] [--split]
 *
 * The phone is a headless Chrome the size of a common phone (390x844 CSS
 * pixels, `--dpr` device pixels to one, 3 by default: most phones sold now).
 * The page moves on its own (a bar sweeping, a console line every 100 ms), so
 * there is something to send. The DevTools is opened from the phone, the way
 * a person there would, and shown on the Console panel, where those lines land.
 *
 * Printed per screen: the way, the picture's size, the phone's canvas in
 * device pixels, pictures per second, kilobytes per second, and the PC's CPU
 * (the app and the browser processes it started, as a share of one core).
 *
 * Needs Windows, Node and Chrome. Nothing of a copy somebody is using is read,
 * written or stopped.
 */
import {findChrome, connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-cast-measure');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SHOTS = path.join(ROOT, 'target', 'shots');
const arg = (name, dflt) => { const i = process.argv.indexOf(name); return i > 0 ? Number(process.argv[i + 1]) : dflt; };
const SECONDS = arg('--seconds', 10);
const DPR = arg('--dpr', 3);
const SPLIT_MODE = process.argv.includes('--split');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};

/** CPU seconds spent so far by the app and every browser process it started */
function cpuSeconds() {
  const r = ps('-Command',
    `$t = 0; Get-CimInstance Win32_Process | Where-Object { ` +
    `($_.ExecutablePath -and $_.ExecutablePath -like '${RUN}\\*') -or ` +
    `(($_.Name -eq 'msedgewebview2.exe' -or $_.Name -eq 'chrome.exe') -and ($_.CommandLine + '') -like '*sk-cast-measure\\localappdata*') } | ` +
    `ForEach-Object { $t += [double]$_.KernelModeTime + [double]$_.UserModeTime }; $t / 1e7`);
  return Number(r.stdout.trim()) || 0;
}

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

// A page that never stops changing: a bar sweeping across and a line said to
// the console ten times a second
const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Moving</title>
<style>body{font:16px system-ui;margin:0;padding:24px}#bar{height:40px;background:#3a7;width:10%}</style></head>
<body><h1>A page that moves</h1><div id="bar"></div><p id="n">0</p>
<script>let n=0;setInterval(()=>{n++;document.getElementById("n").textContent=n;
document.getElementById("bar").style.width=(10+(n%80))+"%";console.log("tick",n,{at:Date.now()});},100);</script>
</body></html>`);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;

const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});
const PHONE_PORT = await freePort();
const PHONE_KEY = 'castmeasure0123456789abc';

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  ...(SPLIT_MODE ? { split: true } : {}),
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  // Answered already, so the start does not stop on the question about the
  // AI CLIs' hooks (this checks nothing about them)
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Cast', id: 'cast', folders: [{ cwd: WORK, tabs: [
    { name: 'page', id: 'page', command: `browser http://127.0.0.1:${pagePort}/` },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
async function connect(target) {
  const {ws, send, run} = await connectCdp(target);
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `cast-measure-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, send, run, shot };
}


// Counted on the phone: every picture that comes down the stills line (a
// listener put on the page's own line, again whenever the line is a new one),
// and the video line's own numbers
const HOOK = `(() => {
  window.__stills = window.__stills || {n: 0, bytes: 0, w: 0, h: 0};
  if (typeof castWs !== "undefined" && castWs && !castWs.__counted) {
    castWs.__counted = true;
    castWs.addEventListener("message", (e) => {
      if (!e.data || !e.data.size) return;
      __stills.n++; __stills.bytes += e.data.size;
      createImageBitmap(e.data).then((b) => { __stills.w = b.width; __stills.h = b.height; if (b.close) b.close(); }).catch(() => {});
    });
  }
})()`;

async function sample(phone) {
  await phone.run(HOOK);
  return phone.run(`(async () => {
    const way = (document.getElementById("castway") || {}).textContent || "";
    let v = null;
    if (typeof videoPc !== "undefined" && videoPc && videoOn) {
      const st = await videoPc.getStats();
      st.forEach((r) => { if (r.type === "inbound-rtp" && r.kind === "video") v = {bytes: r.bytesReceived, frames: r.framesDecoded, w: r.frameWidth, h: r.frameHeight}; });
    }
    const cv = document.getElementById(videoOn ? "castv" : "cast");
    const box = cv ? cv.getBoundingClientRect() : {width: 0, height: 0};
    return {way, videoOn: typeof videoOn !== "undefined" && videoOn, v, stills: {...__stills},
      css: [Math.round(box.width), Math.round(box.height)], dpr: devicePixelRatio};
  })()`);
}

async function measure(phone, label) {
  // Settle: the first pictures and the video's start are not the steady state
  await sleep(4000);
  const a = await sample(phone);
  const c0 = cpuSeconds();
  const t0 = Date.now();
  await sleep(SECONDS * 1000);
  const b = await sample(phone);
  const c1 = cpuSeconds();
  const secs = (Date.now() - t0) / 1000;
  const video = b.videoOn && a.v && b.v;
  const frames = video ? b.v.frames - a.v.frames : b.stills.n - a.stills.n;
  const bytes = video ? b.v.bytes - a.v.bytes : b.stills.bytes - a.stills.bytes;
  const size = video ? `${b.v.w}x${b.v.h}` : `${b.stills.w}x${b.stills.h}`;
  const screen = `${Math.round(b.css[0] * b.dpr)}x${Math.round(b.css[1] * b.dpr)}`;
  console.log(`  ${label.padEnd(9)} way=${video ? 'video' : 'stills'} (${b.way || '-'})  picture=${size}  phone=${screen} (css ${b.css.join('x')} @${b.dpr})  ` +
    `${(frames / secs).toFixed(1)} pictures/s  ${(bytes / secs / 1024).toFixed(0)} KB/s  PC CPU ${(100 * (c1 - c0) / secs).toFixed(0)}% of a core`);
  await phone.shot(label);
}

let chrome = null;
const PHONE_DIR = path.join(RUN, 'phone');
try {
  console.log(`phone: 390x844 css @${DPR}, ${SECONDS}s per screen${SPLIT_MODE ? ', split mode' : ''}`);
  fs.mkdirSync(PHONE_DIR, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + PHONE_DIR,
    '--no-first-run', '--no-default-browser-check', '--autoplay-policy=no-user-gesture-required', 'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(PHONE_DIR, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const pport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let pt;
  await until(async () => (pt = (await targetsOf(pport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connect(pt);
  await phone.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: DPR, mobile: true });
  await until(async () => {
    await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
    await sleep(1500);
    return phone.run('typeof S !== "undefined" && !!S && S.tabs.some(t => t.kind === "browser")');
  }, 'the board on the phone', 60000);

  const select = async (key) => {
    await phone.run(`send({kind:"select", tab: S.tabs.find(t => (t.id || t.name) === ${JSON.stringify(key)}).index})`);
    await until(() => phone.run(`S.tabs.find(t => t.index === S.active) && (S.tabs.find(t => t.index === S.active).id || S.tabs.find(t => t.index === S.active).name) === ${JSON.stringify(key)}`), 'the tab in front on the phone');
    await until(() => phone.run('!document.getElementById("castway").hidden'), 'a picture on the phone', 30000);
  };

  await select('page');
  await measure(phone, 'page');

  // Opened from the phone, as a person there would
  await phone.run('send({kind:"devtools", page:"page"})');
  await until(() => phone.run('S.tabs.some(t => (t.id || t.name) === "page-devtools")'), 'the DevTools as a tab', 30000);
  const folder = await phone.run(`(() => { const t = S.tabs.find(x => (x.id || x.name) === "page-devtools"); const g = S.groups && S.groups[t.group]; return g ? g.folder || "" : ""; })()`);
  console.log(`  the DevTools opened from the phone stands in: ${folder || '(no folder)'}`);
  await select('page-devtools');
  await measure(phone, 'devtools');
  // A press on the picture lands where it is seen: the DevTools' "Console"
  // tab, found by eye on the photograph at (0.59, 0.024) of the picture at
  // 390 CSS pixels across, is pressed, and the photograph after it should
  // show the Console open -- and then the Console, where the page's lines
  // arrive ten a second, is measured
  await phone.run(`(() => { for (const phase of ["pressed", "released"])
    sendIn({kind:"inject", what:"mouse", phase, x: 0.59, y: 0.024, down: phase === "pressed", clicks: 1}); })()`);
  await sleep(2500);
  await phone.shot('devtools-pressed');
  await measure(phone, 'console');
} catch (e) {
  console.error('  FAIL ' + e.message);
  process.exitCode = 1;
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
  server.close();
}
