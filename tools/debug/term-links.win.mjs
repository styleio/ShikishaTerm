/**
 * Addresses and file paths on a terminal's screen, pressed through the running
 * app's own window and through a phone.
 *
 * Starts the app built in this checkout, in a folder of its own, with one
 * terminal tab that prints what a day in a terminal prints: an address served
 * here, a compiler's `path:line:col`, a Japanese file name, a path that names
 * nothing, a file outside the tab's folder, an address too long for one row,
 * and a program's own hyperlink (OSC 8). Then it presses them with the mouse,
 * the way a person does, and checks what happens -- not what the page says it
 * would do:
 *
 *   - each is one place on the screen, and the long address is one place over two rows
 *   - the path's list offers the editor at that line; choosing it opens the
 *     file there with the cursor on that line
 *   - a path that names nothing says so, and offers no editor
 *   - a file outside the folder offers the editor greyed, and says why when pressed
 *   - the address's list opens a browser tab in the folder, written into the settings
 *   - Ctrl+click opens at once, with no list
 *   - with the setting at "Nothing", a plain press brings up nothing
 *   - a phone gets the same places, its own browser instead of the PC's, and
 *     none of what would open on the PC's screen
 *
 *     cargo build
 *     node tools/debug/term-links.win.mjs
 *
 * `LINKS_SPLIT=1` runs the same with the program split in two (Settings >
 * Basic > Screen and work): the window is then a page on the board like the
 * phone's, and is checked to be known as this PC's own window -- offered what
 * opens on this PC, let through a press only it may make, and kept out of the
 * list of devices.
 *
 * Needs Windows, Node and Chrome (for the phone). Isolated the way a new-user
 * run is: its own folder and LOCALAPPDATA. What would open on this PC's screen
 * (the default app, Explorer, this PC's browser) is not pressed. Photographs
 * land in target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-links');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const ELSEWHERE = path.join(RUN, 'elsewhere');
const CONFIG = path.join(APP, 'config', 'config.json');
// The words the screen uses, in the language asked for (`LINKS_LANG=ja`): the
// checks read the lists by what they say, so they read them in that language
const LANG = process.env.LINKS_LANG || 'en';
const SPLIT = process.env.LINKS_SPLIT === '1';
const readLang = (l) => JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', l + '.json'), 'utf8'));
const L = { ...readLang('en'), ...(LANG === 'en' ? {} : readLang(LANG)) };
const AT12 = L['tui.link.edit_line'].replaceAll('{line}', '12');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-links*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
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

// The page an address points at, so a browser tab opened on it has something
let served = 0;
const site = http.createServer((q, s) => { served += 1; s.writeHead(200, { 'content-type': 'text/html' }); s.end('<title>here</title><h1>here</h1>'); });
await new Promise((r) => site.listen(0, '127.0.0.1', r));
const ADDRESS = `http://127.0.0.1:${site.address().port}/hello?x=1`;
const LONG = 'https://example.com/' + 'abcdefghij'.repeat(26);
// The same length again, broken at the edge by a line break rather than run
// on past it: the shape a screen drawn again by the pseudo console comes in
const BROKEN = 'https://example.net/' + 'klmnopqrst'.repeat(26);
// A program's file links that name a machine, the way `ls --hyperlink` writes
// them: this PC by its own name, and a share on a computer that is not there
const HERE_LINK = 'file://' + os.hostname() + '/' + path.join(WORK, 'src', 'main.rs').replaceAll('\\', '/');
const SHARE_LINK = 'file://nas-nowhere-9/share/a%20b.txt';
const SHARE_UNC = '\\\\nas-nowhere-9\\share\\a b.txt';

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS, ELSEWHERE, path.join(WORK, 'src'), path.join(WORK, '資料')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

fs.writeFileSync(path.join(WORK, 'src', 'main.rs'), Array.from({ length: 40 }, (_, i) => `// line ${i + 1}`).join('\n') + '\n');
fs.writeFileSync(path.join(WORK, '資料', '議事録・秋.md'), '# 議事録\n\n一行目\n');
const OUTSIDE = path.join(ELSEWHERE, 'notes.txt');
fs.writeFileSync(OUTSIDE, 'outside\n');
// What the terminal prints, one place per line, then waits so the screen stays
fs.writeFileSync(path.join(WORK, 'say.mjs'), [
  `import fs from 'node:fs';`,
  `console.log('page: ${ADDRESS}');`,
  `console.log('error at src/main.rs:12:5');`,
  `console.log('memo 資料/議事録・秋.md');`,
  `console.log('gone ./src/nothere.rs:3');`,
  `console.log('far ' + ${JSON.stringify(OUTSIDE)});`,
  `console.log('long ${LONG}');`,
  `process.stdout.write('\\x1b]8;;https://example.org/osc\\x1b\\\\labelled link\\x1b]8;;\\x1b\\\\ after\\n');`,
  `process.stdout.write('\\x1b]8;;${HERE_LINK}\\x1b\\\\on this pc\\x1b]8;;\\x1b\\\\ \\x1b]8;;${SHARE_LINK}\\x1b\\\\on a share\\x1b]8;;\\x1b\\\\\\n');`,
  // Cut at exactly the width the screen is drawn at, once the check has
  // measured it there and left it in width.txt. Not the console's own idea
  // of its width: that is read once when the program starts, and the
  // terminal is sized again after -- the very difference this is about
  `const cut = setInterval(() => { let w = 0; try { w = Number(fs.readFileSync('width.txt', 'utf8')); } catch {} if (!(w > 0)) return; clearInterval(cut); const s = 'cut ${BROKEN}'; const out = []; for (let i = 0; i < s.length; i += w) out.push(s.slice(i, i + w)); process.stdout.write(out.join('\\r\\n') + '\\r\\n'); }, 200);`,
  `setInterval(() => {}, 1 << 30);`,
].join('\n'));

const PHONE_PORT = await freePort();
const PHONE_KEY = 'linkphone0123456789abcd';
const writeConfig = (more = {}) => fs.writeFileSync(CONFIG, JSON.stringify({
  language: LANG,
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'node say.mjs' },
    // Split, a prompt to paste into: the right-click paste is the other press
    // only this PC's window may make
    ...(SPLIT ? [{ name: 'typing', id: 'typing', command: 'cmd.exe' }] : [])] }] }],
  ...(SPLIT ? { split: true } : {}),
  ...more,
}, null, 2));
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
writeConfig();

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
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());

async function connect(target, name) {
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
    if (r.exceptionDetails) throw new Error(name + ': ' + (r.exceptionDetails.exception?.description || r.exceptionDetails.text));
    return r.result.value;
  };
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `term-links-${LANG}${SPLIT ? '-split' : ''}-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  // The middle of the first element of a place, found by what it opens
  const placeAt = (go) => run(`(() => { const e = [...document.querySelectorAll("#screen .lk")].find(x => x.dataset.go === ${JSON.stringify(go)});
    if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  const mouse = (type, x, y, extra = {}) => send('Input.dispatchMouseEvent', { type, x, y, button: 'left', pointerType: 'mouse', ...extra });
  const press = async (go, modifiers = 0) => {
    const p = await placeAt(go);
    if (!p) throw new Error('no place on ' + name + ' for ' + go);
    await mouse('mouseMoved', p.x, p.y);
    await mouse('mousePressed', p.x, p.y, { clickCount: 1, modifiers });
    await mouse('mouseReleased', p.x, p.y, { clickCount: 1, modifiers });
  };
  const tap = async (go) => {
    const p = await placeAt(go);
    if (!p) throw new Error('no place on ' + name + ' for ' + go);
    await send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: p.x, y: p.y }] });
    await send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  };
  const menu = () => run('(() => { const m = document.querySelector(".fmenu"); if (!m) return null; ' +
    'return { rows: [...m.querySelectorAll(":scope > div")].map(d => ({ text: d.textContent, cls: d.className, hidden: d.hidden })) }; })()');
  const choose = async (label) => {
    const p = await run(`(() => { const d = [...document.querySelectorAll(".fmenu > div")].find(x => x.textContent === ${JSON.stringify(label)});
      if (!d) return null; const r = d.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
    if (!p) throw new Error('no ' + label + ' in the list on ' + name);
    await mouse('mouseMoved', p.x, p.y);
    await mouse('mousePressed', p.x, p.y, { clickCount: 1 });
    await mouse('mouseReleased', p.x, p.y, { clickCount: 1 });
  };
  const away = () => run('closeFolderMenu(); true');
  return { ws, send, run, shot, press, tap, menu, choose, away, placeAt };
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

let boardTarget;
try {
  // Split in two, the window is a process of its own showing the board's
  // page, opened with the key it was handed (`here=`); otherwise it is the
  // window's own page. Looked for in every browser this copy started
  await until(async () => {
    const base = path.join(LOCAL, 'ShikishaTerm', 'webview2');
    if (!fs.existsSync(base)) return false;
    for (const envName of fs.readdirSync(base)) {
      const p = portOf(envName);
      if (!p) continue;
      const pages = (await targetsOf(p).catch(() => [])).filter((t) => t.type === 'page');
      boardTarget = pages.find((t) => (SPLIT ? /[?&]here=/.test(t.url) : envName === 'shell'));
      if (boardTarget) return true;
    }
    return false;
  }, 'the window\'s page', 60000);
} catch (e) { stopApp(); die(e.message); }
const board = await connect(boardTarget, 'the board');
const backToShell = async () => {
  await until(() => board.run('S.tabs.find(t => t.index === S.active).name === "shell" && document.getElementById("editpanel").hidden && document.querySelectorAll("#screen .lk").length >= 6'), 'the terminal in front again');
  await sleep(500);
};
const texts = (m) => (m ? m.rows.filter((r) => !r.hidden).map((r) => r.text) : []);

try {
  await until(() => board.run('!!S && document.querySelectorAll("#screen .lk").length >= 6'), 'the places on the screen', 30000);

  if (SPLIT) {
    console.log('0. the window of a program split in two is this PC\'s own window');
    check(await board.run('REMOTE === true && AT_PC === true'), 'its page comes from the board and knows it is at this PC');
    const devices = () => {
      const b = path.join(APP, 'data', 'clients.json');
      return fs.existsSync(b) ? (JSON.parse(fs.readFileSync(b, 'utf8')).clients || []).length : 0;
    };
    check(devices() === 0, 'it is not written into the list of devices');
    check(!(await board.run('!!document.getElementById("remotecut")')), 'it is not taken for a phone ("a phone is connected" is not up)');
    // A press a phone is refused, with an effect that can be read without
    // opening anything on this PC's screen: "not now" on the star card
    const asked = path.join(APP, 'data', 'thanks-asked');
    await board.run('send({kind:"thanks", open:false}); true');
    await until(() => fs.existsSync(asked), 'the answer to be written down', 10000).catch(() => {});
    check(fs.existsSync(asked), 'a press only this PC\'s window may make reached the app');
    // This PC's clipboard, pasted by a right-click on the window. Whatever
    // was on the clipboard is put back afterwards
    const kept = path.join(RUN, 'clipboard.txt');
    ps('-Command', `Get-Clipboard -Raw | Set-Content -NoNewline -Encoding utf8 -LiteralPath '${kept}'`);
    const word = 'pasted' + Math.random().toString(36).slice(2, 8);
    ps('-Command', `Set-Clipboard -Value '${word}'`);
    try {
      await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "typing").index}); true');
      await until(() => board.run('S.tabs.find(t => t.index === S.active).name === "typing"'), 'the prompt in front');
      await sleep(1500);
      await board.run('send({kind:"paste"}); true');
      await until(() => board.run(`document.getElementById("screen").textContent.includes(${JSON.stringify(word)})`), 'the paste on screen', 10000).catch(() => {});
      check(await board.run(`document.getElementById("screen").textContent.includes(${JSON.stringify(word)})`), 'a right-click pasted this PC\'s clipboard into the tab');
    } finally {
      ps('-Command', `if (Test-Path -LiteralPath '${kept}') { Get-Content -Raw -Encoding utf8 -LiteralPath '${kept}' | Set-Clipboard }`);
    }
    await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
    await until(() => board.run('S.tabs.find(t => t.index === S.active).name === "shell"'), 'the terminal in front again');
  }

  console.log('1. every place on the screen is one place');
  // The width the screen is drawn at: the first row of the long address runs
  // to the edge, and all of it is plain ASCII, one character to a column
  const cols = await board.run(`(() => { const rows = [...document.getElementById("screen").children];
    const at = rows.filter(r => [...r.querySelectorAll(".lk")].some(e => e.dataset.go.startsWith("https://example.com/")));
    return at.length >= 2 ? at[0].textContent.replace(/\\s+$/, "").length : 0; })()`);
  check(cols > 20, 'the screen is ' + cols + ' columns wide');
  fs.writeFileSync(path.join(WORK, 'width.txt'), String(cols));
  // The address cut at the edge is printed as soon as the width is there
  await until(() => board.run(`[...document.querySelectorAll("#screen .lk")].some(e => e.dataset.go.startsWith("https://example.net/"))`), 'the address cut at the edge', 20000).catch(() => {});
  await sleep(500);
  const places = await board.run('[...document.querySelectorAll("#screen .lk")].map(e => ({ go: e.dataset.go, lk: e.dataset.lk, at: e.dataset.at, text: e.textContent }))');
  const gos = new Set(places.map((p) => p.go));
  for (const go of [ADDRESS, 'src/main.rs:12:5', '資料/議事録・秋.md', './src/nothere.rs:3', OUTSIDE, LONG, BROKEN, 'https://example.org/osc']) {
    check(gos.has(go), 'found: ' + go.slice(0, 60));
  }
  // Counted by the rows its pieces stand in, and all starting at one place
  const rowsOf = (list) => new Set(list.map((p) => p.row)).size;
  const placesWithRows = () => board.run('[...document.querySelectorAll("#screen .lk")].map(e => ({ go: e.dataset.go, at: e.dataset.at, row: [...e.parentNode.parentNode.children].indexOf(e.parentNode) }))');
  const onePlace = async (go) => {
    const list = (await placesWithRows()).filter((p) => p.go === go);
    return { ok: rowsOf(list) >= 2 && new Set(list.map((p) => p.at)).size === 1, rows: rowsOf(list) };
  };
  let one = await onePlace(LONG);
  check(one.ok, 'the long address is one place over ' + one.rows + ' rows');
  one = await onePlace(BROKEN);
  check(one.ok, 'the address broken at the edge by a line break is one place over ' + one.rows + ' rows');
  const osc = places.filter((p) => p.go === 'https://example.org/osc').map((p) => p.text).join('');
  check(osc === 'labelled link', 'the program\'s own link covers its words and nothing after: "' + osc + '"');
  check(!gos.has('page:'), 'the words around a place are not part of it');
  await board.shot('1-screen');

  // The window made narrower, and wider again, after the text is out: the
  // terminal is drawn again at each width. The address that ran on past the
  // edge must still be one address -- how wide the window happens to be when
  // the tool starts is not something to depend on. (The one cut by hand is
  // not asked about here: at another width its cut is no longer at the edge,
  // and it is two lines, as the program wrote it)
  console.log('1b. the long address after the terminal is made narrower and wider');
  const wide = await board.run('({ w: innerWidth, h: innerHeight })');
  await board.send('Emulation.setDeviceMetricsOverride', { width: Math.max(700, Math.round(wide.w * 0.7)), height: wide.h, deviceScaleFactor: 1, mobile: false });
  await sleep(2500);
  one = await onePlace(LONG);
  check(one.ok, 'narrower: still one place, over ' + one.rows + ' rows');
  await board.send('Emulation.clearDeviceMetricsOverride');
  await sleep(2500);
  one = await onePlace(LONG);
  check(one.ok, 'wider again: still one place, over ' + one.rows + ' rows');

  console.log('2. a compiler\'s place opens the editor on that line');
  await board.press('src/main.rs:12:5');
  await until(() => board.menu(), 'the list');
  let m = await board.menu();
  check(texts(m).includes(AT12), 'the list offers the editor at line 12: ' + texts(m).join(' | '));
  check(texts(m).includes(L['tui.link.app']) && texts(m).includes(L['tui.link.reveal']) && texts(m).includes(L['tui.link.copy_path']),
    'and the PC\'s program, its folder, and the path');
  await board.shot('2-list');
  await board.choose(AT12);
  await until(() => board.run('ED.path === "src/main.rs" && !!edAce && edAce.getValue().startsWith("// line 1")'), 'the file in the editor');
  await until(() => board.run('edAce.getCursorPosition().row === 11'), 'the cursor on line 12', 5000).catch(() => {});
  check(await board.run('edAce.getCursorPosition().row === 11'), 'the cursor is on line 12 (row ' + (await board.run('edAce.getCursorPosition().row')) + ')');
  await board.shot('3-editor');
  // Back to the terminal for the rest
  await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
  await backToShell();

  console.log('3. a path that names nothing says so');
  await board.press('./src/nothere.rs:3');
  await until(() => board.menu(), 'the list');
  m = await board.menu();
  check(texts(m).some((t) => t === L['tui.link.missing']), 'the list says there is nothing there');
  check(!texts(m).some((t) => t === L['tui.link.edit'] || t === AT12), 'and offers no editor');
  await board.away();

  console.log('3b. a file link that names a machine');
  // This PC's own name: a path here, in the folder, so the editor is offered
  await board.press(HERE_LINK);
  await until(() => board.menu(), 'the list');
  m = await board.menu();
  check(texts(m).includes(L['tui.link.edit']) && !texts(m).includes(L['tui.link.missing']),
    'a link naming this PC is the file here, offered to the editor: ' + texts(m).join(' | '));
  await board.away();
  // Another computer: its share by the UNC path, offered without asking the
  // computer first (it is not there, and asking would hold the screen)
  const asked = Date.now();
  await board.press(SHARE_LINK);
  await until(() => board.menu(), 'the list');
  const took = Date.now() - asked;
  m = await board.menu();
  // The list's first row is the place: the address, then where it is
  check(texts(m)[0].endsWith(SHARE_UNC), 'a link naming another computer is its share: ' + texts(m).join(' | '));
  const editRow = m.rows.find((r) => r.text === L['tui.link.edit']);
  check(!editRow || /\boff\b/.test(editRow.cls), 'the editor is not offered for another computer\'s file');
  check(took < 5000, 'and the list comes up at once (' + took + ' ms), the computer not asked');
  await board.away();

  console.log('4. a file outside the folder: the editor greyed, and why');
  await board.press(OUTSIDE);
  await until(() => board.menu(), 'the list');
  m = await board.menu();
  const ed = m.rows.find((r) => r.text === L['tui.link.edit']);
  check(!!ed && /\boff\b/.test(ed.cls), 'the editor is on the list, greyed');
  check(texts(m).includes(L['tui.link.app']), 'the default app is offered instead');
  await board.choose(L['tui.link.edit']);
  await sleep(200);
  m = await board.menu();
  const why = m && m.rows.find((r) => /\bbad\b/.test(r.cls) && !r.hidden);
  check(!!why && why.text === L['tui.link.outside'], 'pressed, it says why: ' + (why ? why.text : '(nothing)'));
  check(!(await board.run('ED.path === "notes.txt"')), 'and nothing opened');
  await board.shot('4-outside');
  await board.away();

  console.log('5. an address opens a browser tab in the folder');
  await board.press(ADDRESS);
  await until(() => board.menu(), 'the list');
  m = await board.menu();
  check(JSON.stringify(texts(m).slice(1)) === JSON.stringify([L['tui.link.page'], L['tui.link.pc'], L['tui.link.copy_url']]),
    'the address\'s list: ' + texts(m).slice(1).join(' | '));
  await board.shot('5-address');
  await board.choose(L['tui.link.page']);
  const tabOf = () => {
    const c = JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
    return (c.desks[0].folders[0].tabs || []).find((t) => String(t.command) === 'browser ' + ADDRESS);
  };
  await until(tabOf, 'the browser tab in the settings', 10000).catch(() => {});
  check(!!tabOf(), 'the settings have a browser tab on the address, in the folder');
  await until(() => served > 0, 'the page to be asked for', 15000).catch(() => {});
  check(served > 0, 'and the page was loaded (' + served + ' requests)');
  const count = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8')).desks[0].folders[0].tabs.length;
  const before = count();
  // The new tab is brought to the front once it is up; back to the terminal
  // only after that, or the bringing lands on top of the going back
  const pageInFront = `(() => { const t = S.tabs.find(x => x.index === S.active); return !!t && t.kind === "browser"; })()`;
  await until(() => board.run(pageInFront), 'the browser tab in front', 20000).catch(() => {});
  check(await board.run(pageInFront), 'the browser tab was brought to the front');
  await sleep(1000);
  await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
  await backToShell();

  console.log('6. Ctrl+click opens at once');
  await board.press(ADDRESS, 2);
  await sleep(1500);
  check(!(await board.menu()), 'no list came up');
  check(count() === before, 'the same address is the tab already there, not a second one');
  await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
  await backToShell();
  await board.press('資料/議事録・秋.md', 2);
  await until(() => board.run('ED.path === "資料/議事録・秋.md"'), 'the Japanese file in the editor', 10000).catch(() => {});
  check(await board.run('ED.path === "資料/議事録・秋.md"'), 'Ctrl+click on the Japanese path opened it in the editor');
  check(!(await board.menu()), 'with no list');
  await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
  await backToShell();

  console.log('7. a phone gets the same places, and its own browser');
  fs.mkdirSync(PHONE_DIR, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + PHONE_DIR,
    '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(PHONE_DIR, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const cport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let pageT;
  await until(async () => (pageT = (await targetsOf(cport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connect(pageT, 'the phone');
  await phone.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await phone.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 1 });
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
  await until(() => phone.run('typeof S !== "undefined" && !!S && document.querySelectorAll("#screen .lk").length >= 3'), 'the places on the phone', 30000);
  // A page just opened can still be putting itself together (a first state,
  // a reload to match the build); a tap in that moment is lost the way a
  // person's would be, so it is tapped again rather than failed on
  await sleep(1500);
  for (let i = 0; i < 3 && !(await phone.menu()); i++) {
    await phone.tap(ADDRESS);
    await until(() => phone.menu(), 'the list on the phone', 4000).catch(() => {});
  }
  await until(() => phone.menu(), 'the list on the phone');
  m = await phone.menu();
  check(texts(m).includes(L['tui.link.here']) && !texts(m).includes(L['tui.link.pc']),
    'the phone offers its own browser, not the PC\'s: ' + texts(m).slice(1).join(' | '));
  // A soft keyboard coming up would cover the list the tap brought up
  check(await phone.run('!castInput || document.activeElement !== castInput'), 'and the tap did not put the caret in the input bar');
  await phone.shot('6-phone-address');
  await phone.away();
  await phone.tap('src/main.rs:12:5');
  await until(() => phone.menu(), 'the path\'s list on the phone');
  m = await phone.menu();
  check(texts(m).includes(AT12), 'the phone offers the editor');
  check(!texts(m).includes(L['tui.link.app']) && !texts(m).includes(L['tui.link.reveal']),
    'and nothing that would open on the PC\'s screen: ' + texts(m).slice(1).join(' | '));
  await phone.shot('7-phone-path');
  await phone.away();

  console.log('8. with the setting at "Nothing", a press is the terminal\'s');
  writeConfig({ terminal_links: 'off' });
  await until(() => board.run('S.link_press === "off"'), 'the setting to reach the board', 15000);
  await board.press('src/main.rs:12:5');
  await sleep(1000);
  check(!(await board.menu()), 'a plain press brings up nothing');
} catch (e) {
  check(false, 'stopped: ' + e.message);
  await board.shot('stopped').catch(() => {});
  console.log(await board.run('(() => { const e = [...document.querySelectorAll("#screen .lk")].find(x => x.dataset.go === "./src/nothere.rs:3"); if (!e) return "none"; const r = e.getBoundingClientRect(); const top = document.elementFromPoint(r.left + r.width/2, r.top + r.height/2); return (top && (top.id + "." + top.className + ":" + (top.textContent || "").slice(0, 30))) + " " + JSON.stringify(r); })()'));
  console.log(await board.run('JSON.stringify({waiting: linkWait && {ask: linkWait.ask, go: linkWait.go}, active: S.active, tabs: S.tabs.map(t => [t.index, t.name, t.kind]), panes: S.panes, lk: document.querySelectorAll("#screen .lk").length})').catch((x) => x.message));
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
  site.close();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
