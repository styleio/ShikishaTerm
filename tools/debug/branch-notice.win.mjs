/**
 * A real AI CLI is told, once, that the branch under it was renamed.
 *
 * This checkout's build in a folder of its own, with one real AI tab in a
 * worktree on a drawn branch (`mighty-gannet`).
 *
 *     cargo build
 *     node tools/debug/branch-notice.win.mjs [--cli=claude|codex]
 *
 * Claude Code (the default): the tab's hooks come from a settings file of this
 * run's own, handed to Claude with `--settings` -- the person's own
 * `~/.claude/settings.json` is neither read for this nor written, and the copy
 * is told every CLI's hook was answered "off", so it asks nothing and writes
 * nothing either.
 *
 * Codex: the copy runs in a home of this run's own (USERPROFILE, HOME,
 * CODEX_HOME) with the sign-in copied into it. It is told yes on the board
 * when it asks about the hooks, writes them into that home and has Codex
 * approve them, as it does for a person. The person's ~/.codex is only read
 * for the sign-in, and the run's home is deleted at the end.
 *
 * The AI answers once, so its conversation has read the drawn name. The branch
 * is then renamed the way the folder's rename box renames it, and the AI is
 * asked three more things: which branch it is on, a plain question with
 * nothing to do with the branch, and -- last -- whether anything about the
 * branch confused it. Checked:
 *   - the notice is given exactly once, at the first turn after the rename
 *     (the app's log, and the hook's context in the CLI's own record)
 *   - the AI names the new branch
 * The answer to the last question is printed: that is the interview.
 *
 * Needs Windows, Node, git, and the CLI signed in. Spends four short turns of
 * that account. Nothing of a copy somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-branch-notice');
const APP = path.join(RUN, 'app');
const HOME = path.join(RUN, 'home');
const REPO = path.join(RUN, 'site');
const WT = path.join(RUN, 'site-mighty-gannet');
const HOOKS = path.join(RUN, 'claude-hooks.json');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const PORT = 9466;
const TOKEN = 'branch-notice-token-0123456789';
const CLI = (process.argv.find((a) => a.startsWith('--cli=')) || '--cli=claude').slice(6);
if (!['claude', 'codex'].includes(CLI)) throw new Error('--cli is claude or codex');
const CODEX = CLI === 'codex';
const TAB = CLI;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const git = (cwd, ...args) => {
  const r = spawnSync('git', args, { cwd, encoding: 'utf8' });
  if (r.status !== 0) die(`git ${args.join(' ')}: ${r.stderr}`);
  return r.stdout.trim();
};
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-branch-notice*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, REPO, HOME]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// A project and a worktree of it on a name drawn for it, never pushed
git(REPO, 'init', '-q', '-b', 'main');
fs.writeFileSync(path.join(REPO, 'README.md'), '# site\n');
git(REPO, 'add', '.');
git(REPO, '-c', 'user.name=test', '-c', 'user.email=test@example.invalid', 'commit', '-q', '-m', 'first');
git(REPO, 'worktree', 'add', '-q', '-b', 'mighty-gannet', WT);

const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
let command;
if (CODEX) {
  const signIn = path.join(os.homedir(), '.codex', 'auth.json');
  if (!fs.existsSync(signIn)) die('Codex is not signed in: ' + signIn + ' is missing');
  fs.mkdirSync(path.join(HOME, '.codex'), { recursive: true });
  fs.copyFileSync(signIn, path.join(HOME, '.codex', 'auth.json'));
  // Never an update (a CLI updating itself here updates the one this PC runs
  // everywhere), the sandbox that needs no administrator, and the worktree
  // trusted, so nothing stands in front of the first turn
  fs.writeFileSync(path.join(HOME, '.codex', 'config.toml'),
    'windows_wsl_setup_acknowledged = true\ncheck_for_update_on_startup = false\n\n' +
    '[windows]\nsandbox = "unelevated"\n\n' +
    `[projects.'${WT}']\ntrust_level = "trusted"\n`);
  // The hooks agreed to, so this copy writes them -- into this run's home
  answered['Codex CLI'] = 'on';
  command = ['codex', '--dangerously-bypass-approvals-and-sandbox'];
} else {
  // The hooks this app would write for Claude, pointed at the staged copy, in
  // a file of this run's own. The notice is waited for; the rest are not
  const hook = (arg, wait) => ({ type: 'command', command: appExe, args: ['--hook', arg], timeout: 3, ...(wait ? {} : { async: true }) });
  fs.writeFileSync(HOOKS, JSON.stringify({ hooks: {
    SessionStart: [{ hooks: [hook('session')] }],
    UserPromptSubmit: [{ hooks: [hook('state:BUSY'), hook('notice', true)] }],
    Stop: [{ hooks: [hook('state:DONE')] }],
  } }, null, 2));
  command = ['claude', '--settings', HOOKS, '--dangerously-skip-permissions'];
}
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  external_api: { access: 'user' },
  // The board, served on this machine only, is how this drives the copy: a
  // window's DevTools do not open under a home of the run's own
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  agent_hooks: answered,
  desks: [{ name: 'Notice', id: 'notice', folders: [{ cwd: WT, tabs: [{ name: TAB, id: TAB, command }] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX_HOME)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
if (CODEX) Object.assign(env, { USERPROFILE: HOME, HOME, CODEX_HOME: path.join(HOME, '.codex'), TERM: 'xterm-256color' });
const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

let door;
const openDoor = async () => {
  const tokenFile = path.join(APP, 'data', 'api-token');
  const end = Date.now() + 60000;
  while (!fs.existsSync(tokenFile) && Date.now() < end) await sleep(500);
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); }
  });
  const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
  await line({ token: fs.readFileSync(tokenFile, 'utf8').trim() });
  let n = 0;
  door = async (method, ...params) => {
    const a = await line({ id: String(++n), method, params });
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(700); }
  throw new Error('timed out waiting for ' + what);
};
// The board, as a person's phone reaches it: the token traded for cookies
const BASE = `http://127.0.0.1:${PORT}`;
let cookies = '';
const cookieFrom = async (url) => {
  const r = await fetch(BASE + url, { redirect: 'manual', signal: AbortSignal.timeout(10000) });
  const got = r.headers.getSetCookie().map((c) => c.split(';')[0]);
  const kept = cookies.split('; ').filter((c) => c && !got.some((g) => g.split('=')[0] === c.split('=')[0]));
  cookies = [...kept, ...got].join('; ');
};
const boardState = async () => (await fetch(`${BASE}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) })).json();
const intent = (o) => fetch(`${BASE}/api/intent?t=${TOKEN}`, { method: 'POST', headers: { Cookie: cookies }, body: JSON.stringify(o), signal: AbortSignal.timeout(10000) });

const screen = () => door('tab_screen', TAB).then((s) => String(s || ''));
const log = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '');
const told = () => (log().match(/^.*notice: .* is told: .*$/gm) || []);

// The CLI's own record of this conversation: Claude Code's newest under the
// worktree's folder in its projects, Codex's newest rollout in this run's home
// (Codex files them by date in folders; Claude's folders beside the record
// hold records of their own, of its helpers, which are not the conversation).
// Claude keeps the records of earlier runs in the same folder, so only one
// written since this run began is this run's
const began = Date.now();
const newest = (dir, test) => {
  if (!fs.existsSync(dir)) return null;
  const all = fs.readdirSync(dir, { recursive: CODEX }).map((f) => path.join(dir, String(f)))
    .filter((f) => test(f) && fs.statSync(f).mtimeMs >= began);
  return all.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs)[0] || null;
};
const record = () => {
  const file = CODEX
    ? newest(path.join(HOME, '.codex', 'sessions'), (f) => /rollout-.*\.jsonl$/.test(f))
    : newest(path.join(os.homedir(), '.claude', 'projects', WT.replace(/[^A-Za-z0-9]/g, '-')), (f) => f.endsWith('.jsonl'));
  if (!file) return [];
  return fs.readFileSync(file, 'utf8').split('\n').filter(Boolean).map((l) => { try { return JSON.parse(l); } catch { return null; } }).filter(Boolean);
};
const answers = () => (CODEX
  ? record().filter((r) => r.type === 'response_item' && r.payload?.type === 'message' && r.payload.role === 'assistant')
    .map((r) => (r.payload.content || []).filter((c) => c.type === 'output_text').map((c) => c.text).join('\n'))
  : record().filter((r) => r.type === 'assistant')
    .map((r) => (r.message.content || []).filter((c) => c.type === 'text').map((c) => c.text).join('\n'))).filter(Boolean);
const lastAnswer = () => answers().at(-1) || '';

// One turn: typed, sent, and waited for until the answer is in the record
const turn = async (words) => {
  const before = answers().length;
  await door('send', TAB, words);
  await sleep(1500);
  await door('send', TAB, '\r');
  await until(async () => answers().length > before && (await door('state', TAB)) !== 'BUSY', 'the AI to answer', 180000);
  await sleep(1500);
  return lastAnswer();
};

// Up to the prompt. A question this does not know is left on the screen and
// fails the run, rather than answered with whatever its first choice is
const ready = CODEX
  ? /Ask Codex to do anything|context left|esc to interrupt|\/ commands|full access/i
  : /bypass permissions|for shortcuts/i;
const bringUp = async () => {
  const end = Date.now() + 10 * 60000;
  while (Date.now() < end) {
    const s = await screen();
    if (/Setting up sandbox|Input disabled until setup/i.test(s)) { await sleep(3000); continue; }
    if (/Couldn't set up your sandbox/i.test(s) && /non-admin sandbox/i.test(s)) {
      await door('send', TAB, '2'); await sleep(300); await door('send', TAB, '\r'); await sleep(3000); continue;
    }
    if (/I trust this folder/i.test(s)) { await door('send', TAB, '\x1b[B'); await sleep(300); await door('send', TAB, '\r'); await sleep(2500); continue; }
    if (/Do you trust|allow Codex to work|Trust and continue/i.test(s)) { await door('send', TAB, '\r'); await sleep(2500); continue; }
    if (/Update available|Update now/i.test(s) && /Skip/i.test(s)) { await door('send', TAB, '\x1b'); await sleep(2000); continue; }
    if (ready.test(s)) return;
    await sleep(1000);
  }
  throw new Error(`${TAB} did not come up; its screen:\n${await screen()}`);
};

try {
  await openDoor();
  await until(async () => { await cookieFrom(`/?t=${TOKEN}`); return /(^|; )rs=/.test(cookies); }, 'the board', 60000);
  if (CODEX) {
    // The hooks, agreed to the way a person agrees: on the board's question
    const seq = async () => (await boardState()).ui?.hook_ask?.seq;
    await until(async () => (await seq()) != null, 'the question about the hooks', 60000);
    await intent({ kind: 'agenthooks', answer: 'on', seq: await seq() });
    const file = path.join(HOME, '.codex', 'hooks.json');
    await until(async () => fs.existsSync(file) && fs.readFileSync(file, 'utf8').includes('notice'), 'the hooks to be written', 60000);
    await until(async () => /Codex approved \d+ hook/.test(log()), 'Codex to approve the hooks', 120000);
    const written = JSON.parse(fs.readFileSync(file, 'utf8'));
    check(JSON.stringify(written.hooks?.UserPromptSubmit || []).includes('notice'), 'the notice hook is written on UserPromptSubmit and approved by Codex');
    // The tab started before the hooks were there; started again, it reads them
    await door('restart', TAB, 'fresh');
    await sleep(3000);
  }
  await bringUp();
  await sleep(3000);

  console.log('1. a turn on the drawn branch');
  console.log('   > ' + (await turn('Reply with only the word READY.')).replace(/\s+/g, ' '));
  check(told().length === 0, 'nothing is told before anything changed');

  console.log('2. the branch is renamed the way the rename box does it');
  // The settings, reached through the board as a phone reaches them
  await cookieFrom(`/cfg?t=${TOKEN}`);
  let renamed;
  await until(async () => {
    const r = await fetch(BASE + '/api/folder/rename', {
      method: 'POST', headers: { Cookie: cookies, 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: WT, name: 'fix/login-form', go: true }), signal: AbortSignal.timeout(20000),
    });
    if (r.status === 503) { await intent({ kind: 'opensettings', ret: false }); return false; }
    renamed = await r.json();
    return true;
  }, 'the settings', 60000);
  check(renamed.ok === true && git(WT, 'branch', '--show-current') === 'fix/login-form', `the branch is now fix/login-form (${JSON.stringify(renamed)})`);

  console.log('3. the next turn');
  const which = await turn('Run `git branch --show-current` and tell me in one sentence which branch this folder is on.');
  console.log('   > ' + which.replace(/\s+/g, ' '));
  check(told().length === 1, `the notice was given once (${told().length} in the log)`);
  const context = record().filter((r) => JSON.stringify(r).includes('renamed the git branch of this folder'));
  check(context.length >= 1, 'the notice is in the CLI\'s own record of the turn');
  check(/fix\/login-form/.test(which), 'the AI names the new branch');

  console.log('4. a turn with nothing to do with the branch');
  console.log('   > ' + (await turn('What is 17 times 3? Reply with only the number.')).replace(/\s+/g, ' '));
  check(told().length === 1, `nothing more is told (${told().length} in the log)`);

  console.log('5. the interview');
  const interview = await turn(
    'One honest question about this conversation, not about code: was there any moment you were confused or surprised ' +
    'about which git branch this folder is on -- for example, did it look as if somebody had switched branches? ' +
    'Say what you knew about the branch and when you learned it, in three sentences or fewer.');
  console.log('   > ' + interview.replace(/\s+/g, ' '));
  check(told().length === 1, `the interview turn is not told again (${told().length} in the log)`);
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  stopApp();
  // The sign-in copied into this run's home goes with it
  if (CODEX) { await sleep(1500); fs.rmSync(HOME, { recursive: true, force: true }); }
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
