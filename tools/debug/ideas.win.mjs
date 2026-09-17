/**
 * The ideas window, checked through the running app's own window.
 *
 * The page has tests and ideas.rs has tests; what neither can see is the join
 * between them -- the press reaching the app, the app writing the file, and the
 * answer finding its way back into the page while somebody is still typing.
 *
 * So this starts the app built in this checkout, in a folder of its own, with a
 * desk of two project folders, and uses the ideas the way a person does: the
 * bulb, typing, Enter, Shift+Enter, ticking one off, carrying one by its grip,
 * a card with no project, and a project taken out of the settings. Every step
 * is checked against config/ideas.json itself, not only against the window.
 *
 *     cargo build
 *     node tools/debug/ideas.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder,
 * its own LOCALAPPDATA (the WebView2 store lives there), and started from its
 * own folder. Nothing of a copy somebody is using is read, written or stopped.
 * Photographs land in target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const PORT = 9342;
// Short, under the temporary folder: the WebView2 store nests deeply, and a
// long base path runs past what Windows allows
const RUN = path.join(os.tmpdir(), 'sk-ideas');
const APP = path.join(RUN, 'app');
const APPDIR = path.join(RUN, 'work', 'app');
const NOTES = path.join(RUN, 'work', 'notes');
const FILE = path.join(APP, 'config', 'ideas.json');
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
for (const d of [APP, APPDIR, NOTES, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

const settings = (folders) => ({
  language: 'ja',
  remote: { enabled: false },
  desks: [{ name: 'Check', id: 'check', folders }],
});
const appFolder = { cwd: APPDIR, tabs: [{ name: 'app-shell', id: 'app-shell', command: 'cmd.exe' }] };
const notesFolder = { cwd: NOTES, tabs: [{ name: 'notes-shell', id: 'notes-shell', command: 'cmd.exe' }] };
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify(settings([appFolder, notesFolder]), null, 2));

// Started directly, so the isolated LOCALAPPDATA and the DevTools port reach
// it. What says "you are inside Claude Code" is taken out, so the app is not
// told it is somebody's child session
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

let targets;
for (let i = 0; i < 80 && !targets; i++) {
  try { targets = (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { await sleep(250); }
  if (targets && !targets.length) targets = null;
}
if (!targets) { stopApp(); die('the app\'s page never opened its DevTools port'); }
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
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
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};
const at = async (sel) => run(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null;
  const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
const click = async (sel) => {
  const p = await at(sel);
  if (!p) throw new Error('nothing to click: ' + sel);
  for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
    await send('Input.dispatchMouseEvent', { type, x: p.x, y: p.y, button: 'left', clickCount: 1 });
  }
  await sleep(200);
};
const type = (text) => send('Input.insertText', { text });
const key = async (name, modifiers = 0) => {
  const code = { Enter: 13, Escape: 27, End: 35 }[name];
  const base = { key: name, code: name, windowsVirtualKeyCode: code, modifiers };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base });
  if (name === 'Enter') await send('Input.dispatchKeyEvent', { type: 'char', ...base, text: '\r' });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};
const onDisk = () => { try { return JSON.parse(fs.readFileSync(FILE, 'utf8')).items; } catch { return []; } };
const disk = (test, what, ms) => until(async () => test(onDisk()), what, ms);
const shot = (name) => ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'),
  '-Under', RUN, '-Out', path.join(SHOTS, `ideas-${name}.png`));
const same = (a, b) => (a || '').replace(/[\\/]+$/, '').toLowerCase() === (b || '').replace(/[\\/]+$/, '').toLowerCase();

try {
  console.log('1. the bulb opens the ideas, the caret in the writing line');
  await until(() => run(`!!document.querySelector('.gearrow .ideabtn') && !!(S && S.groups && S.groups.length === 2)`), 'the side column');
  await click('.gearrow .ideabtn');
  await until(() => run(`!document.getElementById('ideas').hidden && IDEAS.known`), 'the ideas and their projects');
  check(await run(`document.activeElement.matches('#ideas .inew .itext')`), 'the caret is in the writing line');
  const chosen = await run(`document.querySelector('#ideas .iproj').value`);
  check(same(chosen, APPDIR), 'the project of the folder in front is chosen: ' + chosen);
  check((await run(`IDEAS.projects.length`)) === 2, 'both project folders are offered');

  console.log('2. writing cards');
  await type('最初のアイデア'); await key('Enter');
  await type('二つ目'); await key('Enter', 8); await type('二行目'); await key('Enter');
  await disk((items) => items.length === 2 && items[1].text === '二つ目\n二行目', 'two cards in the file');
  const items = onDisk();
  check(items[0].text === '最初のアイデア' && same(items[0].project, APPDIR), 'the first card is saved, in its project');
  check(items[1].text === '二つ目\n二行目', 'Shift+Enter kept a new line inside the card');
  check(await run(`document.querySelectorAll('#ideas .ilist .icard').length === 2`), 'both are drawn');
  shot('1-written');

  console.log('3. an edit is saved with no button');
  await click('#ideas .ilist .icard:nth-child(1) .itext');
  await key('End'); await type('（直した）');
  await disk((it) => it[0] && it[0].text === '最初のアイデア（直した）', 'the edit in the file');
  check(true, 'the edit reached the file');

  console.log('4. ticked off');
  await click('#ideas .ilist .icard:nth-child(1) .icheck');
  await disk((it) => it[0] && it[0].done === true, 'done in the file');
  check(await run(`document.querySelectorAll('#ideas .ilist .icard').length === 1`), 'it left the list');
  await click('#ideas .idone');
  check(await run(`document.querySelectorAll('#ideas .ilist .icard.done').length === 1`), 'shown again, marked done, when done ones are shown');
  await click('#ideas .idone');

  console.log('5. carried by its grip');
  await type('三つ目'); await key('Enter');
  await disk((it) => it.length === 3, 'the third card');
  await sleep(300);
  const from = await at('#ideas .ilist .icard:nth-child(2) .igrip');
  const to = await at('#ideas .ilist .icard:nth-child(1) .igrip');
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: from.x, y: from.y, button: 'left', clickCount: 1 });
  for (let k = 1; k <= 8; k++) {
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: from.x, y: from.y + (to.y - 12 - from.y) * k / 8, button: 'left', buttons: 1 });
  }
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: to.x, y: to.y - 12, button: 'left', clickCount: 1 });
  await disk((it) => it.map((i) => i.text).join('|') === '最初のアイデア（直した）|三つ目|二つ目\n二行目', 'the new order in the file');
  check(true, 'the order reached the file, the done card keeping its place');
  shot('2-reordered');

  console.log('6. a card with no project, and one for the other project');
  await run(`(() => { const p = document.querySelector('#ideas .iproj'); p.value = ''; p.dispatchEvent(new Event('change')); })()`);
  await type('どこにも属さないメモ'); await key('Enter');
  const notesKey = await run(`IDEAS.projects.find(p => p.name === 'notes').key`);
  await run(`(() => { const p = document.querySelector('#ideas .iproj'); p.value = ${JSON.stringify(notesKey)}; p.dispatchEvent(new Event('change')); })()`);
  await type('notes のメモ'); await key('Enter');
  await disk((it) => it.some((i) => i.text === 'notes のメモ'), 'the notes card');
  check(onDisk().some((i) => i.text === 'どこにも属さないメモ' && i.project === null), 'the card with no project has none');
  check(same(onDisk().find((i) => i.text === 'notes のメモ').project, NOTES), 'the notes card belongs to notes');

  console.log('7. Esc closes and the settings lose the notes folder');
  await key('Escape');
  check(await run(`document.getElementById('ideas').hidden`), 'Esc closed the ideas');
  fs.writeFileSync(CONFIG, JSON.stringify(settings([appFolder]), null, 2));
  await until(() => run(`S && S.groups && S.groups.length === 1`), 'the settings to be read again', 30000);
  await click('.gearrow .ideabtn');
  await disk((it) => { const n = it.find((i) => i.text === 'notes のメモ'); return n && n.project === null; }, 'the notes card moved to no project');
  check(true, 'the card of the removed project went to no project');
  check((await run(`IDEAS.projects.length`)) === 1, 'the removed project is no longer offered');
  shot('3-project-removed');
  await key('Escape');
} catch (e) {
  check(false, e.message);
} finally {
  ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\ideas-*.png');
process.exit(failures ? 1 : 0);
