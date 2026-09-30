/**
 * The browser bar's Develop list, through the running app's own window: picking
 * parts of a page for an AI, the page's source and DOM, and the hard reload.
 *
 * This checkout's build in a folder of its own, with a small page served here
 * in a browser tab and a stand-in AI tab beside it: a program named `claude`
 * that turns bracketed paste on (as the AI CLIs do) and writes down every byte
 * it is given. The page is pressed through the browser's own input, the way a
 * person's mouse or the phone's relayed finger reaches it.
 *
 *     cargo build
 *     node tools/debug/design-pick.win.mjs [--ja] [--split]
 *
 * Checked:
 *   unasked   a page reporting a pick while nobody armed it is not listed
 *   armed     "Pick elements for the AI" in the Develop list arms the page and
 *             calls up Picked elements in the right-hand column (the page
 *             stepping aside while the list is over it); a press picks instead
 *             of pressing (the button's own handler does not run), and a row
 *             appears
 *   note      a note written on a row is kept by the app
 *   escape    Escape on the page puts picking away, on the board as well
 *   handed    "Hand to an AI" puts the description into the stand-in's input,
 *             with the note, inside bracketed paste, and no Enter after it
 *   source    "Source code" opens what the server sent, read from the browser
 *             (the page is not fetched again) in an editor that only reads, and
 *             a save of it is refused
 *   dom       "DOM" opens the page as it stands, with what its script added
 *   hard      "Hard reload" fetches the page again
 *   phone     a phone's Develop list arms the page and puts the column aside for
 *             the page to be pressed; its edge brings back the list and Stop
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

// A key in the shape a big issuer gives them, made up here, and a value the
// app holds in its own secrets: both are on the page, and neither may leave
const SHOWN_KEY = 'ghp_' + 'aB3cD9eF1gH7iJ5kL0mN2oP4qR6sT8uVwXy';
const HELD = 'plain-words-held-9';
// ── The page ──────────────────────────────────
let pressed = 0;
// How often the page itself was fetched: the source must not fetch it again
let served = 0;
const server = http.createServer((req, res) => {
  if (req.url === '/pressed') { pressed += 1; res.end('ok'); return; }
  if (req.url === '/') served += 1;
  // The page may be kept and must be asked about again (what most pages
  // say); /kept-nowhere is a page its server forbids keeping at all
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8',
    'cache-control': req.url === '/kept-nowhere' ? 'no-store' : 'no-cache' });
  res.end(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Settings</title>
<style>.card{display:flex;gap:12px;padding:16px;border-radius:8px;background:#f3f5f8;margin:40px}
#save{padding:6px 14px;font-size:15px;background:#2266dd;color:#fff;border:0;border-radius:6px}</style></head>
<body><main><section class="card"><h2>Profile</h2>
<button id="save" data-token="secret123" onclick="fetch('/pressed')">Save changes</button></section>
<p id="keys">Your key: ${SHOWN_KEY} (and ${HELD})</p></main>
<script>document.querySelector("main").insertAdjacentHTML("beforeend", "<p id=late>added by script</p>")</script></body></html>`);
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
fs.writeFileSync(path.join(APP, 'config', 'secrets.json'), JSON.stringify({ tokens: { 'notify/pick/held': HELD } }));
fs.writeFileSync(CONFIG, JSON.stringify({
  secrets: 'secrets.json',
  language: JA ? 'ja' : 'en',
  // Answered already, so the start does not stop on the question about the
  // AI CLIs' hooks (this checks nothing about them)
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  ...(SPLIT_MODE ? { split: true } : {}),
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  desks: [{ name: 'Pick', id: 'pick', folders: [{ cwd: WORK, tabs: [
    { name: 'claude', id: 'ai', command: path.join(WORK, 'claude.cmd') },
    { name: 'page', id: 'page', command: `browser http://127.0.0.1:${pagePort}/`, nav: { reload: true, develop: true } },
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
  await until(() => board.run('typeof devRows === "function" && !!S && S.tabs.length >= 2'), 'the board and its tabs', 30000);
  await board.run('if (sideWidth() <= 0) setSideWidth(420)');
  // The Develop list: its button on the bar, and one of its rows by its words
  const develop = async (on, key) => {
    await until(() => on.run('!!document.querySelector("#nav .navdev")'), 'the Develop button', 10000);
    await on.run('document.querySelector("#nav .navdev").click()');
    await until(() => on.run('!!document.querySelector(".fmenu.devlist")'), 'the Develop list', 5000);
    const said = await on.run(`[...document.querySelectorAll(".fmenu.devlist > div")].map(d => d.textContent)`);
    await on.run(`[...document.querySelectorAll(".fmenu.devlist > div")].find(d => d.textContent === T[${JSON.stringify(key)}]).click()`);
    return said;
  };
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

  console.log('2. armed from the Develop list, and a press on the button');
  await until(() => board.run('!!document.querySelector("#nav .navdev")'), 'the Develop button', 10000);
  await board.run('document.querySelector("#nav .navdev").click()');
  await until(() => board.run('!!document.querySelector(".fmenu.devlist")'), 'the Develop list', 5000);
  const rows = await board.run('[...document.querySelectorAll(".fmenu.devlist > div")].map(d => d.textContent)');
  const wanted = await board.run('["tui.dev.hard", "tui.dev.pick", "tui.dev.devtools", "tui.dev.source", "tui.dev.dom"].map(k => T[k])');
  check(JSON.stringify(rows) === JSON.stringify(wanted),
    'the list: ' + rows.join(' | '));
  if (!SPLIT_MODE) check(await board.run('listCovers === true'), 'the page steps aside while the list is over it');
  await board.shot('0-develop');
  await board.run(`[...document.querySelectorAll(".fmenu.devlist > div")].find(d => d.textContent === T["tui.dev.pick"]).click()`);
  check(await board.run('listCovers === false'), 'and comes back when it is gone');
  check(await board.run('sidePanel === "picks" && sideCalled.has("picks") && !document.getElementById("pickpanel").hidden'),
    'Picked elements stands in the column, called up');
  check(!(await board.run('panelOptionsHere().includes("pick")')), 'the input bar no longer offers it');
  await until(async () => (await picks())?.on === true, 'the page armed');
  await until(() => page.run('!!document.documentElement.lastElementChild && getComputedStyle(document.documentElement.lastElementChild).position === "fixed"'), 'the drawing on the page');
  const c = await centre();
  await page.press(c.x, c.y);
  await until(async () => (await picks())?.items?.length === 1, 'the pick listed');
  // ...and the line showing a key
  const k = await page.run('(() => { const r = document.getElementById("keys").getBoundingClientRect(); return {x: r.left + 5, y: r.top + r.height/2}; })()');
  await page.press(k.x, k.y);
  await until(async () => (await picks())?.items?.length === 2, 'the second pick listed');
  check((await picks()).hidden >= 2, 'the key and the held value are counted as hidden: ' + (await picks()).hidden);
  await until(() => board.run('!!document.querySelector("#pickpanel .fsay .warn")'), 'the count on the panel', 5000).catch(() => {});
  check(await board.run('!!document.querySelector("#pickpanel .fsay .warn")'), 'the panel says how many were hidden');
  const got = await picks();
  check(got.items[0].label === 'button "Save changes"', 'the chip names it: ' + got.items[0].label);
  await sleep(600);
  check(pressed === 0, 'the button itself was not pressed');
  await until(() => board.run('document.querySelectorAll("#pickpanel .prow").length === 2'), 'the rows on the panel');
  await page.shot('1-armed-page');

  console.log('3. a note on the row');
  await board.run('(() => { const i = document.querySelector("#pickpanel .prow input"); i.value = "make it green"; i.dispatchEvent(new Event("change")); })()');
  await until(async () => (await picks())?.items?.[0]?.note === 'make it green', 'the note kept');
  check(true, 'the note is the app\'s');
  await board.shot('2-panel');

  console.log('4. Escape on the page');
  await page.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
  await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
  await until(async () => (await picks())?.on === false, 'picking put away');
  check((await picks()).items.length === 2, 'what was picked stays');
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
  check(!heard.includes(SHOWN_KEY) && !heard.includes(HELD) && heard.includes('[hidden]'), 'a key on the page and a held secret are not handed over');
  check(heard.endsWith('\x1b[201~'), 'and nothing after it (no Enter)');
  await until(async () => (await picks()) === null, 'the list emptied once handed');
  check(true, 'handed picks leave the list');

  console.log('6. the source, as the server sent it');
  await board.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => board.run(`S.active === ${pageTab}`), 'the page in front again');
  const was = served;
  await develop(board, 'tui.dev.source');
  await until(() => board.run('(() => { const t = S.tabs.find(t => t.index === S.active); return !!t && t.kind === "editor" && t.read_only; })()'), 'a read-only editor in front', 15000);
  await until(() => board.run('!!edAce && edAce.getValue().includes("Save changes")'), 'the source in it', 15000);
  // The script's own words are in the source; the element it made, as the
  // browser writes one out (quoted attribute), is only in the DOM
  check(await board.run(`!edAce.getValue().includes('<p id="late">') && edAce.getValue().includes("insertAdjacentHTML")`), 'what the server sent, before the script ran');
  check(served === was, 'the page was not fetched again (' + (served - was) + ')');
  check(await board.run('edAce.getReadOnly() === true && edUi.save.style.display === "none"'), 'the editor only reads, and offers no save');
  await board.run('editWrite({overwrite: true})');
  await until(() => board.run('ED.bad === true'), 'the save answered', 5000).catch(() => {});
  check(await board.run('ED.bad === true && ED.said === T["err.page_view.read_only"]'), 'a save is refused: ' + (await board.run('ED.said')));
  await board.shot('5-source');

  console.log('7. the DOM, as it stands');
  await board.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => board.run(`S.active === ${pageTab}`), 'the page in front again');
  await develop(board, 'tui.dev.dom');
  await until(() => board.run('(() => { const t = S.tabs.find(t => t.index === S.active); return !!t && t.kind === "editor" && t.read_only && /DOM/.test(t.file || ""); })()'), 'the DOM editor in front', 15000);
  await until(() => board.run(`!!edAce && edAce.getValue().includes('<p id="late">')`), 'the DOM in it', 15000);
  check(true, 'the DOM carries what the script added');

  console.log('7b. a page its server forbids keeping');
  await board.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => board.run(`S.active === ${pageTab}`), 'the page in front again');
  await page.run(`location.href = "/kept-nowhere"`).catch(() => {});
  await until(() => page.run('location.pathname === "/kept-nowhere" && document.readyState === "complete"'), 'the kept-nowhere page', 15000);
  const editorsBefore = await board.run('S.tabs.filter(t => t.kind === "editor").length');
  await develop(board, 'tui.dev.source');
  await until(() => board.run('S.flash === T["err.browser.source_not_kept"] || (S.flash || "").includes(T["err.browser.source_not_kept"])'), 'the reason said', 10000).catch(() => {});
  check(await board.run('(S.flash || "").includes(T["err.browser.source_not_kept"])'), 'it says why there is no source, and where to look instead: ' + (await board.run('S.flash')));
  check((await board.run('S.tabs.filter(t => t.kind === "editor").length')) === editorsBefore, 'and opens no empty editor');
  await page.run(`location.href = "/"`).catch(() => {});
  await until(() => page.run('location.pathname === "/" && document.readyState === "complete" && !!window.__shikisha_pick'), 'the page back', 15000);

  console.log('8. the hard reload');
  await board.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => board.run(`S.active === ${pageTab}`), 'the page in front again');
  const before8 = served;
  await develop(board, 'tui.dev.hard');
  await until(() => served > before8, 'the page fetched again', 10000).catch(() => {});
  check(served > before8, 'Hard reload fetches the page again');
  await until(() => page.run('document.readyState === "complete" && !!window.__shikisha_pick'), 'the page back', 15000).catch(() => {});

  console.log('9. the phone');
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
  // A finger, not a pointer that hovers: what makes the page say how to stop
  // with the panel rather than with Escape
  await phone.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
  await until(() => phone.run('typeof panelOptions === "function" && !!S && S.active != null'), 'the phone\'s board', 30000);
  await phone.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => phone.run(`S.active === ${pageTab}`), 'the page in front on the phone');
  check(await phone.run('window.matchMedia("(hover: none)").matches'), 'the phone is a screen with no hover');
  await develop(phone, 'tui.dev.pick');
  await until(async () => (await picks())?.on === true, 'armed from the phone');
  check(true, 'the phone arms the page from its Develop list');
  check(await phone.run('sideStoodAside === true && document.getElementById("side").hidden'), 'the column steps aside for the page to be pressed');
  const c2 = await centre();
  await page.press(c2.x, c2.y);
  await until(async () => (await picks())?.items?.length === 1, 'a pick made while the phone armed it');
  await phone.run('document.getElementById("sidegrip").click()');
  await until(() => phone.run('!document.getElementById("side").hidden && sidePanel === "picks"'), 'the list back from the edge');
  await until(() => phone.run('document.querySelectorAll("#pickpanel .prow").length === 1'), 'the row on the phone');
  check(true, 'the phone lists the same pick');
  // The button's own description tells a finger to stop with the button,
  // never with a key the phone does not have
  const told = await phone.run('document.querySelector("#pickpanel .chead button").title');
  const meant = await phone.run('T["tui.pick.hint_touch"] || ""');
  check(!!meant && told === meant && !/\bEsc\b/.test(told), 'the phone\'s stop button says to stop with it: "' + told + '"');
  await sleep(500);
  await phone.shot('3-phone');
  await phone.shot('4-phone-page');
  await phone.run('document.querySelector("#pickpanel .chead button").click()');
  await until(async () => (await picks())?.on === false, 'stopped from the phone\'s panel');
  check(true, 'the phone stops picking with the panel\'s button');
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
