/**
 * The quick actions' settings, checked through the running app's own window.
 *
 * The page has tests and settings-shoot.mjs photographs it, but a photograph
 * is taken over events the scene dispatches itself, and the one thing those
 * cannot show is what real input does: the click a browser sends after a
 * carried row is put down, a press landing on the row under the pointer, the
 * page's Save reaching the file, and the board hearing about it.
 *
 * So this starts the app built in this checkout, in a folder of its own, with
 * five quick actions in its settings, opens Settings > Quick actions from the
 * board the way the sub-input bar's gear does, and uses the screen with the
 * mouse: a row carried by its grip to another place, a row pressed and renamed
 * in its dialog, an empty dialog whose save is held and then left by Escape,
 * and the page's Save. Every step is checked against config/config.json and
 * against what the board then holds, not only against the settings page.
 *
 *     cargo build
 *     node tools/debug/quick-actions.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder,
 * its own LOCALAPPDATA (the WebView2 store lives there), and started from its
 * own folder. Nothing of a copy somebody is using is read, written or stopped.
 *
 * The settings are a page placed in the app's *pages* WebView2 environment
 * (webview2/profiles/default), not the window's (webview2/shell): two browser
 * processes, and a fixed --remote-debugging-port reaches only the first to
 * bind it. So the copy is started with port 0, each environment picks a port
 * of its own, and both are read from the DevToolsActivePort file each writes.
 * Photographs land in target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
// Short, under the temporary folder: the WebView2 store nests deeply, and a
// long base path runs past what Windows allows
const RUN = path.join(os.tmpdir(), 'sk-actions');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });

/** This checkout's copy, stopped by where it runs from and nothing else */
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

// Five actions, in an order the check will change: the last one is carried
// up, and the third is renamed. One runs Lua, so the flag has to survive
const actions = [
  { label: 'Continue', body: 'Please continue.' },
  { label: 'Explain', body: 'Explain what you just did, briefly.' },
  { label: 'Review', body: 'Review this and point out any problems.' },
  { label: 'To reviewer', body: 'shikisha.send_to_tab("reviewer", tab.output)', lua: true },
  { label: 'Fix', body: 'Fix that and show me the change.' },
];
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: false },
  actions,
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));

// Started directly, so the isolated LOCALAPPDATA and the DevTools argument
// reach it. What says "you are inside Claude Code" is taken out, so the app is
// not told it is somebody's child session
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

/** The port an environment's browser chose, once it has written it down */
const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};

/** One DevTools page, spoken to over its socket */
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
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `quick-actions-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  const at = (sel) => run(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null;
    const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  // The system's own road, not a dispatched event: the browser makes the
  // pointer events, the mouse events and the click from these, in the order
  // and the tasks a real mouse gets
  const mouse = (type, x, y, extra = {}) => send('Input.dispatchMouseEvent', { type, x, y, button: 'left', pointerType: 'mouse', ...extra });
  const click = async (sel) => {
    const p = await at(sel);
    if (!p) throw new Error('nothing at ' + sel + ' on ' + name);
    await mouse('mouseMoved', p.x, p.y);
    await mouse('mousePressed', p.x, p.y, { clickCount: 1 });
    await mouse('mouseReleased', p.x, p.y, { clickCount: 1 });
  };
  // A key through the same road as the mouse: the browser makes the keydown
  // and keyup from it, to whatever has the caret
  const key = async (k, code = k, vk) => {
    await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: k, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk });
    await send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk });
  };
  return { ws, send, run, shot, at, mouse, click, key };
}

// The board first: it is the way into the settings, and the last thing asked
let boardTarget;
try {
  await until(async () => {
    const p = portOf('shell');
    if (!p) return false;
    boardTarget = (await targetsOf(p)).find((t) => t.type === 'page');
    return !!boardTarget;
  }, 'the window\'s page and its DevTools port', 40000);
} catch (e) { stopApp(); die(e.message); }
const board = await connect(boardTarget, 'the board');
const names = (page) => page.run('[...document.querySelectorAll(".arow .arowname")].map(e => e.textContent)');
const framed = (page) => page.run('!!document.querySelector(".modal .framed")');
const saved = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8')).actions || [];
let cfg = null;
try {
  await until(() => board.run('typeof openSettings === "function" && !!S'), 'the board script');

  console.log('1. the settings, opened on the quick actions from the board');
  await board.run('openSettings("actions")');
  let cfgTarget;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    if (!p) return false;
    cfgTarget = (await targetsOf(p)).find((t) => t.type === 'page' && /section=actions/.test(t.url));
    return !!cfgTarget;
  }, 'the settings page in the pages\' environment', 30000);
  cfg = await connect(cfgTarget, 'the settings');
  await until(() => cfg.run('document.querySelectorAll(".arow").length === 5'), 'five rows');
  check(JSON.stringify(await names(cfg)) === JSON.stringify(actions.map((a) => a.label)), 'the rows read down in the file\'s order');
  check(await cfg.run('document.querySelector(\'.arow[data-at="3"] .chip\').textContent') === 'Lua', 'the Lua action says so on its row');
  await cfg.shot('1-list');

  console.log('2. the last row, carried by its grip to the middle of the second');
  const grip = await cfg.at('.arow[data-at="4"] .agrip');
  const to = await cfg.at('.arow[data-at="1"]');
  await cfg.mouse('mouseMoved', grip.x, grip.y);
  await cfg.mouse('mousePressed', grip.x, grip.y, { clickCount: 1 });
  for (let k = 1; k <= 12; k++) {
    await cfg.mouse('mouseMoved', grip.x + (to.x - grip.x) * k / 12, grip.y + (to.y - grip.y) * k / 12);
    await sleep(40);
  }
  await sleep(200);
  check(await cfg.run('!!document.querySelector(".arow.dragging")'), 'the row is lifted while it is carried');
  await cfg.shot('2-carrying');
  await cfg.mouse('mouseReleased', to.x, to.y, { clickCount: 1 });
  await sleep(500);
  const carried = ['Continue', 'Explain', 'Fix', 'Review', 'To reviewer'];
  check(JSON.stringify(await names(cfg)) === JSON.stringify(carried), 'put down, the row is where it was let go: ' + (await names(cfg)).join(' / '));
  check(!(await framed(cfg)), 'the click that follows the drop opens nothing');
  check(await cfg.run('document.getElementById("savebtn").classList.contains("dirty")'), 'the page knows it has changed');
  await cfg.shot('3-carried');

  console.log('3. a row pressed, and renamed in its dialog');
  await cfg.click('.arow[data-at="3"]');
  await until(() => framed(cfg), 'the dialog');
  check(await cfg.run('document.querySelector(".modal .framed .mbody input[type=text]").value') === 'Review', 'the dialog opened on the row that was pressed');
  // Typed, not set: the dialog is asked to take the keys a person would send.
  // The box is pressed with the mouse first, so the caret is really in it
  await cfg.click('.modal .framed .mbody input[type=text]');
  await until(() => cfg.run('document.activeElement === document.querySelector(".modal .framed .mbody input[type=text]")'), 'the caret in the name box');
  await cfg.run('document.activeElement.select()');
  await cfg.send('Input.insertText', { text: 'Review it' });
  await until(() => cfg.run('document.querySelector(".modal .framed .mbody input[type=text]").value === "Review it"'), 'the name typed');
  await cfg.click('.modal .framed .mfoot .primary');
  await until(async () => !(await framed(cfg)), 'the dialog to close');
  check((await names(cfg))[3] === 'Review it', 'the new name is on the row');
  await cfg.shot('4-renamed');

  console.log('4. an empty dialog: held, and left by Escape with nothing behind');
  await cfg.click('#actionslist ~ .row button');
  await until(() => framed(cfg), 'the add dialog');
  await cfg.click('.modal .framed .mfoot .primary');
  await sleep(300);
  const why = await cfg.run('(() => { const w = document.querySelector(".modal .framed .mfoot .why"); return w && !w.hidden ? w.textContent : null; })()');
  check(!!why, 'the save is held, and says why: ' + why);
  check(await framed(cfg), 'the dialog is still open');
  await cfg.shot('5-held');
  await cfg.key('Escape', 'Escape', 27);
  await until(async () => !(await framed(cfg)), 'the dialog to leave on Escape');
  check((await names(cfg)).length === 5, 'Escape left nothing in the list');
  // The key was the dialog's alone. It used to reach the page as well, which
  // asked -- for the whole sheet -- whether to throw the unsaved work away,
  // and that question took every press after it until it was answered
  const asked = await cfg.run('(() => { const d = document.querySelector("dialog[open]"); return d ? d.textContent.trim().slice(0, 60) : null; })()');
  check(!asked, 'Escape closed the dialog and nothing else' + (asked ? ' -- but the page asks: ' + asked : ''));
  // The page still takes a press after the key: a row opens, Cancel closes
  await cfg.click('.arow[data-at="0"]');
  let opened = false;
  try { await until(() => framed(cfg), 'a row to open after Escape', 5000); opened = true; } catch (e) {}
  check(opened, 'after Escape, a press on a row still opens its dialog');
  if (opened) {
    await cfg.click('.modal .framed .mfoot .quiet:not(.icon)');
    await until(async () => !(await framed(cfg)), 'Cancel to close the dialog');
  }

  console.log('5. the page saved, and the file read back');
  // What is under the button and whether the press reaches it: said when the
  // file does not change, so a miss can be told from a save that failed
  await cfg.run('window.__pressed = 0; document.getElementById("savebtn").addEventListener("click", () => { window.__pressed++; }, {capture:true});');
  await cfg.click('#savebtn');
  try {
    await until(() => saved()[3]?.label === 'Review it', 'config.json to carry the change');
  } catch (e) {
    const why = await cfg.run(`(() => { const b = document.getElementById("savebtn"); const r = b.getBoundingClientRect();
      const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      const open = document.querySelector("dialog[open]");
      return { pressed: window.__pressed, button: b.className + " / " + b.textContent, under: top ? (top.id || top.className) : null,
        dialogOpen: open ? open.className + ": " + open.textContent.trim().slice(0, 60) : null,
        active: document.activeElement ? document.activeElement.tagName + "#" + document.activeElement.id : null,
        message: [...document.querySelectorAll("#msg, #result, .toast, .status")].map(x => x.textContent.trim()).filter(Boolean),
        visible: document.visibilityState, focus: document.hasFocus() }; })()`);
    const shown = await board.run('(S && S.settings) ?? null');
    throw new Error(e.message + ' -- ' + JSON.stringify({ ...why, sheetOpenOnBoard: shown }));
  }
  const file = saved();
  const want = [['Continue', false], ['Explain', false], ['Fix', false], ['Review it', false], ['To reviewer', true]];
  check(want.every(([label, lua], i) => file[i] && file[i].label === label && !!file[i].lua === lua),
    'config.json holds the carried order, the new name and the Lua flag: ' + file.map((a) => a.label + (a.lua ? ' (lua)' : '')).join(' / '));
  check(file[4].body === actions[3].body, 'the Lua itself is kept as it was');
  check(!file.some((a) => 'lua' in a && !a.lua), 'no row writes a lua flag it does not need');
  await cfg.shot('6-saved');

  console.log('6. the board, told of the change without a relaunch');
  await until(() => board.run('(typeof curActions !== "undefined" ? curActions : []).map(a => a.label).join() === ' + JSON.stringify(want.map((w) => w[0]).join())), 'the board to hold the new order', 15000);
  const held = await board.run('curActions.map(a => a.label + (a.lua ? " (lua)" : ""))');
  check(held[4] === 'To reviewer (lua)', 'the board holds the actions in the saved order, the Lua one as Lua: ' + held.join(' / '));
} catch (e) {
  check(false, e.message);
} finally {
  if (cfg) cfg.ws.close();
  board.ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\quick-actions-*.png');
process.exit(failures ? 1 : 0);
