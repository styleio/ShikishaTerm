/**
 * Throwing a change away, checked through the running app's own window, and
 * against the files on the disk afterwards.
 *
 * This is the one thing the changes column does that git cannot undo: what it
 * throws away was never committed, so there is no object left to find it in
 * again. Everything under the screen has tests, but the part that decides
 * which file disappears runs across the join between the window, the loop and
 * git -- and no test stands there. So this presses the real entry in the real
 * menu, reads the real question, presses the real button, and then looks at the
 * folder.
 *
 * Four rows, because four rows mean four different things: a file changed since
 * the last commit, one git has never seen, one already added to the next commit,
 * and one added and never committed. It also checks the two that must not
 * happen -- a file the project ignores is left alone, and a file in conflict is
 * not offered the entry at all.
 *
 *     cargo build
 *     node tools/debug/git-discard.win.mjs
 *
 * Needs Windows, git and Node. The copy it drives is started by
 * `instance.win.ps1`, so it has its own folder, settings, state and ports, and
 * nothing of a copy somebody is using is read, written or stopped. The copy is
 * stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-drop');
// Beside the copy's folder, not inside it: the copy clears its own on the way up
const WORK = RUN + '-repo';
const INSTANCE = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');

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
const here = (name) => fs.existsSync(path.join(WORK, name));
const text = (name) => fs.readFileSync(path.join(WORK, name), 'utf8').trim();

// ── A folder with one of every kind of change in it ───────────────────────
fs.rmSync(RUN, { recursive: true, force: true });
fs.rmSync(WORK, { recursive: true, force: true });
fs.mkdirSync(WORK, { recursive: true });
git('init', '-q', '-b', 'main');
git('config', 'user.email', 'test@example.invalid');
git('config', 'user.name', 'test');
fs.writeFileSync(path.join(WORK, '.gitignore'), 'built/\n');
fs.writeFileSync(path.join(WORK, 'kept.txt'), 'one\n');
fs.writeFileSync(path.join(WORK, 'staged.txt'), 'one\n');
git('add', '-A');
git('commit', '-qm', 'start');
fs.writeFileSync(path.join(WORK, 'kept.txt'), 'two\n');          // changed, not staged
fs.writeFileSync(path.join(WORK, 'fresh.txt'), 'mine\n');        // git has never seen it
fs.writeFileSync(path.join(WORK, 'staged.txt'), 'two\n');
fs.writeFileSync(path.join(WORK, 'added.txt'), 'new\n');
git('add', 'staged.txt', 'added.txt');                           // in the next commit
fs.mkdirSync(path.join(WORK, 'built'), { recursive: true });
fs.writeFileSync(path.join(WORK, 'built', 'out.bin'), 'made\n'); // ignored, not a change

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
  ws = new WebSocket(targets[0].webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error
      ? rej(new Error(method + ': ' + JSON.stringify(m.error)))
      : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await send('Runtime.evaluate',
      { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) {
      throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    }
    return r.result.value;
  };

  // The column, open on the changes and asked to read the folder again
  const open = `(async () => {
    sidePanel = "git"; setSideWidth(440);
    await new Promise(r => setTimeout(r, 400));
    gitRefresh(true);
    await new Promise(r => setTimeout(r, 1200));
    return [...document.querySelectorAll("#gitpanel .list .row .p")].map(e => e.textContent);
  })()`;

  // One row: right-click it, read the question, press the button. `press`
  // false stops at the question, for the rows that must not be offered it
  const press = (name, go) => `(async () => {
    const wait = ms => new Promise(r => setTimeout(r, ms));
    const row = [...document.querySelectorAll("#gitpanel .list .row")]
      .find(r => r.querySelector(".p") && r.querySelector(".p").textContent === ${JSON.stringify(name)});
    if (!row) return {error: "no row for " + ${JSON.stringify(name)}};
    const box = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent("contextmenu", {bubbles:true, cancelable:true,
      clientX: Math.round(box.left + 60), clientY: Math.round(box.top + 8)}));
    await wait(200);
    const entry = document.querySelector(".fmenu div");
    if (!entry) return {menu: null};
    entry.click();
    await wait(1500);
    const ask = document.getElementById("sask");
    const out = {menu: entry.textContent, asked: !ask.hidden,
      say: ask.querySelector(".vsay").textContent,
      runs: [...ask.querySelectorAll(".bwhere .line.run")].map(e => e.textContent)};
    if (!${go ? 'true' : 'false'} || ask.hidden) { closeAsk(); return out; }
    ask.querySelector(".go").click();
    await wait(2000);
    out.after = G.said || "";
    return out;
  })()`;

  const listed = await run(open);
  console.log('the column lists: ' + listed.join(', '));
  check(!listed.includes('built/out.bin'), 'what the project ignores is not a change');

  // A file changed since the last commit: the working side goes, and only it
  let r = await run(press('kept.txt', true));
  console.log('  ' + JSON.stringify(r));
  check(r.runs && r.runs.length === 1 && /^git (restore|checkout) -- :\(literal\)kept\.txt$/.test(r.runs[0]),
    'the command shown for a changed file is the one that puts it back');
  check(here('kept.txt') && text('kept.txt') === 'one', 'the changed file is back to the last commit');

  // One git has never seen: there is nothing to put back, so it goes
  await run(open);
  r = await run(press('fresh.txt', true));
  console.log('  ' + JSON.stringify(r));
  check(!!r.runs && r.runs.join(' ').includes('clean --force -d --quiet'), 'a new file is taken off the disk');
  check(!!r.runs && !r.runs.join(' ').includes(' -x'), 'the command leaves what the project ignores out of its reach');
  check(!here('fresh.txt'), 'the new file is off the disk');
  check(here('built/out.bin'), 'the ignored file is untouched');

  // Added to the next commit and never committed: out of the commit, off the disk
  await run(open);
  r = await run(press('added.txt', true));
  console.log('  ' + JSON.stringify(r));
  check(!here('added.txt'), 'the file added to the next commit is off the disk');

  // Already in the next commit: back to the last commit, both sides
  await run(open);
  r = await run(press('staged.txt', true));
  console.log('  ' + JSON.stringify(r));
  check(text('staged.txt') === 'one', 'the staged file is back to the last commit');
  check(git('status', '--porcelain').trim() === '', 'git lists nothing as changed any more');

  // A file git has marked as unmerged: the entry is not offered at all
  git('checkout', '-q', '-b', 'other');
  fs.writeFileSync(path.join(WORK, 'kept.txt'), 'theirs\n');
  git('commit', '-qam', 'theirs');
  git('checkout', '-q', 'main');
  fs.writeFileSync(path.join(WORK, 'kept.txt'), 'ours\n');
  git('commit', '-qam', 'ours');
  spawnSync('git', ['merge', 'other'], { cwd: WORK });
  await run(open);
  r = await run(press('kept.txt', false));
  console.log('  ' + JSON.stringify(r));
  check(r.menu === null, 'a file in conflict is offered no entry');
  check(text('kept.txt').includes('<<<<<<<'), 'the conflict is untouched');
} catch (e) {
  console.error(e.message || e);
  failures += 1;
} finally {
  if (ws) ws.close();
  stop();
}

console.log(failures ? failures + ' failed' : 'all good');
process.exit(failures ? 1 : 0);
