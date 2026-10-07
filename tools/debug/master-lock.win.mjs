/**
 * The master password lock, through the running app's own window and door.
 *
 * This checkout's build in a folder of its own, its secrets sealed with a
 * password before it starts. Each run starts it again and opens the lock a
 * different way:
 *
 *     cargo build
 *     node tools/debug/master-lock.win.mjs [--ja]
 *
 * Checked:
 *   window   the lock is up and nothing else is: no tab, Esc and a press beside
 *            it do not put it away, an empty Enter is not an answer; a wrong
 *            password asks again with another note; the automation pipe turns
 *            every call away
 *   phone    over this machine's own line the door serves the lock page and
 *            nothing else (the board's state and the settings answer 423); a
 *            wrong password is told so, the right one opens the app on the PC
 *            as well, and the board comes back on the phone
 *   again    after a restart, with a password on the board too, the phone
 *            already paired comes back on the bare address: the lock page opens
 *            the link again by itself, asks the board password, then the master
 *   pc       the right password typed on the window opens it; the pipe answers
 *   quit     Quit at the lock ends the app
 *   split    with the screen as a program of its own, the window is handed the
 *            lock page and is opened from it
 *
 * Needs Windows, Node, cargo (to seal the store) and Chrome (the phone).
 * Photographs land in target/shots.
 */
import {findChrome, findCargo, connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-master-lock');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const JA = process.argv.includes('--ja');
const PASSWORD = 'lock-check-password';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const L = JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', (JA ? 'ja' : 'en') + '.json'), 'utf8'));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const pidsOfCopy = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { $_.Id }`)
  .stdout.split(/\r?\n/).map(Number).filter((n) => n > 0);
const stopApp = () => { for (const pid of pidsOfCopy()) spawnSync('taskkill.exe', ['/PID', String(pid), '/T', '/F']); };
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

// The test binary that seals a file (crypto::tests::seal_a_file_for_a_check)
const built = spawnSync(findCargo(), ['test', '-p', 'shikisha-core', '--lib', '--no-run', '--message-format=json'],
  { cwd: ROOT, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
if (built.status !== 0) die(built.stderr);
const sealer = built.stdout.split(/\r?\n/).filter((l) => l.startsWith('{')).map((l) => JSON.parse(l))
  .find((r) => r.reason === 'compiler-artifact' && r.target.name === 'shikisha_core' && r.executable)?.executable;
if (!sealer) die('cargo did not report the test binary');

const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});
const PHONE_KEY = 'masterlockphone0123456789';

console.log('staging this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(SECRETS, JSON.stringify({ notify: {} }));
const sealed = spawnSync(sealer, ['--ignored', '--exact', 'crypto::tests::seal_a_file_for_a_check'],
  { encoding: 'utf8', env: { ...process.env, SHIKISHA_SEAL_FILE: SECRETS, SHIKISHA_SEAL_PW: PASSWORD } });
if (sealed.status !== 0 || !fs.readFileSync(SECRETS, 'utf8').includes('"magic"')) die('sealing failed:\n' + sealed.stdout + sealed.stderr);

const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
async function connect(target, name) {
  const {ws, send, run} = await connectCdp(target);
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `master-lock-${JA ? 'ja-' : ''}${name}-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, send, run, shot };
}

// Start the copy with these settings; the window's page once it is up
// One port for every start: a phone's saved key lives with the address
const PORT = await freePort();
async function start({ split = false, boardPassword = '' } = {}) {
  stopApp();
  // The resident process can outlive one round of taskkill (it is started apart)
  await until(() => { stopApp(); return pidsOfCopy().length === 0; }, 'the last copy gone', 20000);
  fs.rmSync(path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort'), { force: true });
  const port = PORT;
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: JA ? 'ja' : 'en',
    remote: split ? { enabled: false } : { enabled: true, bind: '127.0.0.1', port, sticky_token: true, fixed_token: PHONE_KEY, password: boardPassword },
    split,
    external_api: { access: "user" },
    agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
    desks: [{ name: 'Lock', id: 'lock', folders: [{ cwd: WORK, tabs: [{ name: 'sh', id: 'sh', command: 'cmd.exe' }] }] }],
  }, null, 2));
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  let target;
  await until(async () => {
    const p = portOf('shell');
    return p && (target = (await targetsOf(p)).find((t) => t.type === 'page' && !t.url.startsWith('about:')));
  }, 'the window\'s page', 40000);
  return { port, board: await connect(target, split ? 'split' : 'window') };
}

// The pipe, as an automation would call it: every copy's process is tried,
// since which one holds the pipe depends on how the program is split
const tokenFile = () => [path.join(APP, 'data', 'api-token'), path.join(LOCAL, 'ShikishaTerm', 'data', 'api-token')].find((p) => fs.existsSync(p));
async function pipeCall(method, ...params) {
  for (const pid of pidsOfCopy()) {
    const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
    const opened = await new Promise((r) => { sock.once('connect', () => r(true)); sock.once('error', () => r(false)); });
    if (!opened) continue;
    let buf = '';
    const waiting = [];
    sock.on('data', (d) => {
      buf += d.toString('utf8');
      let i;
      while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); }
    });
    const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
    const hello = await line({ token: fs.readFileSync(tokenFile(), 'utf8').trim() });
    if (!hello.ok) { sock.destroy(); return { ok: false, error: 'the door refused the key' }; }
    const a = await line({ id: '1', method, params });
    sock.destroy();
    return a;
  }
  return { ok: false, error: 'no pipe' };
}

// The window's lock, as it stands
const lockState = (board) => board.run(`JSON.stringify((() => {
  const v = document.getElementById("veil"), box = v && v.querySelector(".box");
  return { up: !!v && !v.hidden && !!box, foot: !!(box && box.querySelector(".lockfoot")),
    note: box ? [...box.querySelectorAll(".row")].map(r => r.textContent).join(" ") : "",
    tabs: (typeof S !== "undefined" && S && S.tabs) ? S.tabs.length : 0 };
})())`).then(JSON.parse);
const typeIn = (board, text, key = 'Enter') => board.run(`(() => { const i = document.querySelector("#veil input");
  i.value = ${JSON.stringify(text)}; i.dispatchEvent(new KeyboardEvent("keydown", {key:${JSON.stringify(key)}, bubbles:true})); return true; })()`);

let chrome = null;
try {
  console.log('1. the window, locked');
  let { port, board } = await start();
  await until(async () => (await lockState(board)).up, 'the lock on the window', 30000);
  let s = await lockState(board);
  check(s.foot && s.note === L['prompt.password.note'], 'the lock is up, with Quit and Unlock: ' + JSON.stringify(s));
  await board.shot('1-locked');
  await typeIn(board, 'typed', 'Escape');
  await board.run('(() => { const v = document.getElementById("veil"); v.dispatchEvent(new MouseEvent("mousedown", {bubbles:true})); return true; })()');
  await typeIn(board, '');
  await sleep(1500);
  s = await lockState(board);
  check(s.up, 'Esc, a press beside it and an empty Enter leave it up');
  check(s.tabs === 0, 'no tab is on the board behind it');
  let a = await pipeCall('state', 'sh');
  check(!a.ok && /locked/.test(a.error || ''), 'the pipe turns a call away: ' + JSON.stringify(a));
  await typeIn(board, 'not the password');
  await until(async () => (await lockState(board)).note === L['prompt.password.retry'], 'asked again', 15000).catch(() => {});
  s = await lockState(board);
  check(s.up && s.note === L['prompt.password.retry'], 'a wrong password asks again with another note: ' + s.note);

  console.log('2. the phone opens it');
  const base = `http://127.0.0.1:${port}`;
  const r0 = await fetch(`${base}/?t=${PHONE_KEY}`, { redirect: 'manual' });
  const cookies = r0.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
  const get = (p) => fetch(base + p, { headers: { Cookie: cookies }, redirect: 'manual' });
  check((await get('/api/state')).status === 423, 'the board\'s state answers 423');
  check((await get('/cfg')).status === 423, 'the settings answer 423');
  const page = await (await get('/')).text();
  check(page.includes('api/unlock') && !page.includes('const AT_PC'), 'the door serves the lock page, not the board');
  const phoneDir = path.join(RUN, 'phone');
  fs.mkdirSync(phoneDir, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + phoneDir,
    '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(phoneDir, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const pport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let pt;
  await until(async () => (pt = (await targetsOf(pport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connect(pt, 'phone');
  await phone.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await phone.send('Page.navigate', { url: `${base}/?t=${PHONE_KEY}` });
  await until(() => phone.run('!!document.getElementById("pw")'), 'the lock page on the phone', 20000);
  await phone.shot('2-locked');
  const press = (pw) => phone.run(`(() => { document.getElementById("pw").value = ${JSON.stringify(pw)}; document.getElementById("go").click(); return true; })()`);
  await press('still not it');
  await until(() => phone.run('document.getElementById("said").textContent.length > 0'), 'the phone told', 15000).catch(() => {});
  const told = await phone.run('document.getElementById("said").textContent');
  check(told === L['page.lock.wrong'], 'a wrong password is told so: ' + told);
  await phone.shot('2-wrong');
  check((await lockState(board)).up, 'the window stays locked meanwhile');
  await press(PASSWORD);
  await until(async () => !(await lockState(board)).up, 'the window opened', 20000).catch(() => {});
  await until(async () => (await lockState(board)).tabs > 0, 'the tab on the window', 20000).catch(() => {});
  s = await lockState(board);
  check(!s.up && s.tabs > 0, 'the right password opens the window too, and the tab comes up: ' + JSON.stringify(s));
  await until(() => phone.run('typeof S !== "undefined" && !!S && S.tabs.length > 0'), 'the board on the phone', 20000).catch(() => {});
  check(await phone.run('typeof S !== "undefined" && !!S && S.tabs.length > 0'), 'the board comes back on the phone');
  await phone.shot('2-opened');
  await board.shot('2-opened');
  check((await get('/api/state')).status === 200, 'the board\'s state is handed out again');
  a = await pipeCall('state', 'sh');
  check(a.ok, 'the pipe answers again: ' + JSON.stringify(a).slice(0, 120));

  console.log('2b. after a restart, the phone already paired opens it');
  // The app starts again: every session it gave is gone, and the board has a
  // password of its own as well. The phone comes back on the bare address
  ({ board } = await start({ boardPassword: 'aikotoba' }));
  await until(async () => (await lockState(board)).up, 'the lock on the window', 30000);
  await phone.send('Page.navigate', { url: `${base}/` });
  await until(() => phone.run('!!document.getElementById("pw") && location.search.includes("t=")'), 'the link opened again on its own', 20000).catch(() => {});
  check(await phone.run('location.search.includes("t=")'), 'with no session, the lock page opens the link of the board again by itself');
  await sleep(1500);
  check(await phone.run('document.getElementById("said").textContent === "" && !document.getElementById("pw").disabled'), 'and is ready to be typed in: ' + await phone.run('document.getElementById("said").textContent'));
  const lbl = () => phone.run('document.getElementById("lbl").textContent');
  await press(PASSWORD);
  await until(async () => (await lbl()) === L['page.lock.board_password'], 'asked for the password of the board', 15000).catch(() => {});
  check((await lbl()) === L['page.lock.board_password'], 'the password of the board itself is asked first: ' + await lbl());
  await press('aikotoba');
  await until(async () => (await lbl()) === L['page.lock.field'], 'back to the master password', 15000).catch(() => {});
  await press(PASSWORD);
  await until(async () => !(await lockState(board)).up, 'the window opened', 20000).catch(() => {});
  check(!(await lockState(board)).up, 'then the master password opens the app');
  await until(() => phone.run('typeof S !== "undefined" && !!S && S.tabs.length > 0'), 'the board on the phone', 20000).catch(() => {});
  check(await phone.run('typeof S !== "undefined" && !!S && S.tabs.length > 0').catch(() => false), 'and the board comes back on the phone');
  await phone.shot('2b-opened');

  console.log('3. the window opens it');
  ({ port, board } = await start());
  await until(async () => (await lockState(board)).up, 'the lock on the window', 30000);
  await typeIn(board, PASSWORD);
  await until(async () => { const t = await lockState(board); return !t.up && t.tabs > 0; }, 'opened', 20000).catch(() => {});
  s = await lockState(board);
  check(!s.up && s.tabs > 0, 'the right password typed on the window opens it: ' + JSON.stringify(s));
  a = await pipeCall('state', 'sh');
  check(a.ok, 'the pipe answers');

  console.log('4. quit at the lock');
  ({ port, board } = await start());
  await until(async () => (await lockState(board)).up, 'the lock on the window', 30000);
  await board.run('(() => { document.querySelector("#veil .lockfoot .quiet").click(); return true; })()').catch(() => {});
  await until(() => pidsOfCopy().length === 0, 'the app gone', 20000).catch(() => {});
  check(pidsOfCopy().length === 0, 'Quit ends the app: ' + pidsOfCopy().join(','));

  console.log('5. split in two');
  ({ board } = await start({ split: true }));
  await until(() => board.run('!!document.getElementById("pw")'), 'the lock page in the window', 40000);
  check(await board.run('!document.querySelector(".warn")'), 'the window, on this machine\'s own line, is given the field');
  await board.shot('5-locked');
  const pressIn = (pw) => board.run(`(() => { document.getElementById("pw").value = ${JSON.stringify(pw)}; document.getElementById("go").click(); return true; })()`);
  await pressIn('nor this');
  await until(() => board.run('document.getElementById("said").textContent.length > 0'), 'the window told', 15000).catch(() => {});
  const splitTold = await board.run('document.getElementById("said").textContent');
  check(splitTold === L['page.lock.wrong'], 'a wrong password is told so: ' + splitTold);
  await pressIn(PASSWORD);
  await until(() => board.run('typeof S !== "undefined" && !!S && S.tabs.length > 0'), 'the board in the window', 30000).catch(() => {});
  check(await board.run('typeof S !== "undefined" && !!S && S.tabs.length > 0').catch(() => false), 'opened from it, the window shows the board');
  await board.shot('5-opened');
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (chrome) chrome.kill();
  stopApp();
}
console.log(failures ? `${failures} FAILED` : 'all passed');
process.exit(failures ? 1 : 0);
