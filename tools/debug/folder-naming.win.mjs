/**
 * A folder describing itself from what was typed straight into its AI, with
 * nothing installed anywhere to make that happen.
 *
 * This is the case that used to go silent. The app hears requests from its own
 * input bar, and from a CLI that has been given a hook to report them -- and
 * from neither when somebody types into the terminal of a CLI whose settings
 * have not been touched. Folders sat unnamed for weeks for that reason. The
 * requests are now read out of the record the CLI keeps of its own
 * conversation (`crate::asks`), which needs no arrangement with anyone.
 *
 * So this starts the app built in this checkout with one git repository and
 * one AI tab whose CLI is a stand-in that only writes down the command line it
 * was given. From that the id the app minted is read, a record is written under
 * that id the way Claude Code writes one -- a person's words between two
 * tool answers -- and the app is watched for the one thing that never used to
 * happen: the folder's description being asked for at all.
 *
 *     cargo build
 *     node tools/debug/folder-naming.win.mjs
 *
 * Needs Windows, Node and git. Isolated the way a new-user run is: its own
 * folder, its own home, its own LOCALAPPDATA. Nothing of a copy somebody is
 * using is read, written or stopped, no account is used and nothing leaves the
 * machine. The app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9348;
const RUN = path.join(os.tmpdir(), 'sk-fnaming');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'work', 'shop');
const HOME = path.join(RUN, 'home');
const STUB = path.join(RUN, 'stub');
const CONFIG = path.join(APP, 'config', 'config.json');
const ARGV = path.join(RUN, 'argv.txt');

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
for (const d of [APP, REPO, HOME, STUB, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const git = (...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd: REPO, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
};
git('init', '-q', '-b', 'main');
fs.writeFileSync(path.join(REPO, 'readme.md'), 'a shop\n');
git('add', '-A');
git('commit', '-q', '-m', 'start');

// The CLI, as far as this check needs one. Two jobs, told apart by the first
// argument the app gives it:
//
//   `-p`   the one-shot call this app makes to have a name written. It answers
//          the object that call asks for, so no account is used and nothing
//          leaves the machine
//   else   the tab itself. It writes down the command line -- which is how the
//          id the app minted is read -- and then sits there so the tab lives
fs.writeFileSync(path.join(STUB, 'claude.cmd'),
  '@echo off\r\n'
  + 'if "%~1"=="-p" (\r\n'
  + '  echo {"name":"Login page","summary":"Taking an email address on the login page, and saying which of the two was wrong when a sign-in fails."}\r\n'
  + '  exit /b 0\r\n'
  + ')\r\n'
  + `echo %*> "${ARGV}"\r\n`
  + ':wait\r\n'
  + 'timeout /t 60 /nobreak >nul\r\n'
  + 'goto wait\r\n');

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  desks: [{
    name: 'Check', id: 'check',
    folders: [{
      cwd: REPO, auto_label: true,
      tabs: [{ name: 'claude', id: 'claude', command: 'claude' }],
    }],
  }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.USERPROFILE = HOME;
env.PATH = STUB + ';' + env.PATH;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const until = async (test, what, ms = 60000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(400); }
  throw new Error('timed out waiting for ' + what);
};
const hookLog = () => {
  try { return fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8'); } catch { return ''; }
};
const folder = () => {
  try {
    const cfg = JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
    return (cfg.desks[0].folders || [])[0] || {};
  } catch { return {}; }
};

try {
  console.log('1. the app launches the CLI with an id of its own, so it knows the conversation');
  await until(() => Promise.resolve(fs.existsSync(ARGV) && /--session-id/.test(fs.readFileSync(ARGV, 'utf8'))),
    'the CLI to be launched with a conversation id');
  const line = fs.readFileSync(ARGV, 'utf8').trim();
  const id = (line.match(/--session-id\s+(\S+)/) || [])[1];
  check(!!id, 'the command line carries one: ' + line);

  console.log('2. nothing was installed into the CLI\'s own settings');
  check(!fs.existsSync(path.join(HOME, '.claude', 'settings.json')),
    'the CLI\'s settings file was not written to');

  console.log('3. the person types into the terminal, and the CLI writes it down as CLIs do');
  const recordDir = path.join(HOME, '.claude', 'projects', 'shop');
  fs.mkdirSync(recordDir, { recursive: true });
  const record = path.join(recordDir, id + '.jsonl');
  const asked = (text) => JSON.stringify({ type: 'user', message: { role: 'user', content: text } }) + '\n';
  const returned = () => JSON.stringify({
    type: 'user', toolUseResult: {},
    message: { role: 'user', content: [{ tool_use_id: 't1', type: 'tool_result', content: 'x'.repeat(4000) }] },
  }) + '\n';
  fs.writeFileSync(record,
    returned()
    + asked('make the login page take an email address instead of a user name')
    + returned()
    + asked('and when the sign-in fails, say which of the two was wrong'));

  console.log('4. the folder names and describes itself -- the thing that never used to happen');
  await until(() => Promise.resolve(
    (!!folder().name && !!folder().summary) || hookLog().includes('folder label for ')),
    'the folder to name itself, or to say why it could not', 180000);
  const f = folder();
  check(f.name === 'Login page', 'the name the AI wrote is in the settings: ' + JSON.stringify(f.name));
  check(!!f.summary, 'so is the summary: ' + JSON.stringify((f.summary || '').slice(0, 50)));
  check(!hookLog().includes('folder label for '), 'nothing went wrong on the way');
} catch (e) {
  console.error(e.message);
  const tail = hookLog().split('\n').slice(-12).join('\n');
  if (tail.trim()) console.error('--- the end of hooks.log ---\n' + tail);
  failures += 1;
} finally {
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);
