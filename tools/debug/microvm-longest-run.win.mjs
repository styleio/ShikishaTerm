/**
 * A MicroVM kept at work past the longest run the E2B account allows (an hour
 * on the smallest plan), through the running app and a real E2B account.
 *
 * The service counts that run from when a machine was last started, and
 * pauses it at the end however much time it was asked for. Two machines, each
 * with a terminal that writes the time every few seconds for as long as the
 * run lasts:
 *
 *   A  never falls quiet. It pauses at the longest run, and the app has to
 *      start it again at once: the work goes on, with a gap of seconds, and
 *      the terminal says why it blinked
 *   B  falls quiet a few minutes before the end. The app has to pause and
 *      start it itself in that quiet moment, so the run begins again and the
 *      work never meets the longest run at all
 *
 *     cargo build
 *     node tools/debug/microvm-longest-run.win.mjs
 *
 * Needs Windows, Node, and E2B_API_TOKEN in .private/.env. Takes a little over
 * an hour. Makes two small machines for that long (no AI runs on them) and
 * deletes both on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-microvm-longest-' + process.pid);
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const LOG = path.join(APP, 'logs', 'hooks.log');
const API = 'https://api.e2b.app';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const stamp = () => new Date().toISOString().slice(11, 19);
const say = (s) => console.log(`[${stamp()}] ${s}`);

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
const { Sandbox } = await import(pathToSdk());

const service = async (method, p) => {
  const r = await fetch(API + p, { method, headers: { 'X-API-Key': KEY } });
  const t = await r.text();
  try { return { status: r.status, body: JSON.parse(t) }; } catch { return { status: r.status, body: t }; }
};
const windowOf = async (id) => {
  const b = (await service('GET', '/sandboxes/' + id)).body;
  return { state: b.state, started: Date.parse(b.startedAt), ends: Date.parse(b.endAt) };
};

// The work: the time, on screen and into a file, every few seconds -- until
// /home/user/quiet is there, and then nothing more on screen
const WORKER = `#!/bin/sh
while true; do
  if [ ! -f /home/user/quiet ]; then t=$(date +%s); echo "tick $t"; echo "$t" >> /home/user/ticks; fi
  sleep 4
done
`;

async function machine(name) {
  const made = await (await fetch(API + '/sandboxes', {
    method: 'POST', headers: { 'X-API-Key': KEY, 'Content-Type': 'application/json' },
    body: JSON.stringify({ templateID: 'base', timeout: 600, autoPause: true, autoResume: { enabled: true },
      metadata: { shikisha: '1', project: 'longest-run-' + name } }),
  })).json();
  if (!made.sandboxID) die('no machine: ' + JSON.stringify(made));
  const box = await Sandbox.connect(made.sandboxID, { apiKey: KEY });
  await box.files.write('/tmp/worker', WORKER);
  await box.commands.run('sudo install -m 755 /tmp/worker /usr/local/bin/worker', { timeoutMs: 60000 });
  return box;
}
const inside = async (box, cmd) => {
  const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
  return (r.stdout + r.stderr).trim();
};
const ticks = async (box) => (await inside(box, 'cat /home/user/ticks 2>/dev/null')).split('\n').map(Number).filter(Boolean);
const longestGap = (t) => t.slice(1).reduce((g, x, i) => Math.max(g, x - t[i]), 0);
const log = () => (fs.existsSync(LOG) ? fs.readFileSync(LOG, 'utf8') : '');
// One of the app's own primitives, through its --mcp door
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

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;

let a, b;
try {
  say('making two MicroVMs');
  [a, b] = await Promise.all([machine('a'), machine('b')]);
  say(`A ${a.sandboxId}, B ${b.sandboxId}`);

  stopApp();
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed');
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'en',
    remote: { enabled: false },
    resident: false,
    external_api: { access: 'user' },
    // The minutes most people leave it at
    hosts: [{ name: 'vm-a', kind: 'e2b', template: 'base', minutes: 30 }, { name: 'vm-b', kind: 'e2b', template: 'base', minutes: 30 }],
    desks: [{ name: 'Check', id: 'check', folders: [
      { cwd: '/home/user', host: 'vm-a', sandbox: a.sandboxId, tabs: [{ name: 'a', id: 'a', command: 'worker' }] },
      { cwd: '/home/user', host: 'vm-b', sandbox: b.sandboxId, tabs: [{ name: 'b', id: 'b', command: 'worker' }] },
    ] }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { e2b_api_key: KEY } }, null, 2));
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

  // A terminal on a MicroVM opens when it is first looked at or typed
  // into: each is typed into once, so both are open and at work
  await sleep(15000);
  for (const t of ['a', 'b']) say(`typed into ${t}: ` + (await primitive('send_to_tab', [t, '.'])).slice(0, 120));
  const started = async (box) => (await ticks(box)).length > 0;
  for (let i = 0; i < 40 && !((await started(a)) && (await started(b))); i++) await sleep(3000);
  check((await started(a)) && (await started(b)), 'both machines are at work');
  if (process.argv.includes('--smoke')) {
    // A pause made from outside the app while A is at work, and not at the
    // end of its longest run: somebody paused it on purpose. The app leaves
    // it paused -- starting it would start paying for it again -- and says
    // so on its terminal; a key pressed there starts it
    say('pausing A from outside the app');
    await service('POST', '/sandboxes/' + a.sandboxId + '/pause');
    const seen = Date.now() + 120000;
    while (Date.now() < seen && !/not at the end of its longest run; left for a key/.test(log())) await sleep(2000);
    check(/not at the end of its longest run; left for a key/.test(log()), 'A, paused from outside while at work, was seen and left');
    await sleep(15000);
    check((await windowOf(a.sandboxId)).state === 'paused', 'A stays paused: the app did not start it again');
    check(!/paused at its longest run .* started again/.test(log()), 'it was not taken for the end of its longest run');
    const screen = await primitive('tab_screen', ['a']);
    check(/not at the end of the longest run|Type in this terminal/i.test(screen), 'A\'s terminal says what happened and how to go on');
  } else {
  const deadline = Date.now() + 80 * 60 * 1000;
  let quietSaid = false;
  let bBegun = null;
  let aStarted = null;
  while (Date.now() < deadline && !(bBegun && aStarted)) {
    await sleep(30000);
    const [wa, wb] = await Promise.all([windowOf(a.sandboxId), windowOf(b.sandboxId)]);
    const runA = Math.round((Date.now() - wa.started) / 60000);
    const runB = Math.round((Date.now() - wb.started) / 60000);
    say(`A ${wa.state} ${runA} min into its run; B ${wb.state} ${runB} min into its run`);
    // B falls quiet eight minutes before its hour is up
    if (!quietSaid && !bBegun && Date.now() - wb.started > 52 * 60 * 1000) {
      await inside(b, 'touch /home/user/quiet');
      quietSaid = true;
      say('B has fallen quiet');
    }
    if (!bBegun && /was begun again before its longest run/.test(log()) && log().includes(b.sandboxId)) {
      bBegun = Date.now();
      say('B was begun again');
      await inside(b, 'rm -f /home/user/quiet');
    }
    if (!aStarted && /paused at its longest run \(\d+s\) while at work; started again/.test(log())) {
      aStarted = Date.now();
      say('A was started again after its longest run');
    }
  }
  check(!!bBegun, 'B, quiet near the end of its run, was paused and started again by the app');
  if (bBegun) {
    const wb = await windowOf(b.sandboxId);
    check(wb.state === 'running' && wb.started > Date.now() - 20 * 60 * 1000, 'B runs on, its run begun again');
    const tb = await ticks(b);
    check(tb.length > 0 && Date.now() / 1000 - tb[tb.length - 1] < 30, 'B\'s work goes on after it');
  }
  check(!!aStarted, 'A, at work to the end of its run, was started again by the app at once');
  if (aStarted) {
    await sleep(60000);
    const wa = await windowOf(a.sandboxId);
    check(wa.state === 'running', 'A is running again');
    const ta = await ticks(a);
    const gap = longestGap(ta);
    check(Date.now() / 1000 - ta[ta.length - 1] < 30, `A's work goes on (the longest gap in it ${gap}s)`);
    check(gap < 120, 'A stopped for under two minutes');
  }
  }
  const lines = log().split('\n').filter((l) => /e2b: .*(begun again|longest run|could not)/.test(l));
  for (const l of lines) console.log('  ' + l);
} catch (e) {
  failures += 1;
  console.error('stopped: ' + (e.stack || e));
} finally {
  stopApp();
  for (const box of [a, b].filter(Boolean)) await service('DELETE', '/sandboxes/' + box.sandboxId).catch(() => {});
  say('machines deleted');
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
