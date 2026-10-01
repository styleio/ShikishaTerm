/**
 * A tab's terminal held by the bridge on a MicroVM, through the running app
 * and a real E2B account (far-keep plan §4.4, stage 3).
 *
 * The terminal is opened by the bridge's resident process, not by the app;
 * the app's line to it is cut in the middle of a full-screen program, and
 * the app attaches again when its line comes back:
 *
 *   1. with the bridge connected, the tab opens its terminal in the bridge
 *   2. what is typed arrives, and what the shell says comes back
 *   3. a full-screen program (the alternate screen) is up when the line goes
 *   4. another app's line holds the resident process meanwhile (with none, it
 *      ends a few seconds after the last app went: the away mode that keeps
 *      terminals comes in stage 6)
 *   5. the line comes back, the tab attaches again, and the full screen is
 *      there as it was; typing goes on; leaving the full screen brings back
 *      what was on the normal screen under it
 *   6. the shikisha command in the terminal runs the resident process's build
 *   7-9. the app is killed and started again (far-keep plan §7.5, stage 4):
 *      the tab goes back to the terminal it left running, starting nothing
 *      new; one that ended meanwhile is started again in a new terminal; and,
 *      the resident process killed, one whose session still has a process
 *      in it starts nothing until the tab is restarted, while one with
 *      nothing left starts again
 *   10-11. quitting (far-keep plan §7.1, stage 5): asked, with the AI set to
 *      go on, whether to leave it running -- left running, it goes on, a
 *      call it makes meanwhile is told the PC is away and is written down,
 *      and the next start goes back to it and says so; stopped, it ends
 *
 * The machine is set to keep its AIs for 30 minutes while the app is away.
 *
 *     cargo build   (and a Linux bridge in bridge/, see bridge-far.win.mjs)
 *     node tools/debug/far-hold.win.mjs
 *
 * The machine's entry chooses what its AIs do while the app is away, which is
 * what has its terminals held by the bridge. Needs E2B_API_TOKEN in
 * .private/.env. One machine for a few minutes, deleted on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-far-hold-' + process.pid);
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const LOG = path.join(APP, 'logs', 'hooks.log');
const BRIDGE_DIR = '/home/user/.local/share/shikisha/bridge';
// Made-up builds put there first: unheld and marked, held, and from before the marks
const FAKE = ['0.0.1-aaaa', '0.0.2-bbbb', '0.0.3'];

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
const log = () => (fs.existsSync(LOG) ? fs.readFileSync(LOG, 'utf8') : '');

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const KEY = dotenv.E2B_API_TOKEN;
if (!KEY) die('E2B_API_TOKEN is needed in .private/.env');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (!fs.existsSync(path.join(ROOT, 'bridge', 'shikisha-bridge-x86_64-linux'))) die('no Linux bridge in bridge/ -- build one first');

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
const { Sandbox } = await import(pathToSdk());

// The app's own primitives, through its --mcp door
const appPid = () => ps('-Command', `(Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}*' } | Select-Object -First 1).Id`).stdout.trim();
const primitive = (name, params) => new Promise((res) => {
  const door = spawn(exe, ['--mcp', '--pid', appPid(), '--token-file', path.join(APP, 'data', 'api-token')], { stdio: ['pipe', 'pipe', 'ignore'] });
  let out = '';
  const done = (v) => { try { door.kill(); } catch {} res(v); };
  door.stdout.on('data', (d) => {
    out += d;
    for (const line of out.split('\n')) {
      try { const m = JSON.parse(line); if (m.id === 2) return done(m.result ? (m.result.content || []).map((c) => c.text).join('') : 'ERROR ' + JSON.stringify(m.error)); } catch {}
    }
  });
  const tell = (m) => door.stdin.write(JSON.stringify(m) + '\n');
  tell({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'check', version: '1' } } });
  tell({ jsonrpc: '2.0', method: 'notifications/initialized' });
  tell({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'shikisha_' + name, arguments: { params } } });
  setTimeout(() => done('TIMEOUT'), 60000);
});
const screen = async () => String(await primitive('tab_screen', ['held']));

let box;
try {
  const made = await (await fetch('https://api.e2b.app/sandboxes', {
    method: 'POST', headers: { 'X-API-Key': KEY, 'Content-Type': 'application/json' },
    body: JSON.stringify({ templateID: 'base', timeout: 1800, autoPause: true, autoResume: { enabled: true },
      metadata: { shikisha: '1', project: 'far-hold' } }),
  })).json();
  if (!made.sandboxID) die('no machine: ' + JSON.stringify(made));
  box = await Sandbox.connect(made.sandboxID, { apiKey: KEY });
  const inside = async (cmd) => {
    const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
    return (r.stdout + r.stderr).trim();
  };
  console.log('machine ' + box.sandboxId);
  // An AI tab is what the bridge is put there for: a stand-in `claude` that
  // is a shell, so the tab is taken for an AI and still does what is typed
  await box.files.write('/tmp/claude', '#!/bin/sh\nexec bash --norc\n');
  await inside('sudo install -m 755 /tmp/claude /usr/local/bin/claude');
  // Older builds already there: one nobody runs (to be cleared), one a
  // process holds (to stay), and one from before the marks (to stay)
  await inside(`mkdir -p ${BRIDGE_DIR} && cd ${BRIDGE_DIR} && for b in ${FAKE.join(' ')}; do printf x > shikisha-bridge-$b; done && touch .shikisha-bridge-0.0.1-aaaa.lock .shikisha-bridge-0.0.2-bbbb.lock`);
  await box.commands.run(`cd ${BRIDGE_DIR} && flock -s .shikisha-bridge-0.0.2-bbbb.lock sleep 900`, { background: true });

  stopApp();
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed');
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'en', remote: { enabled: false }, resident: false, external_api: { access: 'user' },
    hosts: [{ name: 'vm', kind: 'e2b', template: 'base', minutes: 15, away: { minutes: 30 } }],
    bridges: ['vm'],
    desks: [{ name: 'Far', id: 'far', folders: [
      { cwd: '/home/user', host: 'vm', sandbox: box.sandboxId, tabs: [{ name: 'held', id: 'held', command: 'claude' }] },
    ] }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { e2b_api_key: KEY } }, null, 2));
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
  Object.assign(env, { LOCALAPPDATA: LOCAL });
  const startApp = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  startApp();
  // Killed, as a crash or a power cut would, and started again: what the log
  // says from then on
  const restart = async () => {
    stopApp();
    await sleep(2000);
    const from = log().length;
    fs.rmSync(path.join(APP, 'data', 'api-token'), { force: true });
    startApp();
    await until(() => fs.existsSync(path.join(APP, 'data', 'api-token')), 'the app', 60000);
    await sleep(3000);
    await primitive('show', ['held']).catch(() => {});
    return () => log().slice(from);
  };
  const residentPid = async () => (await inside(`pgrep -f '${BRIDGE_DIR}/shikisha-bridge-[^ ]* daemon' | head -1`)).trim();
  const bridgeLines = (text) => text.split('\n').filter((l) => /far terminal|bridge/.test(l)).join(' | ');

  console.log('0. putting the bridge there clears only the builds nobody runs');
  const cleared = async () => (await inside(`ls -a ${BRIDGE_DIR}`)).split('\n');
  await until(() => /bridge: connected to/.test(log()), 'the bridge to connect', 240000).catch(() => {});
  const left = await cleared();
  check(!left.includes('shikisha-bridge-0.0.1-aaaa'), 'a build nobody runs was cleared');
  check(left.includes('shikisha-bridge-0.0.2-bbbb'), 'a build in use stayed');
  check(left.includes('shikisha-bridge-0.0.3'), 'a build from before the marks stayed');
  check(left.some((n) => /^\.shikisha-bridge-.*\.lock$/.test(n) && !FAKE.some((b) => n === `.shikisha-bridge-${b}.lock`)), 'the build running now holds its mark: ' + left.filter((n) => n.startsWith('.')).join(' '));

  console.log('1. with the bridge connected, the tab opens its terminal in the bridge');
  await until(() => fs.existsSync(path.join(APP, 'data', 'api-token')), 'the app', 60000);
  // The terminal is opened on the machine when the tab is first shown
  await sleep(3000);
  await primitive('show', ['held']).catch(() => {});
  await until(() => /bridge: connected to/.test(log()), 'the bridge to connect', 240000);
  // Started before the bridge was up, the tab waited for it rather than
  // starting the old way, and opened its terminal there once it was
  await until(() => /far terminal \d+ opened on vm once its line was up/.test(log()), 'the terminal to open in the bridge', 60000)
    .then(() => check(/far terminal for held: to be opened on vm once its line is up/.test(log()), 'the tab waited for the bridge, and its terminal was opened there'))
    .catch(() => check(false, 'the tab waited for the bridge, and its terminal was opened there: ' + log().split('\n').filter((l) => /far terminal|bridge/.test(l)).join(' | ')));

  console.log('2. typing arrives, and what the shell says comes back');
  await sleep(3000);
  await primitive('send_to_tab', ['held', 'echo held-$((6*7))']);
  await until(async () => (await screen()).includes('held-42'), 'the answer', 30000)
    .then(() => check(true, 'typed and answered'))
    .catch(async () => check(false, 'typed and answered: ' + (await screen()).slice(-300)));

  console.log('3. a full-screen program is up');
  await primitive('send_to_tab', ['held', "printf '\\033[?1049h\\033[2J\\033[5;5Hfullscreen-mark'; sleep 900"]);
  await until(async () => (await screen()).includes('fullscreen-mark'), 'the full screen', 30000)
    .then(() => check(true, 'the full screen is up'))
    .catch(async () => check(false, 'the full screen is up: ' + (await screen()).slice(-300)));

  console.log('4. another app holds the resident process; the app\'s line is cut');
  // The build this app put there: not one of the made-up builds from step 0
  const program = (await inside(`ls ${BRIDGE_DIR}`)).split('\n')
    .find((n) => n.startsWith('shikisha-bridge-') && !FAKE.includes(n.slice('shikisha-bridge-'.length)) && !n.endsWith('.part'));
  // The other app is of another build: its door, a program of another name,
  // carries its line to the resident process this app's build started.
  // Marked in its environment, so the cut below leaves it alone
  const other = 'shikisha-bridge-0.0.9-otherbuild';
  await inside(`cp ${BRIDGE_DIR}/${program} ${BRIDGE_DIR}/${other}`);
  await box.commands.run(`(while true; do echo '{"t":"tick"}'; sleep 5; done) | OTHER_APP=1 ${BRIDGE_DIR}/${other} serve > /tmp/other-app.out 2>&1`, { background: true });
  await sleep(4000);
  const otherSaid = await inside('head -c 400 /tmp/other-app.out');
  check(otherSaid.includes('"t":"hello"') && otherSaid.includes(program), 'another app of another build is on the same resident process: ' + otherSaid.slice(0, 200));
  check((await inside(`ls -a ${BRIDGE_DIR}`)).split('\n').includes(`.${other}.lock`), 'the other build\'s door holds its mark');
  const before = log().length;
  const killed = await inside(`for p in $(pgrep -f '^${BRIDGE_DIR}/${program} serve'); do tr '\\0' '\\n' < /proc/$p/environ | grep -qx OTHER_APP=1 || { kill $p; echo killed $p; }; done`);
  check(/killed/.test(killed), 'the app\'s line was cut: ' + killed);

  console.log('5. the line comes back and the tab attaches again');
  await until(() => /attaching again/.test(log().slice(before)), 'the tab to attach again', 240000)
    .then(() => check(true, 'the tab attached again'))
    .catch(() => check(false, 'the tab attached again: ' + log().slice(before).split('\n').filter((l) => /far terminal|bridge/.test(l)).join(' | ')));
  await sleep(3000);
  const back = await screen();
  check(back.includes('fullscreen-mark') && !back.includes('held-42'), 'the full screen is there as it was');
  await primitive('send_to_tab', ['held', '\x03']);
  await sleep(1000);
  await primitive('send_to_tab', ['held', "printf '\\033[?1049l'; echo after-$((2+3))"]);
  await until(async () => (await screen()).includes('after-5'), 'typing after it', 30000)
    .then(() => check(true, 'typing goes on after it'))
    .catch(async () => check(false, 'typing goes on after it: ' + (await screen()).slice(-300)));
  const normal = await screen();
  // What the full screen drew is a line of that word alone; the command
  // typed to draw it, which the normal screen shows, has more on its line
  const drawn = (text) => text.split('\n').some((l) => l.trim() === 'fullscreen-mark');
  check(normal.includes('held-42') && !drawn(normal), 'leaving the full screen brings back the normal screen under it' + (normal.includes('held-42') ? '' : ': ' + JSON.stringify(normal.split('\n').filter((l) => l.trim()).slice(-12))));
  check(!/not known there any more/.test(log()), 'the terminal was never taken for gone');

  console.log('6. the shikisha command in the terminal runs the resident process\'s own build');
  await primitive('send_to_tab', ['held', 'echo "prog=$SHIKISHA_BRIDGE_PROGRAM"']);
  await until(async () => (await screen()).includes(`prog=${BRIDGE_DIR}/${program}`), 'the program in the terminal', 30000)
    .then(() => check(true, 'the terminal names the resident process\'s build'))
    .catch(async () => check(false, 'the terminal names the resident process\'s build: ' + (await screen()).slice(-300)));

  console.log('7. the app is killed and started again: the tab goes back to the terminal it left running');
  await primitive('send_to_tab', ['held', 'echo before-restart-$((7*7))']);
  await until(async () => (await screen()).includes('before-restart-49'), 'the mark before the restart', 30000);
  let since = await restart();
  await until(() => /went back to it/.test(since()), 'the tab to go back to its terminal', 240000)
    .then(() => check(true, 'the tab went back to the terminal it left running'))
    .catch(() => check(false, 'the tab went back to the terminal it left running: ' + bridgeLines(since())));
  check(!/far terminal \d+ opened on/.test(since()), 'nothing new was started');
  await sleep(2000);
  check((await screen()).includes('before-restart-49'), 'what was on its screen is there');
  await primitive('send_to_tab', ['held', 'echo after-restart-$((8*8))']);
  await until(async () => (await screen()).includes('after-restart-64'), 'typing after the restart', 30000)
    .then(() => check(true, 'typing goes on after the restart'))
    .catch(async () => check(false, 'typing goes on after the restart: ' + (await screen()).slice(-300)));

  console.log('8. the terminal ends while the app is away: the tab starts again in a new one');
  stopApp();
  await sleep(2000);
  await inside(`pkill -9 -P ${await residentPid()}`);
  since = await restart();
  await until(() => /ended while away; opened \d+ in its place/.test(since()), 'a new terminal in its place', 240000)
    .then(() => check(true, 'a terminal that ended meanwhile is started again in a new one'))
    .catch(() => check(false, 'a terminal that ended meanwhile is started again in a new one: ' + bridgeLines(since())));
  await sleep(3000);
  await primitive('send_to_tab', ['held', 'echo fresh-$((3*3))']);
  await until(async () => (await screen()).includes('fresh-9'), 'the new terminal answering', 30000)
    .then(() => check(true, 'the new terminal answers'))
    .catch(async () => check(false, 'the new terminal answers: ' + (await screen()).slice(-300)));

  console.log('9. the resident process is killed: what ran in it is looked for, and its AI started again only once it is gone');
  // Something of the terminal's session that outlives the resident process:
  // whether the AI is gone cannot be told, and nothing is started
  await primitive('send_to_tab', ['held', 'nohup sleep 900 >/dev/null 2>&1 & echo outlives-$((9*9))']);
  await until(async () => (await screen()).includes('outlives-81'), 'the process that outlives it', 30000);
  stopApp();
  await sleep(2000);
  await inside(`kill -9 ${await residentPid()}`);
  since = await restart();
  await until(() => /not known there any more/.test(since()), 'the answer that it is not known', 240000)
    .then(() => check(true, 'the bridge said it does not know the terminal'))
    .catch(() => check(false, 'the bridge said it does not know the terminal: ' + bridgeLines(since())));
  await sleep(3000);
  check(!/far terminal \d+ opened on/.test(since()), 'nothing new was started by itself');
  // Read as one line: the words are wrapped at the tab's width
  const told = (await screen()).replace(/\s+/g, ' ');
  check(told.includes('is not known'), 'the tab says so' + (told.includes('is not known') ? '' : ': ' + told.slice(-400)));
  // Started by the person: in the bridge when its line is up, and the way a
  // tab of a machine whose terminals are not held is when it is not (a new
  // tab waits for the line only once the away mode can be chosen, stage 6)
  const restartMark = log().length;
  await primitive('restart', ['held']);
  // It waits for its line, which is made again at once now it is wanted
  await until(() => /far terminal \d+ opened on/.test(log().slice(restartMark)), 'the restarted tab to open', 60000).catch(() => {});
  await sleep(3000);
  await primitive('send_to_tab', ['held', 'echo restarted-$((4*4))']);
  await until(async () => (await screen()).includes('restarted-16'), 'the restarted tab answering', 60000)
    .then(() => check(true, 'restarting the tab starts a new one'))
    .catch(async () => check(false, 'restarting the tab starts a new one: ' + bridgeLines(since()) + ' / ' + (await screen()).slice(-300)));
  await inside('pkill -f "sleep 900"');

  // Nothing of it outlives the resident process: its end is known, and the
  // tab starts again where its conversation was
  {
    let held = false;
    for (let i = 0; i < 12 && !held; i++) {
      const mark = log().length;
      await primitive('restart', ['held']);
      held = await until(() => /far terminal \d+ opened on/.test(log().slice(mark)), 'a held terminal', 15000).then(() => true).catch(() => false);
      if (!held) await sleep(10000);
    }
    stopApp();
    await sleep(2000);
    await inside(`kill -9 ${await residentPid()}`);
    since = await restart();
    await until(() => /it ended while away; opened \d+ in its place/.test(since()), 'a new terminal in its place', 240000)
      .then(() => check(true, 'one whose resident process ended with nothing of it left is started again'))
      .catch(() => check(false, 'one whose resident process ended with nothing of it left is started again: ' + bridgeLines(since())));
  }

  const quitApp = (answer) => ps('-File', path.join(ROOT, 'tools', 'debug', 'lib', 'quit-app.ps1'), '-Root', APP, '-Answer', answer).stdout.trim();
  const appGone = () => until(() => appPid() === '', 'the app to quit', 60000);
  // The processes the resident process holds terminals in: none once it has ended
  const children = async () => (await inside(`p=$(pgrep -f '${BRIDGE_DIR}/shikisha-bridge-[^ ]* daemon' | head -1); if [ -n "$p" ]; then pgrep -P "$p" | wc -l; else echo 0; fi`)).trim();

  console.log('10. quitting, the AI left running: it goes on, and a call it makes meanwhile is written down');
  // The tab's terminal held by the bridge again, now that its line is up
  // (restarted until its line is up when it starts: a tab started without it
  // opens the old way)
  let held = false;
  for (let i = 0; i < 12 && !held; i++) {
    const mark = log().length;
    await primitive('restart', ['held']);
    held = await until(() => /far terminal \d+ opened on/.test(log().slice(mark)), 'a held terminal', 15000).then(() => true).catch(() => false);
    if (!held) await sleep(10000);
  }
  if (!held) throw new Error('the tab did not get a held terminal again');
  await sleep(3000);
  await primitive('send_to_tab', ['held', '(sleep 20; shikisha tab_list > /tmp/away.out 2>&1) & echo armed-$((5*5))']);
  await until(async () => (await screen()).includes('armed-25'), 'the call to be set', 30000);
  let said = quitApp('Yes');
  check(/answered Yes/.test(said) && /go on running/.test(said), 'quitting asks whether to leave the AI running: ' + said);
  await appGone().then(() => check(true, 'the app quit')).catch(() => check(false, 'the app quit'));
  await sleep(3000);
  check(Number(await children()) > 0, 'the AI goes on after the app quit');
  await sleep(25000);
  const awayOut = await inside('cat /tmp/away.out');
  check(awayOut.includes('PC is away'), 'a call made meanwhile is told the PC is away: ' + awayOut.slice(0, 200));
  since = await restart();
  await until(() => /went back to it/.test(since()), 'the tab to go back to it', 240000)
    .then(() => check(true, 'the next start goes back to the AI left running'))
    .catch(() => check(false, 'the next start goes back to the AI left running: ' + bridgeLines(since())));
  await until(() => /did not get through while this app was away/.test(since()), 'the calls that did not get through', 60000)
    .then(() => check(true, 'the call made while away is told about'))
    .catch(() => check(false, 'the call made while away is told about: ' + bridgeLines(since())));

  console.log('11. quitting with every AI stopped: it ends');
  await sleep(3000);
  said = quitApp('No');
  check(/answered No/.test(said), 'quitting asked, answered to stop them all: ' + said);
  await appGone().then(() => check(true, 'the app quit')).catch(() => check(false, 'the app quit'));
  await until(async () => Number(await children()) === 0, 'the AI to stop', 30000)
    .then(() => check(true, 'the AI was stopped'))
    .catch(async () => check(false, 'the AI was stopped: ' + (await children()) + ' left'));
} catch (e) {
  failures += 1;
  console.error('stopped: ' + (e.stack || e));
} finally {
  stopApp();
  if (box) await fetch('https://api.e2b.app/sandboxes/' + box.sandboxId, { method: 'DELETE', headers: { 'X-API-Key': KEY } }).catch(() => {});
  console.log('machine deleted');
}
const hold = log().split('\n').filter((l) => /far terminal|bridge: /.test(l));
for (const l of hold.slice(-12)) console.log('  ' + l);
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
