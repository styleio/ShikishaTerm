/**
 * Whether "Add a project" is answered from a phone.
 *
 * This checkout's build is started in a folder of its own with the relay on,
 * and its board is opened in a headless Chrome standing in for a phone. From
 * there the add-a-project dialog's own asks are sent -- a new project made on
 * this PC, and a clone onto a MicroVM asked about (what it would sign in to
 * the git server as) -- and the answers are read off the page's state,
 * which is what the dialog draws from.
 *
 * The fault it was written for was invisible from the window: the same asks
 * from the window were answered, and from a phone they were refused without a
 * word, so the dialog there said "checking how it signs in..." for good and a
 * press of the button did nothing. No machine is made: the ask about a clone
 * is answered before anything is, and the token is a made-up one of the right
 * shape.
 *
 *   cargo build
 *   node tools/debug/add-project-afar.win.mjs
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-addafar');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const BOARD = 9381;          // the app's relay; 93xx is this folder's band
const WINDOW_CDP = 9382;     // the app's own window: a port of its own
const CHROME_CDP = 9383;     // the stand-in device's own DevTools port
const URL = 'https://github.com/octocat/Hello-World.git';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  die('no Chrome found; set CHROME');
}

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated, with the relay on');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: true, bind: '127.0.0.1', port: BOARD },
  hosts: [{ name: 'microvm', kind: 'e2b', template: 'base', minutes: 30, at: '' }],
  git_accounts: [{ name: 'check', owners: ['octocat'] }],
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));
// A token of the fine-grained shape and nothing behind it: the ask about a
// clone says what kind it is, and never uses it
fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { 'git/check': 'github_pat_0000000000_made_up_for_this_check' } }, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${WINDOW_CDP}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const tokenFile = path.join(APP, 'data', 'remote-token');
let token = '';
for (let i = 0; i < 160 && !token; i++) {
  try { token = fs.readFileSync(tokenFile, 'utf8').trim(); } catch { await sleep(250); }
}
if (!token) { stopApp(); die('the relay never opened -- see ' + path.join(APP, 'logs')); }

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-addafar-'));
const chrome = spawn(findChrome(), [
  '--headless=new',
  '--remote-debugging-port=' + CHROME_CDP,
  '--user-data-dir=' + profile,
  '--window-size=412,915',
  '--no-first-run',
  '--no-default-browser-check',
  'about:blank',
], { stdio: 'ignore' });

// One conversation with the stand-in device's browser. Raw CDP over a
// socket, so nothing here needs a driver installed
async function attach(port = CHROME_CDP) {
  let hit;
  for (let i = 0; i < 80 && !hit; i++) {
    try {
      const list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter((t) => t.type === 'page');
      hit = list[0];
    } catch {}
    if (!hit) await sleep(250);
  }
  if (!hit) die('nothing answered the DevTools port ' + port);
  const ws = new WebSocket(hit.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const call = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  return { call, run, close: () => ws.close() };
}

const until = async (test, what, ms = 25000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};

let dev = null;
try {
  console.log('\n1. the board, as a phone is served it');
  dev = await attach();
  await dev.call('Page.enable');
  await dev.call('Runtime.enable');
  await dev.call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
  await dev.call('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  await dev.call('Page.navigate', { url: `http://127.0.0.1:${BOARD}/?t=${token}` });
  await until(() => dev.run('!!(S && S.tabs && S.tabs.length)'), 'the board');
  check(await dev.run('!OURS'), 'the page knows it is not in the window');
  const answer = () => dev.run('JSON.stringify((S && S.add_project) || null)').then((t) => JSON.parse(t || 'null'));

  console.log('\n2. a new project, made on this PC from the phone');
  await dev.run(`send({kind:"addproject", how:"create", text:"made-from-afar", parent:${JSON.stringify(WORK)}, ask:7, host:"", project:"", ai:"", account:""}); true`);
  await until(async () => ((await answer()) || {}).ask === 7, 'the app\'s answer about the new project', 30000);
  const made = await answer();
  check(!made.error && fs.existsSync(path.join(WORK, 'made-from-afar', '.git')), 'the project is made, not refused: ' + JSON.stringify(made));

  console.log('\n3. a clone onto a MicroVM, asked about before anything is made');
  await dev.run(`send({kind:"addproject", how:"microvm_look", text:${JSON.stringify(URL)}, parent:"", ask:8, host:"microvm", project:"", ai:"claude", account:"check"}); true`);
  await until(async () => { const a = await answer(); return !!(a && a.ask === 8 && a.sign_in); }, 'what it would sign in as', 30000);
  const asked = await answer();
  check(asked.microvm === true && asked.sign_in.account === 'check' && asked.sign_in.kind === 'fine',
    'the phone is told what the MicroVM signs in as: ' + JSON.stringify(asked.sign_in));
  check(!(await dev.run('S.making && S.making.length')), 'and nothing was made by asking');
} catch (e) {
  console.error('\n' + (e && e.stack || e));
  failures += 1;
} finally {
  try { if (dev) dev.close(); } catch {}
  chrome.kill();
  await sleep(500);
  fs.rmSync(profile, { recursive: true, force: true });
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);
