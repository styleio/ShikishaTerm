/**
 * A real Claude Code is told, once, that the branch under it was renamed.
 *
 * This checkout's build in a folder of its own, with one real Claude Code tab
 * in a worktree on a drawn branch (`mighty-gannet`). The tab's hooks come from
 * a settings file of this run's own, handed to Claude with `--settings` -- the
 * person's own `~/.claude/settings.json` is neither read for this nor written,
 * and the copy is told every CLI's hook was answered "off", so it asks nothing
 * and writes nothing either.
 *
 *     cargo build
 *     node tools/debug/branch-notice.win.mjs
 *
 * Claude answers once, so its conversation has read the drawn name. The branch
 * is then renamed the way the folder's rename box renames it, and Claude is
 * asked three more things: which branch it is on, a plain question with
 * nothing to do with the branch, and -- last -- whether anything about the
 * branch confused it. Checked:
 *   - the notice is given exactly once, at the first turn after the rename
 *     (the app's log, and the hook's context in Claude's own record)
 *   - Claude names the new branch and does not say it was switched
 * Claude's answer to the last question is printed: that is the interview.
 *
 * Needs Windows, Node, git, and `claude` signed in. Spends four short turns of
 * that account. Nothing of a copy somebody is using is read, written or stopped.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-branch-notice');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'site');
const WT = path.join(RUN, 'site-mighty-gannet');
const HOOKS = path.join(RUN, 'claude-hooks.json');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const PORT = 9466;

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
for (const d of [APP, REPO]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// A project and a worktree of it on a name drawn for it, never pushed
git(REPO, 'init', '-q', '-b', 'main');
fs.writeFileSync(path.join(REPO, 'README.md'), '# site\n');
git(REPO, 'add', '.');
git(REPO, '-c', 'user.name=test', '-c', 'user.email=test@example.invalid', 'commit', '-q', '-m', 'first');
git(REPO, 'worktree', 'add', '-q', '-b', 'mighty-gannet', WT);

// The hooks this app would write for Claude, pointed at the staged copy, in a
// file of this run's own. The notice is waited for; the rest are not
const hook = (arg, wait) => ({ type: 'command', command: appExe, args: ['--hook', arg], timeout: 3, ...(wait ? {} : { async: true }) });
fs.writeFileSync(HOOKS, JSON.stringify({ hooks: {
  SessionStart: [{ hooks: [hook('session')] }],
  UserPromptSubmit: [{ hooks: [hook('state:BUSY'), hook('notice', true)] }],
  Stop: [{ hooks: [hook('state:DONE')] }],
} }, null, 2));

const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  external_api: { access: 'user' },
  agent_hooks: answered,
  desks: [{ name: 'Notice', id: 'notice', folders: [{ cwd: WT, tabs: [
    { name: 'claude', id: 'claude', command: ['claude', '--settings', HOOKS, '--dangerously-skip-permissions'] },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
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
const screen = () => door('tab_screen', 'claude').then((s) => String(s || ''));
const log = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '');
const told = () => (log().match(/^.*notice: .* is told: .*$/gm) || []);

// Claude Code's own record of this conversation: the newest under the
// worktree's folder in its projects
const record = () => {
  const dir = path.join(os.homedir(), '.claude', 'projects', WT.replace(/[^A-Za-z0-9]/g, '-'));
  if (!fs.existsSync(dir)) return [];
  const file = fs.readdirSync(dir).filter((f) => f.endsWith('.jsonl'))
    .map((f) => path.join(dir, f)).sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs)[0];
  if (!file) return [];
  return fs.readFileSync(file, 'utf8').split('\n').filter(Boolean).map((l) => { try { return JSON.parse(l); } catch { return null; } }).filter(Boolean);
};
const lastAnswer = () => {
  const said = record().filter((r) => r.type === 'assistant')
    .map((r) => (r.message.content || []).filter((c) => c.type === 'text').map((c) => c.text).join('\n')).filter(Boolean);
  return said[said.length - 1] || '';
};

// One turn: typed, sent, and waited for until Claude's answer is in its record
const turn = async (words) => {
  const before = record().filter((r) => r.type === 'assistant').length;
  await door('send', 'claude', words);
  await sleep(1500);
  await door('send', 'claude', '\r');
  await until(async () => {
    const answers = record().filter((r) => r.type === 'assistant');
    return answers.length > before && (await door('state', 'claude')) !== 'BUSY' && lastAnswer();
  }, 'Claude to answer', 180000);
  await sleep(1500);
  return lastAnswer();
};

let board;
try {
  await openDoor();
  await until(async () => {
    const s = await screen();
    if (/I trust this folder/i.test(s)) { await door('send', 'claude', '\x1b[B'); await sleep(300); await door('send', 'claude', '\r'); }
    return /bypass permissions|for shortcuts/i.test(s);
  }, 'Claude Code to come up', 120000);
  await sleep(3000);

  console.log('1. a turn on the drawn branch');
  console.log('   > ' + (await turn('Reply with only the word READY.')).replace(/\s+/g, ' '));
  check(told().length === 0, 'nothing is told before anything changed');

  console.log('2. the branch is renamed the way the rename box does it');
  const pages = (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).filter((t) => t.type === 'page');
  board = await connectCdp(pages[0]);
  await board.run(`send(${JSON.stringify({ kind: 'opensettings', ret: false })}); true`);
  let opened;
  await until(async () => (opened = log().match(/Loaded settings: (http:\/\/127\.0\.0\.1:\d+)\/\?token=([0-9a-f]+)/)), 'the settings page', 30000);
  const [, server, token] = opened;
  const renamed = await (await fetch(server + '/api/folder/rename', {
    method: 'POST', headers: { 'X-Token': token, 'Content-Type': 'application/json' },
    body: JSON.stringify({ path: WT, name: 'fix/login-form', go: true }),
  })).json();
  await board.run(`send(${JSON.stringify({ kind: 'closesettings' })}); true`);
  check(renamed.ok === true && git(WT, 'branch', '--show-current') === 'fix/login-form', `the branch is now fix/login-form (${JSON.stringify(renamed)})`);

  console.log('3. the next turn');
  const which = await turn('Run `git branch --show-current` and tell me in one sentence which branch this folder is on.');
  console.log('   > ' + which.replace(/\s+/g, ' '));
  check(told().length === 1, `the notice was given once (${told().length} in the log)`);
  const context = record().filter((r) => JSON.stringify(r).includes('renamed the git branch of this folder'));
  check(context.length >= 1, 'the notice is in Claude\'s own record of the turn');
  check(/fix\/login-form/.test(which), 'Claude names the new branch');

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
  if (board) board.stop();
  stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
