/**
 * A Claude Code tab's dot while a helper it started on the side (a subagent
 * run in the background) is still at work after the conversation's own turn
 * has ended.
 *
 * This checkout's build in a folder of its own, with one real Claude Code
 * tab. The tab's hooks come from a settings file of this run's own, handed to
 * Claude with `--settings` -- the person's own `~/.claude/settings.json` is
 * neither read for this nor written, and the copy is told every CLI's hook
 * was answered "off", so it asks nothing and writes nothing either. Claude is
 * asked to start one background helper that works for about a minute and to
 * end its own turn at once.
 *
 *     cargo build
 *     node tools/debug/helper-status.win.mjs
 *
 * Checked, from the tab's state read once a second and the app's log:
 *   - the helper's beginning is heard (`helper … began`)
 *   - from the moment the turn's answer is on screen until the helper ends,
 *     the tab never reads DONE: it reads BACKGROUND, held by what the program
 *     says runs beside it
 *   - once the helper has ended and Claude has had its say about it, the tab
 *     comes to rest (DONE)
 *
 * Needs Windows, Node, and `claude` signed in. Spends two or three turns of
 * that account. Nothing of a copy somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-helper-status');
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
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-helper-status*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

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
  desks: [{ name: 'Helpers', id: 'helpers', folders: [{ cwd: WORK, tabs: [
    { name: 'claude', id: 'claude', command: ['claude', '--settings', HOOKS, '--dangerously-skip-permissions'] },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
// For the photograph of the board while the helper holds the tab
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const SHOT = path.join(ROOT, 'target', 'shots', 'helper-status-background.png');
const photograph = async () => {
  const f = path.join(env.LOCALAPPDATA, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return false;
  const port = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  const page = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
  if (!page) return false;
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  const got = await new Promise((r) => {
    ws.addEventListener('message', (e) => { const m = JSON.parse(e.data); if (m.id === 1) r(m.result && m.result.data); });
    ws.send(JSON.stringify({ id: 1, method: 'Page.captureScreenshot', params: { format: 'png' } }));
  });
  ws.close();
  if (!got) return false;
  fs.mkdirSync(path.dirname(SHOT), { recursive: true });
  fs.writeFileSync(SHOT, Buffer.from(got, 'base64'));
  return true;
};
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

try {
  await openDoor();
  await until(async () => {
    const s = await screen();
    if (/I trust this folder/i.test(s)) { await door('send', 'claude', '\x1b[B'); await sleep(300); await door('send', 'claude', '\r'); }
    return /bypass permissions|for shortcuts/i.test(s);
  }, 'Claude Code to come up', 120000);
  await sleep(3000);

  console.log('asking for one helper in the background');
  await door('send', 'claude',
    'Use the Agent tool with run_in_background set to true to start exactly one general-purpose subagent. ' +
    'Its task: think through the numbers from 1 to 40 one at a time, writing one short fact about each number, and finish with the word HELPERDONE. ' +
    'Tell it not to use any tools. Do not wait for it: as soon as it has started, reply to me with only the word STARTED and end your turn.');
  // The Enter on its own: arriving with the text, it is taken as part of a paste
  await sleep(1500);
  await door('send', 'claude', '\r');

  // One reading a second, from the request to the helper's end and a little after
  const seen = [];
  const t0 = Date.now();
  let startedAt = null;
  let endedAt = null;
  let shot = false;
  while (Date.now() - t0 < 6 * 60 * 1000) {
    const st = await door('state', 'claude').catch(() => '?');
    const s = await screen();
    const at = Math.round((Date.now() - t0) / 1000);
    seen.push([at, st]);
    // Claude's own line, not the request's echo of the word
    if (startedAt === null && /^\W*STARTED\s*$/m.test(s) && st !== 'BUSY') startedAt = at;
    if (!shot && startedAt !== null && st === 'BACKGROUND') shot = await photograph().catch(() => false);
    if (endedAt === null && /helper \S+ ended/.test(log())) endedAt = at;
    if (endedAt !== null && at - endedAt > 40) break;
    await sleep(1000);
  }
  const runs = [];
  for (const [at, st] of seen) {
    if (!runs.length || runs[runs.length - 1][1] !== st) runs.push([at, st]);
  }
  console.log('states: ' + runs.map(([at, st]) => `${at}s ${st}`).join(' -> '));
  const text = log();
  check(/helper \S+ began/.test(text), 'the helper\'s beginning was heard');
  check(startedAt !== null, 'Claude ended its turn with the helper still going (STARTED on screen)');
  check(endedAt !== null, 'the helper\'s end was heard');
  if (startedAt !== null && endedAt !== null) {
    const between = seen.filter(([at]) => at >= startedAt && at < endedAt).map(([, st]) => st);
    check(between.length > 0 && !between.includes('DONE') && !between.includes('WAIT'),
      `while the helper ran (${startedAt}s-${endedAt}s) the tab never read done: ${[...new Set(between)].join(', ')}`);
    check(between.includes('BACKGROUND'), 'it read BACKGROUND while only the helper was at work');
    check(/at work behind its prompt: .*what its program says runs beside it/.test(text), 'what held it was the program\'s word about its helper');
  }
  if (shot) console.log('  the board while the helper held the tab: ' + SHOT);
  const last = seen.length ? seen[seen.length - 1][1] : '?';
  check(last === 'DONE', `after the helper ended, the tab came to rest (${last})`);
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (process.env.HELPER_DEBUG) {
    try { console.log('screen:\n' + (await screen())); } catch {}
    console.log(log());
  }
  const lines = log().split(/\r?\n/).filter((l) => /helper|behind its prompt|set_running|said/.test(l));
  if (lines.length) console.log('log:\n  ' + lines.slice(-20).join('\n  '));
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
