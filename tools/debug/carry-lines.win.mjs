/**
 * Choosing what a new worktree brings along, a .gitignore line at a time.
 *
 * The worktree dialog lists everything git ignores that is there, one row
 * each. A line such as `www/tmp/*` can match a hundred of them, so the list
 * has a second tab that shows each line once. What is chosen there reaches
 * the list only when it is applied, and then becomes the project's own.
 *
 * So this starts the app built in this checkout, in a folder of its own, with
 * a repository whose .gitignore matches many folders under one line, opens the
 * dialog through the page, and checks: a row changed by hand is kept, every
 * row of the line chosen by line changes, the choice is in the settings file,
 * and the dialog opened again starts from it.
 *
 *     cargo build
 *     node tools/debug/carry-lines.win.mjs
 *
 * Needs Windows, Node and git. Isolated the way a new-user run is: its own
 * folder, its own LOCALAPPDATA, started from its own folder. Nothing of a copy
 * somebody is using is read, written or stopped. Photographs land in
 * target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const PORT = 9344;
const RUN = path.join(os.tmpdir(), 'sk-carry');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'work', 'shop');
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
for (const d of [APP, REPO, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const git = (...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd: REPO, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
};
git('init', '-q');
fs.writeFileSync(path.join(REPO, '.gitignore'), 'www/tmp/*\ndata/config.php\nThumbs.db\n');
git('add', '.gitignore');
git('commit', '-q', '-m', 'start');
// One line matching many folders -- the folder holding them is not ignored and
// is not offered -- and two lines matching one file each
const TMP = 12;
for (let i = 0; i < TMP; i++) fs.mkdirSync(path.join(REPO, 'www', 'tmp', `t${String(i).padStart(2, '0')}`), { recursive: true });
fs.mkdirSync(path.join(REPO, 'data'), { recursive: true });
fs.writeFileSync(path.join(REPO, 'data', 'config.php'), '<?php');
fs.writeFileSync(path.join(REPO, 'Thumbs.db'), 'x');

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: false },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: REPO, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
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
const click = async (sel) => {
  const p = await run(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null;
    e.scrollIntoView({block:'center'}); const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  if (!p) throw new Error('nothing to click: ' + sel);
  for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
    await send('Input.dispatchMouseEvent', { type, x: p.x, y: p.y, button: 'left', clickCount: 1 });
  }
  await sleep(250);
};
const shot = (name) => ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'),
  '-Under', RUN, '-Out', path.join(SHOTS, `carry-lines-${name}.png`));
// The rows of the list, as { name: how }
const rows = () => run(`Object.fromEntries(Array.from(document.querySelectorAll('#branch .bcarry select')).map(s => [s.dataset.name, s.value]))`);
const choose = (sel, value) => run(`(() => { const s = document.querySelector(${JSON.stringify(sel)}); s.value = ${JSON.stringify(value)};
  s.dispatchEvent(new Event('change')); return s.value; })()`);
const rule = (pattern) => {
  const cfg = JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
  return ((cfg.desks[0].projects || [])[0]?.bring || []).find((r) => r.pattern === pattern);
};
const open = async () => {
  await run(`openBranch({folder: ${JSON.stringify(REPO)}}); showMore(document.getElementById('branch'), true); true`);
  await until(async () => Object.keys(await rows()).length === TMP + 2, 'the list of what comes along', 30000)
    .catch(async (e) => {
      console.log('--- rows ---\n' + JSON.stringify(await rows()) + '\n--- the app\'s answer ---\n' +
        JSON.stringify(await run(`S.branch && {from: S.branch.from, carry: (S.branch.carry || []).length, error: S.branch.error}`)));
      throw e;
    });
};

try {
  await until(() => run(`!!(S && S.groups && S.groups.length)`), 'the board', 30000);

  console.log('1. the dialog lists every row, and offers the lines');
  await open();
  const first = await rows();
  check(!('www/tmp' in first), 'the folder holding what the line ignores is not offered');
  check(first['www/tmp/t00'] === 'link' && first['data/config.php'] === 'copy', 'rows start from the project\'s answer: ' + JSON.stringify(first));
  check(await run(`!document.querySelector('#branch .bctabs').hidden`), 'the two tabs are shown');
  check(await run(`document.querySelector('#branch .bctabs button.on').dataset.ctab === 'each'`), 'the list is the tab in front');
  shot('1-list');

  console.log('2. one row changed by hand, then a line chosen together');
  await choose('#branch .bcarry select[data-name="data/config.php"]', 'skip');
  await click('#branch .bctabs button[data-ctab="lines"]');
  check(await run(`!document.querySelector('#branch .bclines').hidden && document.querySelector('#branch .bcarry').hidden`), 'the lines replace the list');
  const lines = await run(`Array.from(document.querySelectorAll('#branch .bclines > div')).map(d => d.querySelector('.nm').textContent + '|' + d.querySelector('.n').textContent)`);
  check(lines.includes(`www/tmp/*|${TMP} 件`), 'the line and how many it matches: ' + JSON.stringify(lines));
  await run(`(() => { const d = Array.from(document.querySelectorAll('#branch .bclines > div')).find(d => d.querySelector('.nm').textContent === 'www/tmp/*');
    const s = d.querySelector('select'); s.value = 'skip'; s.dispatchEvent(new Event('change')); return true; })()`);
  check((await rows())['www/tmp/t00'] === 'link', 'choosing by line does not touch the list yet');
  shot('2-lines');

  console.log('3. back on the list, applied');
  await click('#branch .bctabs button[data-ctab="each"]');
  check(await run(`!document.querySelector('#branch .bcapply').hidden`), 'the list says there is something to apply');
  const say = await run(`document.querySelector('#branch .bcapply .say').textContent + ' / ' + document.querySelector('#branch .bcapply .bcgo').textContent`);
  check(/1 行分/.test(say) && /一覧に反映/.test(say), 'it says how many lines and what the button does: ' + say);
  shot('3-pending');
  await click('#branch .bcapply .bcgo');
  const after = await rows();
  const tmpRows = Object.entries(after).filter(([n]) => n === 'www/tmp' || n.startsWith('www/tmp/'));
  check(tmpRows.length === TMP && tmpRows.every(([, h]) => h === 'skip'), 'every row of the line is left out');
  check(after['data/config.php'] === 'skip', 'the row changed by hand kept its choice');
  check(after['Thumbs.db'] === 'copy', 'a line nobody chose stays as it was');
  check(await run(`document.querySelector('#branch .bcapply').hidden`), 'nothing is left to apply');
  await until(async () => rule('www/tmp/*')?.how === 'skip', 'the choice in the settings file');
  check(true, 'the choice is the project\'s: ' + JSON.stringify(rule('www/tmp/*')));
  check(!rule('data/config.php'), 'the row changed by hand did not become the project\'s');
  await sleep(1500);
  check(Object.entries(await rows()).filter(([n]) => n === 'www/tmp' || n.startsWith('www/tmp/')).every(([, h]) => h === 'skip'), 'the reload after saving did not put the rows back');
  shot('4-applied');

  console.log('4. opened again, it starts from the project\'s choice');
  await run(`closeBranch(); true`);
  await sleep(500);
  await open();
  await until(async () => (await rows())['www/tmp/t05'] === 'skip', 'the saved choice on a new dialog', 15000);
  check((await rows())['data/config.php'] === 'copy', 'a row changed by hand in the dialog before is not carried into this one');
  await click('#branch .bctabs button[data-ctab="lines"]');
  const saidLine = await run(`(() => { const d = Array.from(document.querySelectorAll('#branch .bclines > div')).find(d => d.querySelector('.nm').textContent === 'www/tmp/*'); return d.querySelector('select').value; })()`);
  check(saidLine === 'skip', 'the line says the project\'s choice');
  check(await run(`document.querySelector('#branch .bcapply').hidden`), 'nothing to apply on a fresh dialog');
} catch (e) {
  check(false, e.message);
} finally {
  ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\carry-lines-*.png');
process.exit(failures ? 1 : 0);
