/**
 * A folder with thousands of changed files, in the running app's Git column.
 *
 * A folder whose build output git does not ignore lists every file of it as
 * new. The column is drawn again on every state the app sends, and it used to
 * make a row for each of those files every time: the whole window stuttered
 * for as long as the column was open on such a folder. This opens the column
 * on 6000 new files in a real copy and checks, in the real window:
 *
 *   1. the list makes its first 200 rows, says how many there are in all, and
 *      offers the next ones under them
 *   2. states arriving from the app leave the list as it is (the same rows,
 *      not made again), and the window keeps up with them
 *   3. the button under the list shows the next 200
 *   4. "Stage all" stages every one of them -- more paths than one command
 *      line holds -- and its button says how far it has got while it goes
 *
 *     cargo build
 *     node tools/debug/git-many-files.win.mjs
 *
 * Needs Windows, git and Node. The copy it drives is started by
 * `instance.win.ps1`, so it has its own folder, settings, state and ports, and
 * nothing of a copy somebody is using is read, written or stopped. The copy is
 * stopped on the way out.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-many-files');
// Beside the copy's folder, not inside it: the copy clears its own on the way up
const WORK = RUN + '-repo';
const INSTANCE = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');
const MANY = 6000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => {
  console.log((ok ? '  PASS ' : '  FAIL ') + what);
  if (!ok) failures += 1;
};

const ps = (args) => spawnSync('powershell.exe',
  ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', INSTANCE, ...args],
  { encoding: 'utf8' });
const git = (...args) => {
  const r = spawnSync('git', args, { cwd: WORK, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ': ' + (r.stderr || r.stdout));
  return r.stdout;
};

// ── A folder whose build output is not ignored ────────────────────────────
fs.rmSync(RUN, { recursive: true, force: true });
fs.rmSync(WORK, { recursive: true, force: true });
fs.mkdirSync(path.join(WORK, 'out'), { recursive: true });
git('init', '-q', '-b', 'main');
git('config', 'user.email', 'test@example.invalid');
git('config', 'user.name', 'test');
fs.writeFileSync(path.join(WORK, 'kept.txt'), 'one\n');
git('add', '-A');
git('commit', '-qm', 'start');
for (let i = 0; i < MANY; i++) fs.writeFileSync(path.join(WORK, 'out', 'f' + i + '.o'), '');

// ── The copy, and its window ──────────────────────────────────────────────
const started = ps(['-At', RUN, '-Work', WORK]);
if (started.status !== 0) die(started.stdout + started.stderr);
const said = Object.fromEntries((started.stdout || '').split('\n')
  .map((l) => l.trim().split('=')).filter((p) => p.length > 1)
  .map(([k, ...v]) => [k, v.join('=')]));
const cdp = Number((said.cdp || '').split(':').pop());
if (!cdp) die('the copy did not say where its DevTools port is:\n' + started.stdout);
const stop = () => ps(['-At', RUN, '-Stop']);

let ws;
try {
  let targets = [];
  for (let i = 0; i < 80 && !targets.length; i++) {
    try {
      targets = (await (await fetch(`http://127.0.0.1:${cdp}/json/list`)).json())
        .filter((t) => t.type === 'page');
    } catch { await sleep(250); }
  }
  if (!targets.length) throw new Error('the window never answered on its DevTools port');
  const connection = await connectCdp(targets[0]);
  ws = connection.ws;
  const {run} = connection;

  // Every state the app hands the window is counted and kept, so the last one
  // can be handed over again the way the app hands it
  await run(`(() => {
    const take = window.__state;
    window.__seen = {n: 0, last: null};
    window.__state = function (json) { window.__seen.n += 1; window.__seen.last = json; return take.apply(this, arguments); };
  })()`);

  // 1. The column, open on the changes, once the folder has been read
  const first = await run(`(async () => {
    setSideWidth(440); sideReveal("git");
    await new Promise(r => setTimeout(r, 400));
    gitRefresh(true);
    for (let i = 0; i < 100 && !(G.rows && G.rows.length); i++) await new Promise(r => setTimeout(r, 100));
    await new Promise(r => setTimeout(r, 300));
    const list = gitUi && gitUi.work;
    return {rows: (G.rows || []).length, made: list ? list.querySelectorAll(".row").length : -1,
      count: gitUi ? gitUi.workN.textContent : "", offer: list && list.querySelector(".growsmore")
        ? list.querySelector(".growsmore").textContent : ""};
  })()`);
  console.log('  ' + JSON.stringify(first));
  check(first.rows === MANY, `the app reads all ${MANY} new files`);
  check(first.made === 200, 'the list makes its first 200 rows, not all of them');
  check(first.count === String(MANY), 'the list says how many there are in all');
  check(/5[,.]?800/.test(first.offer), 'the button under the list says how many are not shown');

  // 2. States from the app: the same rows stay, and the window keeps up
  const steady = await run(`(async () => {
    const list = gitUi.work, row = list.querySelector(".row");
    const seen = window.__seen.n;
    await new Promise(r => setTimeout(r, 3000));
    const arrived = window.__seen.n - seen;
    // The app's own last state, handed over as the app hands it, thirty times
    const t0 = performance.now();
    for (let i = 0; i < 30; i++) window.__state(window.__seen.last);
    const each = (performance.now() - t0) / 30;
    return {arrived, kept: list.querySelector(".row") === row && list.isConnected, each,
      made: list.querySelectorAll(".row").length};
  })()`);
  console.log('  ' + JSON.stringify(steady));
  check(steady.kept, 'states arriving leave the list as it is: its rows are not made again');
  check(steady.made === 200, 'still 200 rows after the states');
  check(steady.each < 40, `a state is taken in ${Math.round(steady.each)} ms with the column open (under 40)`);

  // 3. The next ones, a press away
  const more = await run(`(async () => {
    gitUi.work.querySelector(".growsmore").click();
    await new Promise(r => setTimeout(r, 200));
    return gitUi.work.querySelectorAll(".row").length;
  })()`);
  check(more === 400, 'the button shows the next 200 (' + more + ')');

  // 4. "Stage all": more paths than one command line holds. The main button
  //    says how far it has got while it goes, and every file is staged
  const staged = await run(`(async () => {
    const labels = new Set();
    const watch = new MutationObserver(() => labels.add(gitUi.main.textContent));
    watch.observe(gitUi.main, {childList: true, subtree: true, characterData: true});
    gitAsk("stage", {paths: gitAllPaths(false)});
    const t0 = performance.now();
    while (performance.now() - t0 < 60000) {
      await new Promise(r => setTimeout(r, 100));
      if (!G.busy && (G.rows || []).length && G.rows.every(r => r.staged)) break;
    }
    watch.disconnect();
    return {staged: (G.rows || []).filter(r => r.staged).length, said: G.said, bad: !!G.bad,
      labels: [...labels], took: Math.round(performance.now() - t0)};
  })()`);
  console.log('  ' + JSON.stringify({...staged, labels: staged.labels.slice(0, 6)}));
  check(staged.staged === MANY, `"Stage all" stages all ${MANY} files (${staged.staged})`);
  check(!staged.bad, 'and says nothing went wrong' + (staged.bad ? ': ' + staged.said : ''));
  check(staged.labels.some(l => /\d[\d,.]* \/ 6[,.]?000/.test(l)), 'the button says how many of the files are done while it goes');
} catch (e) {
  console.error(e.message || e);
  failures += 1;
} finally {
  if (ws) ws.close();
  stop();
  fs.rmSync(WORK, { recursive: true, force: true });
}

console.log(failures ? failures + ' failed' : 'all good');
process.exit(failures ? 1 : 0);
