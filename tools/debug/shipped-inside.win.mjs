/**
 * The app's own rules for places inside, through the running app's own window.
 *
 * A project whose .gitignore ignores `.claude/` as a whole copies it into a
 * new worktree -- and with it Claude Code's helper worktrees, each a whole
 * checkout. The app brings its own rules for those places; a project keeps
 * only what the person changed about one. This starts the app built in this
 * checkout, in a folder of its own, with such a repository, and checks:
 *
 *   1. the dialog says, under the `.claude` row, what is not brought and how
 *      big, with nothing called added (the project never looked before),
 *      and the app's rules are listed;
 *   2. the worktree made gets `.claude` without its helper worktrees;
 *   3. opened again, nothing is marked new any more (the project was shown);
 *   4. a version that adds a rule -- the record of what was shown taken back
 *      to an older edition -- marks it new again;
 *   5. the rule changed to come along (written the way the settings page
 *      writes a change), the next worktree gets the helpers' worktrees.
 *
 *     cargo build
 *     node tools/debug/shipped-inside.win.mjs
 *
 * Needs Windows, Node and git. Isolated the way a new-user run is: its own
 * folder, its own LOCALAPPDATA, worktrees placed inside that folder. Nothing
 * of a copy somebody is using is read, written or stopped. Photographs land
 * in target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const PORT = 9346;
const RUN = path.join(os.tmpdir(), 'sk-shipped');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'work', 'shop');
const PLACE = path.join(RUN, 'wt');
const CONFIG = path.join(APP, 'config', 'config.json');
const SHOWN = path.join(APP, 'data', 'inside-rules-shown.json');
const HELPER_BYTES = 3 * 1024 * 1024;

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
for (const d of [APP, REPO, PLACE, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const git = (...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd: REPO, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
  return r.stdout;
};
git('init', '-q', '-b', 'main');
fs.writeFileSync(path.join(REPO, '.gitignore'), '.claude/\n');
fs.writeFileSync(path.join(REPO, 'readme.md'), 'a shop\n');
git('add', '-A');
git('commit', '-q', '-m', 'start');
// What Claude Code keeps in a checkout: its settings, a helper's worktree
// with a build folder, and its record of edits
const put = (rel, body) => { const p = path.join(REPO, rel); fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, body); };
put('.claude/settings.json', '{}');
put('.claude/worktrees/agent-1/target/big.bin', Buffer.alloc(HELPER_BYTES, 7));
put('.claude/worktrees/agent-1/src/main.rs', 'fn main() {}');
put('.claude/checkpoints/one', 'x');
// Its record of this PC's state, files and a folder
put('.claude/agent-registry.json', '{}');
put('.claude/scheduled_tasks.json', '[]');
put('.claude/routines/.state/run.json', '{}');
put('.claude/routines/mine.md', 'a routine somebody wrote');

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const writeConfig = (bring) => {
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'ja',
    remote: { enabled: false },
    // Answered here so the question about the AI CLIs' hooks does not cover
    // the board (this checks nothing about them)
    agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
    desks: [{
      name: 'Check', id: 'check',
      projects: [{ name: 'shop', at: REPO, placement: PLACE, bring }],
      folders: [{ cwd: REPO, project: 'shop', tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }],
    }],
  }, null, 2));
};
const LINE = { pattern: '.claude/', how: 'copy' };
writeConfig([LINE]);

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

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
// The page itself, as its own renderer draws it: the window runs behind
// everything else here, so a picture of the screen would show something else
const shot = async (name) => {
  fs.mkdirSync(SHOTS, { recursive: true });
  const { data } = await send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(SHOTS, `shipped-inside-${name}.png`), Buffer.from(data, 'base64'));
};

// The dialog on the repository, the Name tab up and "More" open, answered
// with the sizes in
const open = async () => {
  await run(`openBranch({folder: ${JSON.stringify(REPO)}}); branchTab = "name"; drawBranchTabs(document.getElementById("branch"));
    showMore(document.getElementById("branch"), true); true`);
  await until(() => run(`!!(S.branch && S.branch.from && S.branch.carry_sizes)`), 'the dialog and its sizes', 30000);
  await until(() => run(`!!document.querySelector('#branch .bcarry select[data-name=".claude"]')`), 'the .claude row', 15000);
};
const leftSaid = () => run(`(() => { const s = document.querySelector('#branch .bcarry select[data-name=".claude"]');
  const l = s && s.parentElement.querySelector('.bleft'); return l ? l.textContent : null; })()`);
// Every place the app counted as left out of .claude, by its name
const leftPlaces = async () => JSON.parse(await run(
  `JSON.stringify((((S.branch.carry_sizes || []).find(s => s.path === ".claude") || {}).left || []).map(l => l.path))`));
const make = async (name) => {
  await run(`(() => { const q = document.getElementById("bq"); q.value = ${JSON.stringify(name)}; q.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => run(`!!(S.branch && S.branch.asked === ${JSON.stringify(name)} && !S.branch.error)`), 'the answer about ' + name, 30000);
  const folder = await run(`S.branch.folder`);
  await run(`document.querySelector("#branch .bgo .go").click(); true`);
  await until(async () => fs.existsSync(path.join(folder, 'readme.md')) && fs.existsSync(path.join(folder, '.claude', 'settings.json')),
    'the worktree with what it brings: ' + folder, 90000);
  await sleep(1500);
  return folder;
};

try {
  await until(() => run(`!!(S && S.groups && S.groups.length)`), 'the board', 30000);

  console.log('1. the dialog says what the app\'s rules leave out of .claude, as new');
  await open();
  const listed = JSON.parse(await run(`JSON.stringify(S.branch.shipped_rules)`));
  check(listed.length === 8 && listed[0] === '**/.claude/worktrees/' && listed.includes('**/.claude/scheduled_tasks.json'),
    'the app\'s rules are listed: ' + JSON.stringify(listed));
  check((await run(`S.branch.shipped_new.length`)) === 0, 'none of them is said to be added to a project that never looked');
  await until(async () => /\.claude\/worktrees/.test((await leftSaid()) || ''), 'the line under .claude', 15000).catch(async (e) => {
    console.log('--- said: ' + JSON.stringify(await leftSaid()) + '\n--- sizes: ' +
      await run(`JSON.stringify((S.branch.carry_sizes || []).map(s => ({path: s.path, left: s.left})))`));
    throw e;
  });
  const first = await leftSaid();
  check(/^持っていかない場所: \.claude\/worktrees,/.test(first), 'it names the largest place first: ' + first);
  const leftNow = await leftPlaces();
  check(leftNow.length === 5 && leftNow.includes('.claude/checkpoints') && leftNow.includes('.claude/scheduled_tasks.json'),
    'every place left out is counted: ' + JSON.stringify(leftNow));
  check(/MB/.test(first), 'it says how big: ' + first);
  check(!/前回の確認より後に追加/.test(first), 'it does not call the rules added the first time: ' + first);
  await run(`document.querySelector('#branch .bcarry select[data-name=".claude"]').parentElement.scrollIntoView({block:'center'}); true`);
  await sleep(300);
  await shot('1-dialog');

  console.log('2. the worktree gets .claude without its helpers\' worktrees');
  const one = await make('first-cut');
  check(fs.existsSync(path.join(one, '.claude', 'settings.json')), '.claude came along');
  check(!fs.existsSync(path.join(one, '.claude', 'worktrees')), 'the helpers\' worktrees did not');
  check(!fs.existsSync(path.join(one, '.claude', 'checkpoints')), 'nor the record of edits');
  const shown = JSON.parse(fs.readFileSync(SHOWN, 'utf8'));
  const shopUid = JSON.parse(fs.readFileSync(path.join(APP, 'config', 'config.json'), 'utf8')).desks[0].projects[0].uid;
  check(/^[0-9a-f-]{36}$/.test(shopUid) && shown[shopUid] === 2 && Object.keys(shown).length === 1,
    'the project counts as shown the app\'s rules, by who it is: ' + JSON.stringify(shown));
  for (const f of ['agent-registry.json', 'scheduled_tasks.json', 'routines/.state']) {
    check(!fs.existsSync(path.join(one, '.claude', f)), 'nor ' + f);
  }
  check(fs.existsSync(path.join(one, '.claude', 'routines', 'mine.md')), 'a routine somebody wrote still came');

  console.log('3. opened again, nothing is new any more');
  await run(`closeBranch(); true`);
  await sleep(500);
  await open();
  await until(async () => /\.claude\/worktrees/.test((await leftSaid()) || ''), 'the line under .claude', 15000);
  check(!/前回の確認より後に追加/.test(await leftSaid()), 'not marked new once shown: ' + await leftSaid());

  console.log('4. a version that adds a rule marks it new again');
  await run(`closeBranch(); true`);
  // Shown the first edition only: the second batch is what is new
  fs.writeFileSync(SHOWN, JSON.stringify({ [shopUid]: 1 }));
  await sleep(300);
  await open();
  const fresh = JSON.parse(await run(`JSON.stringify(S.branch.shipped_new)`));
  check(fresh.length === 5 && !fresh.includes('**/.claude/worktrees/') && fresh.includes('**/.claude/agent-registry.json'),
    'only the second batch is new to a project shown the first: ' + JSON.stringify(fresh));
  await until(async () => /前回の確認より後に追加/.test((await leftSaid()) || ''), 'the mark on a rule newer than what was shown', 15000);
  check(true, 'marked new against an older edition: ' + await leftSaid());
  await run(`closeBranch(); true`);

  console.log('5. changed to come along, the helpers\' worktrees come');
  writeConfig([LINE, { default: 'claude-helper-worktrees', path: '**/.claude/worktrees/', how: 'copy' }]);
  await sleep(2500);
  await open();
  await until(async () => !/\.claude\/worktrees/.test((await leftSaid()) || ''), 'the line no longer naming the helpers', 15000);
  const now = await leftSaid();
  const leftThen = await leftPlaces();
  check(leftThen.includes('.claude/checkpoints') && !leftThen.includes('.claude/worktrees'),
    'the rules not changed still leave their places out: ' + JSON.stringify(leftThen) + ' / ' + now);
  const two = await make('second-cut');
  check(fs.statSync(path.join(two, '.claude', 'worktrees', 'agent-1', 'target', 'big.bin')).size === HELPER_BYTES, 'the helpers\' worktrees came along');
  check(!fs.existsSync(path.join(two, '.claude', 'checkpoints')), 'the other rule still holds');
  const kept = JSON.parse(fs.readFileSync(CONFIG, 'utf8')).desks[0].projects[0].bring;
  check(kept.length === 2 && !kept.some((r) => r.path === '**/.claude/checkpoints/'), 'the settings hold only the change, not the app\'s list: ' + JSON.stringify(kept));
  await shot('5-changed');
} catch (e) {
  check(false, e.message);
} finally {
  ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\shipped-inside-*.png');
process.exit(failures ? 1 : 0);
