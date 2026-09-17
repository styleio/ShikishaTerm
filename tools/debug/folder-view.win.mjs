/**
 * Pressing a folder's name shows that folder, and never half of it beside another.
 *
 * What went wrong once: the screen was split on one folder -- a terminal on the
 * left, a git diff on the right, focus on the right -- and pressing the name of
 * a folder of another project that had not been looked at yet put that folder
 * into the right pane only. The left pane went on showing the first folder.
 *
 * So this starts the app built in this checkout, in a folder of its own, with a
 * desk of two folders, splits the screen on the first, presses the second, and
 * checks that the second is shown on its own; then presses the first again and
 * checks that its split comes back as it was left.
 *
 *     cargo build
 *     node tools/debug/folder-view.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder,
 * its own LOCALAPPDATA, started from its own folder. Nothing of a copy somebody
 * is using is read, written or stopped. Photographs land in target/shots; the
 * app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const PORT = 9345;
const RUN = path.join(os.tmpdir(), 'sk-fview');
const APP = path.join(RUN, 'app');
const FIRST = path.join(RUN, 'work', 'first');
const SECOND = path.join(RUN, 'work', 'second');
const CONFIG = path.join(APP, 'config', 'config.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
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
for (const d of [APP, FIRST, SECOND, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: false },
  desks: [{ name: 'Check', id: 'check', folders: [
    { cwd: FIRST, tabs: [
      { name: 'first-a', id: 'first-a', command: 'cmd.exe' },
      { name: 'first-b', id: 'first-b', command: 'cmd.exe' },
    ] },
    { cwd: SECOND, tabs: [{ name: 'second-a', id: 'second-a', command: 'cmd.exe' }] },
  ] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

let targets;
for (let i = 0; i < 120 && !targets; i++) {
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
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};
const say = (o) => run(`send(${JSON.stringify(o)}); true`);
const shot = (name) => ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'),
  '-Under', RUN, '-Out', path.join(SHOTS, `folder-view-${name}.png`));
// What is on screen, pane by pane, by the tabs' names
const screen = () => run(`(() => {
  const name = s => ((S.tabs || []).find(t => t.index === s) || {}).name || String(s);
  return PANES && PANES.panes && PANES.panes.length ? PANES.panes.map(p => name(p.surface)) : [name(S.active)];
})()`);

try {
  await until(() => run(`!!(S && S.tabs && S.tabs.length === 3)`), 'the board', 30000);

  console.log('1. the first folder, split');
  await say({ kind: 'folderview', folder: FIRST });
  await until(async () => (await screen())[0] === 'first-a', 'the first folder in front');
  await run(`send({kind:"splitpane", id:(PANES && PANES.panes && PANES.panes[0] ? PANES.panes[0].id : 1), down:false}); true`);
  await until(async () => (await screen()).length === 2, 'the split');
  const split = await screen();
  check(split.length === 2, 'split on the first folder: ' + JSON.stringify(split));
  await sleep(800);
  shot('1-split');

  console.log('2. the second folder, never looked at yet, is pressed');
  await say({ kind: 'folderview', folder: SECOND });
  await until(async () => (await screen()).includes('second-a'), 'the second folder on screen');
  await sleep(800);
  const second = await screen();
  check(second.length === 1 && second[0] === 'second-a', 'the second folder is shown on its own: ' + JSON.stringify(second));
  shot('2-second');

  console.log('3. the first folder again: its split comes back');
  await say({ kind: 'folderview', folder: FIRST });
  await until(async () => (await screen()).includes('first-a'), 'the first folder back');
  await sleep(800);
  const back = await screen();
  check(JSON.stringify(back) === JSON.stringify(split), 'the split is as it was left: ' + JSON.stringify(back));
  shot('3-back');
} catch (e) {
  check(false, e.message);
} finally {
  ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\folder-view-*.png');
process.exit(failures ? 1 : 0);
