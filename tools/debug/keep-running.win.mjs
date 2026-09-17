/**
 * An AI at work keeps working while other folders and desks are changed around it.
 *
 * What went wrong once, in this order: an AI tab was given a job in one
 * worktree; then another worktree of the same project was deleted, the desk
 * was switched, and -- in one save of the settings -- the desk on screen was
 * deleted while the desk with the AI in it was renamed. The AI stopped
 * mid-task, and its tab came back as a new conversation.
 *
 * So this starts the app built in this checkout, in a folder of its own, gives
 * a real Claude Code a job that takes over a minute, does those same things
 * through the app's own doors while it runs, and then checks what matters:
 * the job finished in the same conversation, no second conversation was
 * started in that folder, and no Claude Code tab exited.
 *
 *     cargo build
 *     node tools/debug/keep-running.win.mjs [--only=worktree|switch|desks]
 *
 * Needs Windows, Node, git, and `claude` signed in on this machine. It spends
 * one short turn of that account (a sleep and a one-word reply). The worktree
 * with nothing at work in it runs `cmd.exe` under the same tab name, which is
 * all it takes to be mistaken for the AI's tab. Isolated the way a new-user
 * run is: its own folder, its own LOCALAPPDATA, started from its own folder.
 * Nothing of a copy somebody is using is read, written or stopped.
 * Photographs land in target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const PORT = 9343;
// One of the three alone, to find which of them does it: worktree, switch, or
// desks (which switches first, since it deletes the desk on screen). All three
// in order when absent
const ONLY = (process.argv.find((a) => a.startsWith('--only=')) || '').slice('--only='.length);
if (ONLY && !['worktree', 'switch', 'desks'].includes(ONLY)) { console.error('--only takes worktree, switch or desks'); process.exit(2); }
const RUN = path.join(os.tmpdir(), 'sk-keep');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'work', 'repo');
const GONE = path.join(RUN, 'work', 'wt-gone');
const BUSY = path.join(RUN, 'work', 'wt-busy');
const SCRATCH = path.join(RUN, 'work', 'scratch');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
// Where Claude Code files the conversations of a folder: its path with every
// character that is not a letter or a digit turned into a dash
const CONVERSATIONS = path.join(os.homedir(), '.claude', 'projects', BUSY.replace(/[^A-Za-z0-9]/g, '-'));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });

/** This checkout's copy, stopped by where it runs from and nothing else */
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (spawnSync('where.exe', ['claude'], { encoding: 'utf8' }).status !== 0) die('claude is not on PATH');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
for (const wt of [GONE, BUSY]) {
  if (fs.existsSync(REPO)) spawnSync('git', ['worktree', 'remove', '--force', wt], { cwd: REPO });
}
fs.rmSync(RUN, { recursive: true, force: true });
fs.rmSync(CONVERSATIONS, { recursive: true, force: true });
for (const d of [APP, REPO, SCRATCH, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const git = (cwd, ...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
};
git(REPO, 'init', '-q');
git(REPO, 'commit', '-q', '--allow-empty', '-m', 'start');
git(REPO, 'worktree', 'add', '-q', '-b', 'gone', GONE);
git(REPO, 'worktree', 'add', '-q', '-b', 'busy', BUSY);
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

// The shape the incident had: the deleted worktree listed before the busy one,
// both with a tab called "claude", and a second desk to be on when the first
// one is renamed
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: false },
  confirm_worktree_delete: false,
  desks: [
    { name: 'DEFAULT', id: 'default', folders: [
      { cwd: REPO, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] },
      { cwd: GONE, name: 'gone', tabs: [{ name: 'claude', id: 'claude@gone', command: 'cmd.exe' }] },
      { cwd: BUSY, name: 'busy', tabs: [{ name: 'claude', id: 'claude', command: 'claude --dangerously-skip-permissions' }] },
    ] },
    { name: 'ワークスペース', id: 'space', folders: [
      { cwd: SCRATCH, tabs: [{ name: 'claude', id: 'claude', command: 'cmd.exe' }] },
    ] },
  ],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const pages = async () => {
  try { return (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { return []; }
};
const connect = async (target) => {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
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
  return { ws, send, run };
};
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};
const shot = (name) => ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'),
  '-Under', RUN, '-Out', path.join(SHOTS, `keep-running-${name}.png`));
const conversations = () => (fs.existsSync(CONVERSATIONS) ? fs.readdirSync(CONVERSATIONS).filter((f) => f.endsWith('.jsonl')) : []);
const said = (file) => fs.readFileSync(path.join(CONVERSATIONS, file), 'utf8');

let board;
try {
  let targets = [];
  await until(async () => (targets = await pages()).length > 0, 'the app\'s page to open its DevTools port', 30000);
  board = await connect(targets[0]);
  const { run } = board;
  const say = (o) => run(`send(${JSON.stringify(o)}); true`);
  // The AI's tab, by the name automation calls it, on the desk in front
  const aiTab = () => run(`(() => { const t = (S.tabs || []).find(t => t.id === 'claude' && t.kind !== 'page'); return t ? {index: t.index, state: t.state} : null; })()`);

  console.log('1. the AI in the busy worktree is given a job that takes over a minute');
  await until(async () => !!(await aiTab()), 'the AI tab', 30000);
  const { index } = await aiTab();
  await say({ kind: 'select', tab: index });
  const screen = () => run(`document.getElementById('screen').innerText`);
  // A folder Claude Code has never seen asks to be trusted first. The answer
  // is typed the way the terminal's keyboard sends it: down to "Yes", Enter
  await until(async () => /I trust this folder|bypass permissions|for shortcuts/i.test(await screen()), 'Claude Code to come up', 60000);
  if (/I trust this folder/i.test(await screen())) {
    await say({ kind: 'key', named: 'down', shift: false, alt: false });
    await sleep(500);
    await say({ kind: 'key', named: 'enter', shift: false, alt: false });
  }
  await until(async () => /bypass permissions|for shortcuts/i.test(await screen()), 'the prompt', 60000)
    .catch(async (e) => { console.log('--- the screen ---\n' + (await screen()) + '\n---'); throw e; });
  await sleep(1500);
  await say({ kind: 'say', tab: index, text: 'Run exactly this with the Bash tool and wait for it to finish: sleep 75 . After it finishes, reply with only the word FINISHED.' });
  await until(async () => conversations().some((f) => /sleep 75/.test(said(f))), 'the job to be in its conversation', 90000);
  const before = conversations();
  check(before.length === 1, 'one conversation in the busy folder: ' + before.join(', '));
  const conversation = before[0];
  await until(async () => /tool_use/.test(said(conversation)), 'the sleep to start', 60000);
  shot('1-at-work');

  if (!ONLY || ONLY === 'worktree') {
    console.log('2. the other worktree of the same project is deleted');
    await say({ kind: 'folderdiscard', folder: GONE, unasked: false });
    await until(async () => !fs.readFileSync(CONFIG, 'utf8').includes('wt-gone'), 'the worktree out of the settings', 30000);
    await until(async () => !fs.existsSync(GONE), 'the worktree off the disk', 60000);
    await sleep(2500);
    check(true, 'the worktree is gone');
  }

  const toDesk = async (n) => {
    await say({ kind: 'opendesk' });
    await until(() => run(`!!S.desk_open`), 'the desk list');
    await say({ kind: 'menu', key: String(n) });
    await until(() => run(`S.desk_index === ${n - 1}`), `desk ${n} in front`);
    await sleep(1500);
  };
  if (!ONLY || ONLY === 'switch' || ONLY === 'desks') {
    console.log('3. the desk is switched');
    await toDesk(2);
    check(true, 'the second desk is in front');
  }
  if (ONLY === 'switch') {
    await toDesk(1);
    check(true, 'and back to the first');
  }

  if (!ONLY || ONLY === 'desks') {
    console.log('4. in one save: the desk on screen deleted, the one with the AI renamed');
    // The settings page is opened the way its gear opens it, and saved the way
    // its Save button saves: the whole settings, posted to the page's own server
    // with the page's own token. The page itself is a WebView of its own that
    // the DevTools port does not reach, so what it would post is posted here
    await say({ kind: 'opensettings', ret: false });
    let opened;
    await until(async () => {
      opened = (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '').match(/Loaded settings: (http:\/\/127\.0\.0\.1:\d+)\/\?token=([0-9a-f]+)/);
      return !!opened;
    }, 'the settings page', 30000);
    const [, server, token] = opened;
    const read = await fetch(server + '/api/config', { headers: { 'X-Token': token } });
    const edited = await read.json();
    edited.desks.splice(1, 1);
    edited.desks[0].name = 'ワイアード＆エコ';
    const posted = await fetch(server + '/api/config', {
      method: 'POST', headers: { 'X-Token': token, 'Content-Type': 'application/json' }, body: JSON.stringify(edited, null, 2),
    });
    check((await posted.json()).ok === true, 'the settings were saved through the settings page\'s server');
    await say({ kind: 'closesettings' });
    await until(() => run(`S.desks.length === 1 && S.desks[0] === 'ワイアード＆エコ' && S.desk_index === 0`), 'the renamed desk in front', 30000);
    await sleep(1500);
  }
  shot('2-after');

  console.log('5. the job finishes, in the same conversation');
  await until(async () => /FINISHED/.test(said(conversation).split('\n').filter((l) => /"type":"assistant"/.test(l)).slice(-3).join('\n')),
    'FINISHED from the same conversation', 180000);
  check(true, 'FINISHED arrived in ' + conversation);
  const after = conversations();
  check(after.length === 1 && after[0] === conversation, 'no new conversation was started in that folder: ' + after.join(', '));
  const now = await aiTab();
  check(!!now && now.state !== 'EXIT', 'the AI tab is still running: ' + JSON.stringify(now));
  check(!/EXIT \[Claude Code\]/.test(fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : ''), 'the log shows no Claude Code tab exiting');
  shot('3-finished');
} catch (e) {
  check(false, e.message);
} finally {
  board?.ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\keep-running-*.png');
process.exit(failures ? 1 : 0);
