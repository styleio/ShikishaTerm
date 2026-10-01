/**
 * A Claude Code tab whose turn has ended, with a log watcher (`tail -F`) it
 * started still running after the shell it ran it in has gone.
 *
 * On Windows nothing ends such a process: it stays in the tab's job, and a
 * count of the job's processes read the tab as at work behind its prompt for
 * hours (2026-10-01, a tab of a measuring run held green for 6 h 40 min). A
 * CLI that lists what it left running is believed over that count.
 *
 * This checkout's build in a folder of its own, with one real Claude Code
 * tab. The tab's hooks come from a settings file of this run's own, handed to
 * Claude with `--settings` -- the person's own `~/.claude/settings.json` is
 * neither read for this nor written, and the copy is told every CLI's hook
 * was answered "off", so it asks nothing and writes nothing either. Claude is
 * asked to start the watcher detached from its shell and to end its turn.
 *
 *     cargo build
 *     node tools/debug/orphan-status.win.mjs
 *
 * Checked, from the tab's state read once a second and the app's log:
 *   - the watcher is still running once the turn has ended
 *   - from a few seconds after the answer, the tab never reads BACKGROUND,
 *     and it comes to rest (DONE)
 *
 * Needs Windows, Node, and `claude` signed in. Spends one turn of that
 * account. The watcher is ended on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-orphan-status');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const HOOKS = path.join(RUN, 'claude-hooks.json');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-orphan-status*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// The hooks this app would write for Claude, pointed at the staged copy, in a
// file of this run's own
const hook = (arg) => ({ hooks: [{ type: 'command', command: appExe, args: ['--hook', arg], timeout: 3, async: true }] });
fs.writeFileSync(HOOKS, JSON.stringify({ hooks: {
  SessionStart: [hook('session')],
  UserPromptSubmit: [hook('state:BUSY')],
  PermissionRequest: [hook('state:QUESTION')],
  Stop: [hook('state:DONE')],
  StopFailure: [hook('state:FAILED')],
  SubagentStart: [hook('helper:up')],
  SubagentStop: [hook('helper:down')],
} }, null, 2));

// Every CLI with a hook answered "off": nothing is asked, nothing written
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
  desks: [{ name: 'Orphans', id: 'orphans', folders: [{ cwd: WORK, tabs: [
    { name: 'claude', id: 'claude', command: ['claude', '--settings', HOOKS, '--dangerously-skip-permissions'] },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
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

// The watcher left behind: a `tail -F` on a file of this run's own, so it is
// told from anything else running on this PC
const WATCHED = path.join(WORK, 'watched.log').replace(/\\/g, '/');
fs.writeFileSync(path.join(WORK, 'watched.log'), 'start\n');
const tails = () => {
  const r = ps('-Command', `@(Get-CimInstance Win32_Process -Filter "Name='tail.exe'" | Where-Object { ($_.CommandLine + '') -like '*watched.log*' -and ($_.CommandLine + '') -like '*sk-orphan-status*' }).Count`);
  return Number((r.stdout || '0').trim()) || 0;
};

try {
  await openDoor();
  await until(async () => {
    const s = await screen();
    if (/I trust this folder/i.test(s)) { await door('send', 'claude', '\x1b[B'); await sleep(300); await door('send', 'claude', '\r'); }
    return /bypass permissions|for shortcuts/i.test(s);
  }, 'Claude Code to come up', 120000);
  await sleep(3000);

  console.log('asking for a watcher left behind');
  await door('send', 'claude',
    `Run exactly this one command with the Bash tool, in the foreground (not in the background): (tail -n 0 -F ${WATCHED} > /dev/null 2>&1 &) ; echo LAUNCHED . ` +
    'Then reply to me with only the word LEFT and end your turn. Do not run anything else and do not stop the tail.');
  // The Enter on its own: arriving with the text, it is taken as part of a paste
  await sleep(1500);
  await door('send', 'claude', '\r');

  // One reading a second, from the request until a minute after the answer
  const seen = [];
  const t0 = Date.now();
  let leftAt = null;
  while (Date.now() - t0 < 4 * 60 * 1000) {
    const st = await door('state', 'claude').catch(() => '?');
    const s = await screen();
    const at = Math.round((Date.now() - t0) / 1000);
    seen.push([at, st]);
    // Claude's own line, not the request's echo of the word
    if (leftAt === null && /^\W*LEFT\s*$/m.test(s) && st !== 'BUSY') leftAt = at;
    if (leftAt !== null && at - leftAt > 60) break;
    await sleep(1000);
  }
  const runs = [];
  for (const [at, st] of seen) {
    if (!runs.length || runs[runs.length - 1][1] !== st) runs.push([at, st]);
  }
  console.log('states: ' + runs.map(([at, st]) => `${at}s ${st}`).join(' -> '));
  const text = log();
  check(leftAt !== null, 'Claude ran the command and ended its turn (LEFT on screen)');
  check(tails() > 0, 'the watcher is still running after the shell it was started in has gone');
  check(!/at work behind its prompt: its processes/.test(text), 'the tab was never held by the count of its processes');
  if (leftAt !== null) {
    const after = seen.filter(([at]) => at >= leftAt + 5).map(([, st]) => st);
    check(after.length > 0 && !after.includes('BACKGROUND'),
      `with nothing Claude says is still running, the watcher left behind does not hold the tab at work: ${[...new Set(after)].join(', ')}`);
    check(after[after.length - 1] === 'DONE', `the tab came to rest (${after[after.length - 1]})`);
  }
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (process.env.ORPHAN_DEBUG) {
    try { console.log('screen:\n' + (await screen())); } catch {}
    console.log(log());
  }
  const lines = log().split(/\r?\n/).filter((l) => /behind its prompt|State .*claude|tab2|said/.test(l));
  if (lines.length) console.log('log:\n  ' + lines.slice(-15).join('\n  '));
  stopApp();
  // What this run left behind on this PC goes with it
  ps('-Command', `Get-CimInstance Win32_Process -Filter "Name='tail.exe'" | Where-Object { ($_.CommandLine + '') -like '*sk-orphan-status*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
