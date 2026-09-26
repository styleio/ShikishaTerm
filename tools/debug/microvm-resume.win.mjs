/**
 * An AI tab in a folder on a MicroVM comes back, after the app is restarted,
 * running its CLI on the conversation it was having -- through the running
 * app and a real E2B account.
 *
 * The machine is given a stand-in `claude` that writes down the arguments it
 * was started with and makes the record of its conversation where Claude Code
 * keeps one, so what the app typed there is read back from the machine:
 *
 *   1. the first start hands it a new conversation under an id the app chose
 *   2. the app writes that id down
 *   3. restarted, the tab resumes that conversation, on the machine
 *   4. restarted with the record gone from the machine, it starts a new one
 *      under the same id, rather than failing on an id nobody has
 *   5. quitting the app ends its shell on the machine, and the AI in it, so
 *      the next start does not put a second one on the same conversation
 *   6. the machine paused while the app was closed -- what an hour away
 *      does -- and the app started again: the machine comes back, and the
 *      tab is on its conversation
 *   7. the machine paused while the app is open, and something typed into
 *      the tab: the machine comes back and it arrives; the tab takes up
 *      the same shell again, and what is typed after is answered on screen
 *   8. with its minutes set to one, the machine stays up for as long as its
 *      terminal is at work, and pauses about a minute after it stops
 *
 *     cargo build
 *     node tools/debug/microvm-resume.win.mjs
 *
 * Needs Windows, Node, and E2B_API_TOKEN in .private/.env. Makes one machine
 * for about ten minutes and deletes it on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
// A folder of this run's own: two sessions running this at once must not
// stage over each other or stop each other's app (it is found by this path)
const RUN = path.join(os.tmpdir(), 'sk-microvm-resume-' + process.pid);
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const FOLDER = '/home/user/proj';
const CALLS = '/home/user/claude-calls.log';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 60000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const KEY = dotenv.E2B_API_TOKEN;
if (!KEY) die('E2B_API_TOKEN is needed in .private/.env');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

function pathToSdk() {
  const dir = path.join(ROOT, 'target', 'e2b-sdk');
  const entry = path.join(dir, 'node_modules', 'e2b', 'dist', 'index.mjs');
  if (!fs.existsSync(entry)) {
    fs.mkdirSync(dir, { recursive: true });
    spawnSync('npm', ['init', '-y'], { cwd: dir, shell: true });
    spawnSync('npm', ['i', 'e2b', '--silent'], { cwd: dir, shell: true });
  }
  return 'file://' + entry.replace(/\\/g, '/');
}

// Claude Code's own arguments, and where it keeps a conversation's record
const STAND_IN = `#!/bin/sh
echo "$*" >> ${CALLS}
id=""
while [ $# -gt 0 ]; do case "$1" in --session-id|--resume) id="$2"; shift;; esac; shift; done
mkdir -p "$HOME/.claude/projects/-home-user-proj"
touch "$HOME/.claude/projects/-home-user-proj/$id.jsonl"
echo "stand-in claude on $id"
# At work while /home/user/busy is there: something on the screen every few
# seconds, the way an AI's screen moves while it works
(while true; do [ -f /home/user/busy ] && echo "working $(date +%s)"; sleep 5; done) &
while read -r line; do echo "$line" >> /home/user/typed.log; echo "GOT:$line"; done
`;

const { Sandbox } = await import(pathToSdk());
console.log('making a MicroVM');
// Made the way the app makes one: it pauses when its time runs out, and a
// request to it starts it again. The service's client leaves both off
const made = await (await fetch('https://api.e2b.app/sandboxes', {
  method: 'POST', headers: { 'X-API-Key': KEY, 'Content-Type': 'application/json' },
  body: JSON.stringify({ templateID: 'base', timeout: 600, autoPause: true, autoResume: { enabled: true },
    metadata: { shikisha: '1', project: 'resume-check' } }),
})).json();
if (!made.sandboxID) die('no machine: ' + JSON.stringify(made));
const box = await Sandbox.connect(made.sandboxID, { apiKey: KEY });
const inside = async (cmd) => {
  const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
  return (r.stdout + r.stderr).trim();
};
const calls = async () => (await inside(`cat ${CALLS} 2>/dev/null`)).split('\n').filter(Boolean);
const remembered = () => {
  const f = [path.join(APP, 'last-session'), path.join(APP, 'data', 'last-session')].find((p) => fs.existsSync(p));
  if (!f) return null;
  const s = JSON.parse(fs.readFileSync(f, 'utf8'));
  return (s.desks || []).flatMap((d) => d.tabs || []).find((t) => t.id === 'ai') || null;
};
// The window's page, to press its own close button
const boardOf = async () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  let target;
  await until(async () => {
    if (!fs.existsSync(f)) return false;
    const port = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
    target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
    return !!target;
  }, 'the window\'s page', 30000);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let n = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => { const m = JSON.parse(e.data); if (waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); } });
  return (expression) => new Promise((res) => {
    const id = ++n;
    waiting.set(id, (m) => res(m.result && m.result.result ? m.result.result.value : undefined));
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, returnByValue: true } }));
    // A page that does not answer is an answer of nothing, not a run that hangs
    setTimeout(() => res(undefined), 5000);
  });
};
// One of the app's own primitives, called the way an outside client calls
// them: the app's --mcp door, pointed at the copy that is running
const appPid = () => ps('-Command', `(Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}*' } | Select-Object -First 1).Id`).stdout.trim();
const primitive = (name, params) => new Promise((res) => {
  const door = spawn(exe, ['--mcp', '--pid', appPid(), '--token-file', path.join(APP, 'data', 'api-token')], { stdio: ['pipe', 'pipe', 'ignore'] });
  let out = '';
  const done = (v) => { try { door.kill(); } catch {} res(v); };
  door.stdout.on('data', (b) => {
    out += b;
    for (const line of out.split('\n')) {
      try {
        const m = JSON.parse(line);
        if (m.id === 2) return done(m.result ? (m.result.content || []).map((c) => c.text).join('') : 'ERROR ' + JSON.stringify(m.error));
      } catch {}
    }
  });
  const say = (m) => door.stdin.write(JSON.stringify(m) + '\n');
  say({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'check', version: '1' } } });
  say({ jsonrpc: '2.0', method: 'notifications/initialized' });
  say({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'shikisha_' + name, arguments: { params } } });
  setTimeout(() => done('TIMEOUT'), 60000);
});

// The service itself, to pause the machine and to ask what state it is in
const API = 'https://api.e2b.app';
const service = async (method, p) => {
  const r = await fetch(API + p, { method, headers: { 'X-API-Key': KEY } });
  const t = await r.text();
  try { return { status: r.status, body: JSON.parse(t) }; } catch { return { status: r.status, body: t }; }
};
const stateOf = async () => (await service('GET', '/sandboxes/' + box.sandboxId)).body.state;
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const start = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

try {
  await box.files.write('/tmp/claude', STAND_IN);
  await inside('sudo install -m 755 /tmp/claude /usr/local/bin/claude && mkdir -p ' + FOLDER);

  stopApp();
  await sleep(800);
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  const configure = (minutes) => fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'ja',
    remote: { enabled: false },
    // Closing the window quits, rather than going to the notification area
    resident: false,
    // Leaves the key in data/api-token, for the calls below to use
    external_api: { access: 'user' },
    hosts: [{ name: 'vm', kind: 'e2b', template: 'base', minutes }],
    desks: [{ name: 'Check', id: 'check', folders: [{
      cwd: FOLDER, host: 'vm', sandbox: box.sandboxId,
      tabs: [{ name: 'ai', id: 'ai', command: 'claude' }],
    }] }],
  }, null, 2));
  configure(10);
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { e2b_api_key: KEY } }, null, 2));

  console.log('1. the first start: a new conversation under the app\'s id');
  start();
  await until(async () => (await calls()).length >= 1, 'the CLI to start on the machine', 90000);
  const first = (await calls())[0];
  const minted = (first.match(/^--session-id ([0-9a-f-]{36})$/) || [])[1];
  check(!!minted, 'started with a conversation id: ' + first);

  console.log('2. the app writes that id down');
  await until(() => remembered()?.session === minted, 'the id in the last session', 20000)
    .then(() => check(true, 'remembered as the tab\'s conversation'))
    .catch(() => check(false, 'remembered as the tab\'s conversation: ' + JSON.stringify(remembered())));

  console.log('3. restarted, it resumes that conversation there');
  stopApp();
  await sleep(1500);
  start();
  await until(async () => (await calls()).length >= 2, 'the CLI to start again', 90000);
  check((await calls())[1] === '--resume ' + minted, 'resumed: ' + (await calls())[1]);

  console.log('4. restarted with the record gone, a new one under the same id');
  await sleep(4000);
  stopApp();
  await inside('rm -f $HOME/.claude/projects/*/' + minted + '.jsonl');
  await sleep(1500);
  start();
  await until(async () => (await calls()).length >= 3, 'the CLI to start a third time', 90000);
  check((await calls())[2] === '--session-id ' + minted, 'a new conversation under the same id: ' + (await calls())[2]);

  console.log('5. quitting ends its shell on the machine');
  const running = ps('-Command', `(Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}\*' } | Select-Object -First 1).Id`).stdout.trim();
  const ours = async () => (await box.commands.list()).filter((p) => (p.tag || '').startsWith('shikisha-' + running + '-'));
  check((await ours()).length === 1, 'its shell is there while it runs');
  const press = await boardOf();
  // A tab at work makes quitting ask first, in a box of its own: waited out
  await sleep(12000);
  await until(async () => { press('typeof winAct === "function" && winAct("close")'); return true; }, 'the close button');
  await until(() => !ps('-Command', `Get-Process -Id ${running} -ErrorAction SilentlyContinue`).stdout.trim(), 'the app to quit', 30000);
  check((await ours()).length === 0, 'and gone once it quit: ' + JSON.stringify(await ours()));

  console.log('6. paused while the app was closed, and the app started again');
  const paused = await service('POST', '/sandboxes/' + box.sandboxId + '/pause');
  check(paused.status < 300, 'the machine paused: ' + paused.status);
  await until(async () => (await stateOf()) === 'paused', 'the machine to say it is paused', 60000);
  check(true, 'the service says it is paused');
  start();
  await until(async () => (await stateOf()) === 'running', 'the machine to be running again', 90000);
  check(true, 'starting the app brought the machine back');
  await until(async () => (await calls()).length >= 4, 'the CLI to start after the pause', 90000);
  check((await calls())[3] === '--resume ' + minted, 'on its conversation: ' + (await calls())[3]);

  console.log('7. paused while the app is open');
  const board = await boardOf();
  const tabNow = () => board('JSON.stringify({active: S.active, tabs: (S.tabs || []).filter(t => t.id === "ai").map(t => ({index: t.index, state: t.state}))})');
  console.log('    before: ' + await tabNow());
  const typeIn = async (text) => console.log('    send_to_tab: ' + (await primitive('send_to_tab', ['ai', text])).slice(0, 200));
  const arrived = async (text) => (await inside('cat /home/user/typed.log 2>/dev/null')).includes(text);
  await typeIn('before-pause');
  await until(() => arrived('before-pause'), 'what was typed before the pause to arrive', 30000)
    .then(() => check(true, 'typed before the pause, it arrives'))
    .catch(() => check(false, 'typed before the pause, it arrives'));
  await sleep(3000);
  const paused2 = await service('POST', '/sandboxes/' + box.sandboxId + '/pause');
  check(paused2.status < 300, 'the machine paused under the open app: ' + paused2.status);
  await until(async () => (await stateOf()) === 'paused', 'the machine to say it is paused', 60000);
  await sleep(15000);
  console.log('    after the pause: ' + await tabNow() + ' / machine ' + await stateOf());
  await typeIn('after-pause');
  await sleep(20000);
  console.log('    machine after typing: ' + await stateOf());
  await until(() => arrived('after-pause'), 'what was typed to arrive on the machine', 60000)
    .then(() => check(true, 'typed after the pause, it arrives on the machine'))
    .catch(() => check(false, 'typed after the pause, it arrives on the machine'));
  const screenNow = () => primitive('tab_screen', ['ai']);
  await until(() => fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8').includes(' is awake; the shell '), 'the tab to take its shell up again', 30000)
    .catch(() => {});
  await sleep(2000);
  await typeIn('after-listening');
  await until(async () => (await screenNow() || '').includes('GOT:after-listening'), 'the answer on screen', 30000)
    .then(() => check(true, 'and its answer is on the tab\'s screen'))
    .catch(async () => check(false, 'and its answer is on the tab\'s screen: ' + ((await screenNow()) || '').slice(-300)));
  const L = path.join(APP, 'logs', 'hooks.log');
  console.log('    log: ' + fs.readFileSync(L, 'utf8').split(/\r?\n/).slice(-4).join(' | '));

  console.log('8. minutes set to one: up while at work, paused after');
  await sleep(12000);
  await board('typeof winAct === "function" && winAct("close")');
  await until(() => !appPid(), 'the app to quit', 30000);
  configure(1);
  start();
  await until(async () => (await calls()).length >= 5, 'the CLI to start with one minute', 90000);
  await inside('touch /home/user/busy');
  await sleep(150000);
  check((await stateOf()) === 'running', 'at work for two and a half minutes, it is still up: ' + await stateOf());
  await inside('rm -f /home/user/busy');
  const stoppedAt = Date.now();
  const ends = async () => (await service('GET', '/sandboxes/' + box.sandboxId)).body.endAt;
  const watching = setInterval(async () => console.log('    ' + Math.round((Date.now() - stoppedAt) / 1000) + 's: ' + await stateOf() + ', ends ' + await ends()), 30000);
  await until(async () => (await stateOf()) === 'paused', 'the machine to pause once nothing is at work', 240000)
    .then(() => check(true, 'left alone, it paused after ' + Math.round((Date.now() - stoppedAt) / 1000) + 's'))
    .catch(() => check(false, 'left alone, it paused'));
  clearInterval(watching);
  console.log('    e2b log: ' + fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8').split(/\r?\n/).filter((l) => /e2b/.test(l)).slice(-8).join(' | '));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + (e.stack || e));
  console.log('    (on the machine: ' + JSON.stringify((await box.commands.list()).map((p) => ({ pid: p.pid, tag: p.tag, cmd: p.cmd, args: p.args }))) + ')');
} finally {
  stopApp();
  await box.kill().catch(() => {});
  await sleep(500);
  fs.rmSync(RUN, { recursive: true, force: true });
  console.log('machine deleted');
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
