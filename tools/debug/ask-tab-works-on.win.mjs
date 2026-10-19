/**
 * An ask whose tab ends its turn with work still going on behind its prompt:
 * what it says then is passed on as said so far, and what it says once that
 * work is over reaches the caller's inbox too.
 *
 * Measured 2026-10-19: an AI asked for a long piece of work said "waiting for
 * verify and the review" with two helpers out. That went to the caller as the
 * answer, the ask was closed, and the report it gave 40 minutes later went to
 * nobody.
 *
 *     cargo build
 *     node tools/debug/ask-tab-works-on.win.mjs [--only=held|late] [--keep]
 *
 * The asker is a Git Bash tab running `shikisha`, so what it is told can be
 * read whole from files; the worker is a real Claude Code tab, told to start
 * one helper in the background that takes about a minute and to end its turn
 * at once. Two roads:
 *
 *   held -- the worker says its first words while the asker still holds the
 *           line: the call prints them with STILL WORKING, and the last
 *           words come through the inbox
 *   late -- the worker first works in the foreground past the asker's hold
 *           (the call prints STILL WORKING with nothing said), so its first
 *           words come through the inbox too; that mail is dealt with, as
 *           an AI would, and the last words come after it
 *
 * Checked: what the call printed, each piece of mail, and the app's log
 * (`-> WORKING`, then `-> DONE`).
 *
 * Needs Windows, Node 22, Git for Windows, and `claude` signed in. Spends a
 * few turns of that account (two asks, a helper each). Isolated as
 * confer-real.win.mjs is: a home of its own with a copy of the sign-in, the
 * hooks written there, the CLI's own updates off, and all of it deleted at
 * the end (--keep leaves it).
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const arg = (name, dflt) => {
  const a = process.argv.find((x) => x.startsWith(`--${name}=`));
  return a ? a.slice(name.length + 3) : dflt;
};
const KEEP = process.argv.includes('--keep');
const ROADS = arg('only', '') ? [arg('only', '')] : ['held', 'late'];
const RUN = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-workson-'));
const INSTANCE = path.join(RUN, 'copy');
const APP = path.join(INSTANCE, 'app');
const HOME = path.join(RUN, 'home');
const WORK = path.join(RUN, 'work');
const ASKER = path.join(WORK, 'asker');
const WORKER = path.join(WORK, 'worker');
const CONFIG = path.join(RUN, 'config.json');
const TOKEN = 'works-on-check-token-0123456789';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const launcher = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8', windowsHide: true });
let cleaned = false;
const cleanup = () => {
  if (cleaned) return;
  cleaned = true;
  ps('-File', launcher, '-At', INSTANCE, '-Stop');
  if (KEEP) { console.log('kept: ' + RUN); return; }
  if (fs.existsSync(HOME) && fs.realpathSync(HOME) === path.join(fs.realpathSync(RUN), 'home')) {
    fs.rmSync(HOME, { recursive: true, force: true });
  }
};
process.on('exit', cleanup);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
{
  // A check of an older build says nothing about the code in front of you
  const built = fs.statSync(exe).mtimeMs;
  const newer = (dir) => fs.readdirSync(dir, { withFileTypes: true }).some((e) => {
    const p = path.join(dir, e.name);
    return e.isDirectory() ? newer(p) : fs.statSync(p).mtimeMs > built;
  });
  if (['src', path.join('crates', 'core', 'src')].some((d) => newer(path.join(ROOT, d)))) {
    die('the build at target\\debug is older than the sources -- run cargo build first');
  }
}
const bash = ['ProgramFiles', 'ProgramW6432'].map((k) => process.env[k]).filter(Boolean)
  .map((p) => path.join(p, 'Git', 'bin', 'bash.exe')).find((p) => fs.existsSync(p));
if (!bash) die('no Git for Windows under Program Files');
if (spawnSync('where.exe', ['claude'], { encoding: 'utf8' }).status !== 0) die('claude is not on PATH');
const REAL = os.homedir();
const SIGN_INS = [
  [path.join(REAL, '.claude', '.credentials.json'), path.join(HOME, '.claude', '.credentials.json')],
  [path.join(REAL, '.claude.json'), path.join(HOME, '.claude.json')],
];
for (const [from] of SIGN_INS) if (!fs.existsSync(from)) die(`not signed in: ${from} is missing`);

// ── The copy, its home, its folders and its tabs ───────────────────────────
for (const d of [APP, path.join(HOME, '.claude'), ASKER, WORKER]) fs.mkdirSync(d, { recursive: true });
for (const [from, to] of SIGN_INS) fs.copyFileSync(from, to);
// Running without asking is agreed to once, here; and never an update: a CLI
// updating itself here updates the one this PC runs everywhere
fs.writeFileSync(path.join(HOME, '.claude', 'settings.json'), JSON.stringify({
  skipDangerousModePermissionPrompt: true, env: { DISABLE_AUTOUPDATER: '1' },
}, null, 2));
spawnSync('git', ['init', '-q'], { cwd: WORKER });

fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  keep_terminals: false,
  remote: { sticky_token: true, fixed_token: TOKEN },
  external_api: { access: 'user' },
  agent_hooks: { 'Claude Code': 'on' },
  desks: [{ name: 'Works on', id: 'works-on', folders: [
    { cwd: ASKER, tabs: [{ name: 'asker', id: 'asker', command: [bash, '--login', '-i'] }] },
    { cwd: WORKER, tabs: [{ name: 'worker', id: 'worker', command: ['claude', '--dangerously-skip-permissions'] }] },
  ] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
Object.assign(env, { USERPROFILE: HOME, HOME, DISABLE_AUTOUPDATER: '1', TERM: 'xterm-256color' });
const started = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', launcher,
  '-At', INSTANCE, '-Work', WORK, '-Exe', exe, '-Config', CONFIG, '-Cdp', '0'],
  { env, encoding: 'utf8', windowsHide: true });
if (started.status !== 0) die('launch failed: ' + started.stdout + started.stderr);
const pid = Number(/^pid=(\d+)/m.exec(started.stdout)?.[1]);
const port = Number(/board=http:\/\/127\.0\.0\.1:(\d+)/.exec(started.stdout)?.[1]);
if (!pid || !port) die('the launcher did not identify its copy');
console.log('isolated copy: ' + RUN);

// ── The door: the pipe, as the person ─────────────────────────────────────
const dataDir = () => [path.join(APP, 'data'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data')]
  .find((d) => fs.existsSync(path.join(d, 'api-token')));
let door;
const openDoor = async () => {
  const end = Date.now() + 60000;
  let dir;
  while (!(dir = dataDir()) && Date.now() < end) await sleep(500);
  if (!dir) die('no api-token appeared');
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i); buf = buf.slice(i + 1);
      const w = waiting.shift(); if (w) w(JSON.parse(line));
    }
  });
  const line = (o) => new Promise((resolve) => { waiting.push(resolve); sock.write(JSON.stringify(o) + '\n'); });
  const hello = await line({ token: fs.readFileSync(path.join(dir, 'api-token'), 'utf8').trim() });
  if (!hello.ok) die('the door refused: ' + JSON.stringify(hello));
  let n = 0;
  door = async (method, ...params) => {
    const a = await line({ id: String(++n), method, params });
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await test().catch(() => false)) return true;
    await sleep(500);
  }
  throw new Error('timed out waiting for ' + what);
};
const screen = (id) => door('tab_screen', id).then((s) => String(s || ''));
const hooksLog = () => {
  const f = path.join(APP, 'logs', 'hooks.log');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '';
};

// The hooks, agreed to on the board the way a person agrees, then the worker
// started again to read them (as confer-real.win.mjs does)
const agreeToHooks = async () => {
  let cookies = '';
  const base = `http://127.0.0.1:${port}`;
  const board = async () => {
    const r = await fetch(`${base}/?t=${TOKEN}`, { redirect: 'manual', signal: AbortSignal.timeout(10000) });
    cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
  };
  const state = async () => (await fetch(`${base}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) })).json();
  await until(async () => { await board(); return /(^|; )rs=/.test(cookies); }, 'the board', 60000);
  await until(async () => !!(await state()).ui?.hook_ask, 'the question about the hooks', 60000);
  const asked = (await state()).ui.hook_ask;
  await fetch(`${base}/api/intent?t=${TOKEN}`, { method: 'POST', headers: { Cookie: cookies },
    body: JSON.stringify({ kind: 'agenthooks', answer: 'on', seq: asked.seq }), signal: AbortSignal.timeout(10000) });
  const settings = path.join(HOME, '.claude', 'settings.json');
  await until(async () => fs.readFileSync(settings, 'utf8').includes('--hook'), 'the hooks to be written', 60000);
  await door('restart', 'worker', 'fresh');
  await sleep(3000);
};

const bringUp = async () => {
  const end = Date.now() + 3 * 60000;
  while (Date.now() < end) {
    const s = await screen('worker');
    if (/I trust this folder|Do you trust/i.test(s)) { await door('send', 'worker', '\x1b[B'); await sleep(400); await door('send', 'worker', '\r'); await sleep(2500); continue; }
    if (/Bypass Permissions mode/i.test(s) && /Yes, I accept/i.test(s)) { await door('send', 'worker', '\x1b[B'); await sleep(400); await door('send', 'worker', '\r'); await sleep(2500); continue; }
    // A question this does not know is left on the screen and fails the run,
    // rather than answered with whatever its first choice is
    if (/bypass permissions|for shortcuts|\? for/i.test(s)) return;
    await sleep(1000);
  }
  throw new Error('the worker did not come up:\n' + await screen('worker'));
};

// ── The asker: commands typed into its bash, read back from files ──────────
// The app types its word about new mail into the asker as it arrives, and a
// command typed at the same moment is broken by it: the line is cleared
// first, and a command that never finished is typed again
let seq = 0;
const inAsker = async (command, ms) => {
  for (let tries = 0; ; tries += 1) {
    const n = ++seq;
    const out = path.join(ASKER, `out-${n}.txt`);
    const end = path.join(ASKER, `out-${n}.end`);
    const unix = (p) => p.replace(/\\/g, '/').replace(/^([A-Za-z]):/, (_, d) => '/' + d.toLowerCase());
    await door('send', 'asker', '\x03');
    await sleep(500);
    await door('send', 'asker', `${command} > '${unix(out)}' 2>&1; echo ok > '${unix(end)}'\r`);
    try {
      await until(async () => fs.existsSync(end), `the asker's command ${n}`, ms);
      return fs.readFileSync(out, 'utf8');
    } catch (e) {
      // A call that holds the line is not typed twice: only short ones are
      if (tries >= 2 || ms > 30000) throw e;
    }
  }
};
const inbox = async () => {
  const said = await inAsker('shikisha inbox', 30000);
  // The JSON, then a line of what to do next
  try { return JSON.parse(said.split(/\r?\n\r?\n\[shikisha\]/)[0]); } catch { return null; }
};
const dealt = (h) => inAsker(`shikisha inbox '{"dealt":${h}}'`, 30000);
// Mail until `test` finds what it wants in one piece of it; what comes before
// is dealt with as it comes, as an AI would
const mailUntil = async (test, what, ms) => {
  const seen = [];
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const box = await inbox();
    if (box && box.handover) {
      for (const m of box.mail) seen.push(m.text);
      await dealt(box.handover);
      const hit = box.mail.find((m) => test(m.text));
      if (hit) return { hit: hit.text, seen };
    }
    await sleep(10000);
  }
  throw new Error(`timed out waiting for ${what}; mail seen:\n${seen.join('\n---\n')}`);
};

const nonce = () => Math.random().toString(36).slice(2, 8).toUpperCase();
const helperTask = 'Use the Agent tool with run_in_background set to true to start exactly one general-purpose subagent. ' +
  'Its task: run the shell command `ping -n 60 127.0.0.1` (it takes about a minute), then read the file code.txt in this folder and report its contents. ' +
  'Do not wait for it: as soon as it has started, reply with only the words WAITING FOR THE HELPER and end your turn. ' +
  'When the helper has finished and you have its result, reply with FINAL: followed by the contents of code.txt as the helper reported them.';
const TASKS = {
  held: helperTask,
  late: 'First run the shell command `ping -n 110 127.0.0.1` in the foreground and wait for it to finish (about two minutes). Then: ' + helperTask,
};

const road = async (name) => {
  console.log(`\n${name}:`);
  const code = nonce();
  fs.writeFileSync(path.join(WORKER, 'code.txt'), code + '\n');
  fs.writeFileSync(path.join(ASKER, `task-${name}.txt`), TASKS[name]);
  const before = hooksLog().length;
  const t0 = Date.now();
  const printed = await inAsker(`shikisha ask_tab worker "Run a helper and report" "$(cat task-${name}.txt)"`, 4 * 60000);
  console.log(`  the call returned after ${Math.round((Date.now() - t0) / 1000)}s:\n    ${printed.trim().split('\n').slice(-3).join('\n    ')}`);
  if (name === 'held') {
    check(/WAITING FOR THE HELPER/.test(printed) && /\[shikisha\] STILL WORKING: <@worker> has said the above so far/.test(printed),
      'the call printed what it said so far, as still working');
  } else {
    check(/\[shikisha\] STILL WORKING: <@worker> has not finished/.test(printed), 'the call handed its wait over (still working, nothing said)');
    const first = await mailUntil((t) => /WAITING FOR THE HELPER/.test(t), 'what it said so far', 6 * 60000);
    check(/\[shikisha\] STILL WORKING: <@worker> has said the above so far/.test(first.hit),
      'what it said so far came by mail, as still working, and was dealt with');
  }
  const last = await mailUntil((t) => t.includes('FINAL') && t.includes(code), 'its last words', 8 * 60000);
  // A reply goes into the inbox as it was said; only what is not one is
  // marked with a [shikisha] line
  check(!/\[shikisha\]/.test(last.hit), `its last words came by mail as its answer (FINAL: ${code})`);
  check(last.seen.slice(0, -1).every((t) => /\[shikisha\] STILL WORKING/.test(t)), 'everything before them came marked as still working');
  const log = hooksLog().slice(before);
  check(/ask_tab: worker -> WORKING/.test(log) && /ask_tab: worker -> DONE/.test(log), 'the log says WORKING, then DONE');
  check(/helper \S+ began/.test(log), 'the helper was heard of');
  await sleep(15000);
};

try {
  await openDoor();
  await agreeToHooks();
  await bringUp();
  await until(async () => /\$\s*$/m.test(await screen('asker')), 'the asker\'s bash', 60000);
  for (const name of ROADS) await road(name);
} catch (e) {
  console.error(e.message || e);
  failures += 1;
  try { console.log('--- worker screen ---\n' + await screen('worker')); } catch {}
  console.log('--- log ---\n' + hooksLog().split(/\r?\n/).filter((l) => /ask_tab|helper|confer|orchestration|inbox/.test(l)).slice(-40).join('\n'));
}
console.log(failures ? `${failures} failed` : 'all passed');
cleanup();
process.exit(failures ? 1 : 0);
