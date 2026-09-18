/**
 * What a typed name becomes, from the box down to git and the disk.
 *
 * A branch and a folder are held to the letters every machine can hold, so a
 * name typed in Japanese cannot be either. The dialog keeps what it can of
 * what was typed, draws a name when there is too little left, and says which
 * under the box while it is typed. What was typed stays as the folder's name
 * in the list, which is this app's own and never reaches git.
 *
 * So this starts the app built in this checkout, in a folder of its own, with
 * a repository to cut branches from, and checks the whole chain: what the box
 * says while typing, and -- after the button -- the branch git really made,
 * the folder really on the disk, and the name really in the settings.
 *
 *     cargo build
 *     node tools/debug/worktree-naming.win.mjs
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
const PORT = 9346;
const RUN = path.join(os.tmpdir(), 'sk-naming');
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
  return r.stdout;
};
git('init', '-q', '-b', 'main');
fs.writeFileSync(path.join(REPO, 'readme.md'), 'a shop\n');
git('add', '-A');
git('commit', '-q', '-m', 'start');

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
const shot = (name) => ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'),
  '-Under', RUN, '-Out', path.join(SHOTS, `worktree-naming-${name}.png`));

// The dialog, opened on the repository with the Name tab up -- the one tab
// with a box to type a name into
const open = async () => {
  await run(`openBranch({folder: ${JSON.stringify(REPO)}}); branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await until(() => run(`!!(S.branch && S.branch.from)`), 'the dialog to be answered', 30000);
};
// Typed the way a person types: the box changed and the page told, then the
// app's answer for that name waited for
const type = async (name) => {
  await run(`(() => { const q = document.getElementById("bq"); q.value = ${JSON.stringify(name)};
    q.dispatchEvent(new Event("input")); return q.value; })()`);
  await until(() => run(`!!(S.branch && S.branch.asked === ${JSON.stringify(name)} && !S.branch.error)`),
    'the answer about ' + JSON.stringify(name), 30000);
  return run(`({said: document.querySelector("#branch .bname").textContent,
    box: document.getElementById("bq").value,
    branch: S.branch.branch, folder: S.branch.folder})`);
};
// What the settings say a folder is called, once it is written down
const labelOf = (folder) => {
  const cfg = JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
  const found = (cfg.desks[0].folders || []).find((f) => String(f.cwd).replace(/\\/g, '/') === folder.replace(/\\/g, '/'));
  return found ? found.name : null;
};

try {
  await until(() => run(`!!(S && S.groups && S.groups.length)`), 'the board', 30000);

  console.log('1. a name already in the letters both can hold is left alone');
  await open();
  const kept = await type('fix/crash-on-open');
  check(kept.branch === 'fix/crash-on-open', 'the branch is the name as typed: ' + kept.branch);
  check(kept.said === '', 'nothing is said, because there is nothing to say: ' + JSON.stringify(kept.said));

  console.log('2. a name partly in letters both can hold keeps that part');
  const cut = await type('ログイン画面 login');
  check(cut.branch === 'login', 'what is left is the branch: ' + cut.branch);
  check(cut.box === 'ログイン画面 login', 'the box keeps what was typed: ' + cut.box);
  check(cut.said.includes('login'), 'the line under the box names it: ' + cut.said);

  console.log('3. a name with nothing either can hold draws one');
  const drawn = await type('ログイン画面');
  check(/^[a-z]+-[a-z]+$/.test(drawn.branch), 'a drawn name of two words: ' + drawn.branch);
  check(drawn.said.includes(drawn.branch), 'the line under the box names it: ' + drawn.said);
  check(drawn.box === 'ログイン画面', 'the box keeps what was typed: ' + drawn.box);
  const again = await type('ログイン画面!');
  check(again.branch === drawn.branch, 'the drawn name does not change with the next keystroke: ' + again.branch);
  shot('drawn');

  console.log('4. pressed, git and the disk get that name and the list keeps what was typed');
  await run(`document.querySelector("#branch .bgo .go").click(); true`);
  await until(() => run(`!document.getElementById("branch") || document.getElementById("branch").hidden`), 'the dialog to close', 60000);
  const folder = again.folder;
  await until(async () => fs.existsSync(folder) && !!labelOf(folder), 'the folder to be made and written down', 60000);
  check(/^[\x20-\x7e]+$/.test(folder), 'the folder on the disk is in letters every machine can hold: ' + folder);
  check(path.basename(folder) === drawn.branch, 'the folder is named for the branch: ' + path.basename(folder));
  const made = spawnSync('git', ['-C', folder, 'branch', '--show-current'], { encoding: 'utf8' }).stdout.trim();
  check(made === drawn.branch, 'git is on the drawn branch: ' + JSON.stringify(made));
  const heads = git('for-each-ref', '--format=%(refname)', 'refs/heads');
  check(!/[^\x00-\x7f]/.test(heads), 'no branch in the repository is outside those letters:\n' + heads.trim());
  check(labelOf(folder) === 'ログイン画面!', 'the list keeps what was typed: ' + JSON.stringify(labelOf(folder)));
  await sleep(1200);
  shot('made');
} catch (e) {
  console.error(e.message);
  failures += 1;
} finally {
  stopApp();
  await sleep(500);
  // A worktree goes where this app puts every worktree, which is the person's
  // own folder and not this run's. Made here, so taken away here -- a check
  // that leaves folders behind has changed the machine it was checking
  fs.rmSync(path.join(os.homedir(), 'SHIKISHA-TERM', 'branches', path.basename(REPO)),
    { recursive: true, force: true });
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);
