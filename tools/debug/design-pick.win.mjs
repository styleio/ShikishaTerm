/**
 * Picking parts of a page for an AI (🎯), through the running app's own window.
 *
 * This checkout's build in a folder of its own, with a small page served here
 * in a browser tab and a stand-in AI tab beside it: a program named `claude`
 * that turns bracketed paste on (as the AI CLIs do) and writes down every byte
 * it is given. The page is pressed through the browser's own input, the way a
 * person's mouse or the phone's relayed finger reaches it.
 *
 *     cargo build
 *     node tools/debug/design-pick.win.mjs
 *
 * Checked:
 *   unasked   a page reporting a pick while nobody armed it is not listed
 *   armed     the panel's switch arms the page; a press picks instead of pressing
 *             (the button's own handler does not run), and the chip appears
 *   note      a note written on a chip is kept by the app
 *   escape    Escape on the page puts picking away, on the board as well
 *   handed    "Hand to an AI" puts the description into the stand-in's input,
 *             with the note, inside bracketed paste, and no Enter after it
 *   phone     the page a phone is served offers the same 🎯 panel on a page
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
const RUN = path.join(os.tmpdir(), 'sk-pick');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const HEARD = path.join(RUN, 'heard.bin');
// --ja runs the same checks with the screen in Japanese, for the photographs
const JA = process.argv.includes('--ja');
// --split runs the same checks with the window and the runtime as two programs
const SPLIT_MODE = process.argv.includes('--split');

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

// ── The page ──────────────────────────────────
let pressed = 0;
const server = http.createServer((req, res) => {
  if (req.url === '/pressed') { pressed += 1; res.end('ok'); return; }
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Settings</title>
<style>.card{display:flex;gap:12px;padding:16px;border-radius:8px;background:#f3f5f8;margin:40px}
#save{padding:6px 14px;font-size:15px;background:#2266dd;color:#fff;border:0;border-radius:6px}</style></head>
<body><main><section class="card"><h2>Profile</h2>
<button id="save" data-token="secret123" onclick="fetch('/pressed')">Save changes</button></section></main></body></html>`);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;

// ── The copy ──────────────────────────────────
console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

// The stand-in AI: bracketed paste on, and every byte written down
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
const PHONE_KEY = 'pickphone0123456789abcdef';
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  ...(SPLIT_MODE ? { split: true } : {}),
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  desks: [{ name: 'Pick', id: 'pick', folders: [{ cwd: WORK, tabs: [
    { name: 'claude', id: 'ai', command: path.join(WORK, 'claude.cmd') },
    { name: 'page', id: 'page', command: `browser http://127.0.0.1:${pagePort}/` },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
// Where the pages are drawn: the window's own engine, or -- the window and
// the runtime being two programs -- a browser the runtime started
const pagesPort = () => {
  if (!SPLIT_MODE) return portOf(path.join('profiles', 'default'));
  const f = path.join(LOCAL, 'ShikishaTerm', 'chromium', 'default', 'DevToolsActivePort');
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
    fs.writeFileSync(path.join(SHOTS, `design-pick-${JA ? "ja-" : ""}${label}.png`), Buffer.from(r.data, 'base64'));
  };
  const mouse = (type, x, y, extra = {}) => send('Input.dispatchMouseEvent', { type, x, y, button: 'left', pointerType: 'mouse', ...extra });
  const press = async (x, y) => {
    await mouse('mouseMoved', x, y);
    await mouse('mousePressed', x, y, { clickCount: 1 });
    await mouse('mouseReleased', x, y, { clickCount: 1 });
  };
  return { ws, send, run, shot, press };
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
  await until(() => board.run('typeof renderPanel === "function" && !!S && S.tabs.length >= 2'), 'the board and its tabs', 30000);
  const pageTab = await board.run('S.tabs.find(t => t.kind === "browser").index');
  await board.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => board.run(`S.active === ${pageTab}`), 'the page in front');
  await until(async () => {
    const p = pagesPort();
    return p && (pageTarget = (await targetsOf(p)).find((t) => t.type === 'page' && t.url.includes(String(pagePort))));
  }, 'the page\'s own browser', 30000);
  const page = await connect(pageTarget);
  await until(() => page.run('!!window.__shikisha_pick && document.readyState === "complete"'), 'the page script');
  const picks = () => board.run('JSON.stringify((S.tabs.find(t => t.kind === "browser") || {}).picks || null)').then(JSON.parse);
  const centre = () => page.run('(() => { const r = document.getElementById("save").getBoundingClientRect(); return {x: r.left + r.width/2, y: r.top + r.height/2}; })()');

  console.log('1. a page reporting a pick nobody asked for');
  await page.run('window.__shikisha_post(JSON.stringify({kind:"picked", item:{tag:"button", name:"forged"}}))');
  await sleep(1500);
  check((await picks()) === null, 'nothing is listed for the page');

  console.log('2. armed from the panel, and a press on the button');
  // A window over a runtime of its own is served the page a far device is,
  // and opens the bar over a page the way a phone does
  await board.run('ensureBar(); if (typeof REMOTE !== "undefined" && REMOTE) enterCast(); else openTermBar(); castPanel = "pick"; userPanel = "pick"; renderPanel();');
  await board.run('send({kind:"pick", on:true})');
  await until(async () => (await picks())?.on === true, 'the page armed');
  await until(() => page.run('!!document.documentElement.lastElementChild && getComputedStyle(document.documentElement.lastElementChild).position === "fixed"'), 'the drawing on the page');
  const c = await centre();
  await page.press(c.x, c.y);
  await until(async () => (await picks())?.items?.length === 1, 'the pick listed');
  const got = await picks();
  check(got.items[0].label === 'button "Save changes"', 'the chip names it: ' + got.items[0].label);
  await sleep(600);
  check(pressed === 0, 'the button itself was not pressed');
  await until(() => board.run('document.querySelectorAll("#castpick .pchip").length === 1'), 'the chip on the panel');
  await page.shot('1-armed-page');

  console.log('3. a note on the chip');
  await board.run('(() => { const i = document.querySelector("#castpick .pchip input"); i.value = "make it green"; i.dispatchEvent(new Event("change")); })()');
  await until(async () => (await picks())?.items?.[0]?.note === 'make it green', 'the note kept');
  check(true, 'the note is the app\'s');
  await board.shot('2-panel');

  console.log('4. Escape on the page');
  await page.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
  await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
  await until(async () => (await picks())?.on === false, 'picking put away');
  check((await picks()).items.length === 1, 'what was picked stays');
  await page.press(c.x, c.y);
  await until(() => pressed === 1, 'the button pressed as usual', 5000).catch(() => {});
  check(pressed === 1, 'with picking away, a press presses again');

  console.log('5. handed to the AI tab');
  const before = fs.existsSync(HEARD) ? fs.readFileSync(HEARD).length : 0;
  await board.run('pickAsk("send", {to: "ai"})');
  await until(() => fs.existsSync(HEARD) && fs.readFileSync(HEARD, 'utf8').includes('\x1b[201~'), 'the draft in the stand-in', 20000);
  await sleep(1500);
  const heard = fs.readFileSync(HEARD, 'utf8').slice(before);
  check(heard.startsWith('\x1b[200~'), 'it arrived as one bracketed paste');
  check(heard.includes('Elements I picked') && heard.includes('Save changes') && heard.includes('make it green'), 'with the element and the note');
  check(heard.includes('Selector: #save'), 'with its selector');
  check(!heard.includes('secret123'), 'a secret-looking attribute is not handed over');
  check(heard.endsWith('\x1b[201~'), 'and nothing after it (no Enter)');
  await until(async () => (await picks()) === null, 'the list emptied once handed');
  check(true, 'handed picks leave the list');

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
  await until(() => phone.run('typeof panelOptions === "function" && !!S && S.active != null'), 'the phone\'s board', 30000);
  await phone.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => phone.run(`S.active === ${pageTab}`), 'the page in front on the phone');
  check((await phone.run('panelOptions()')).includes('pick'), 'the phone offers 🎯 on a page');
  await phone.run('send({kind:"pick", on:true})');
  await until(async () => (await picks())?.on === true, 'armed from the phone');
  check(true, 'the phone arms the page');
  const c2 = await centre();
  await page.press(c2.x, c2.y);
  await until(async () => (await picks())?.items?.length === 1, 'a pick made while the phone armed it');
  await phone.run('enterCast(); castPanel = "pick"; userPanel = "pick"; renderPanel();');
  await until(() => phone.run('document.querySelectorAll("#castpick .pchip").length === 1'), 'the chip on the phone');
  check(true, 'the phone lists the same pick');
  await sleep(500);
  await phone.shot('3-phone');
  await phone.run('send({kind:"pick", on:false})');
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
