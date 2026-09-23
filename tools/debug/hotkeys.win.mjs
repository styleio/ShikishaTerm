/**
 * The keys that work from any program, pressed for real.
 *
 * They are registered with Windows, so a press sent to the page through the
 * DevTools protocol never reaches them: only a key going through the system's
 * own input does. This starts the app built in this checkout, in a folder of
 * its own, and presses the quick commands' and the ideas' keys the way a
 * keyboard does -- with the window minimised, in front, and put away in the
 * notification area -- and checks the window came forward and what opened.
 *
 *     cargo build
 *     node tools/debug/hotkeys.win.mjs
 *
 * Needs Windows and Node. It takes the front of the screen while it runs:
 * bringing the window forward is what is being checked. The mouse is not
 * touched. Isolated like the other checks here; the app is stopped on the way
 * out. If another copy of the app is running with these keys, Windows gives
 * them to that copy, and this says so and stops.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9351;
const RUN = path.join(os.tmpdir(), 'sk-hotkeys');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');

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

// The system's side: pressing keys through its input, and asking which
// program has the front and whether the app's window is minimised
const WIN32 = `
Add-Type -Namespace SkCheck -Name U -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, System.UIntPtr extra);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int cmd);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool PostMessageW(System.IntPtr h, uint msg, System.IntPtr w, System.IntPtr l);
'@
$app = Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}\\*' } | Select-Object -First 1
`;
const win32 = (body) => {
  const r = ps('-Command', WIN32 + body);
  return (r.stdout || '').trim();
};
/** Alt+Shift and a letter, down and up the way fingers do it */
const press = (letter) => win32(`
$k = [byte][char]'${letter.toUpperCase()}'
[SkCheck.U]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
[SkCheck.U]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)
[SkCheck.U]::keybd_event($k, 0, 0, [UIntPtr]::Zero)
[SkCheck.U]::keybd_event($k, 0, 2, [UIntPtr]::Zero)
[SkCheck.U]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero)
[SkCheck.U]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
`);
const inFront = () => win32(`
$pid2 = 0; [void][SkCheck.U]::GetWindowThreadProcessId([SkCheck.U]::GetForegroundWindow(), [ref]$pid2)
if ($pid2 -eq $app.Id) { 'yes' } else { 'no' }`) === 'yes';
// The window itself, taken once while it is showing: a window put away is
// no longer the one the system calls the program's main window
let hwnd = '0';
const minimise = () => win32(`[void][SkCheck.U]::ShowWindow([IntPtr]${hwnd}, 6)`);
const minimised = () => win32(`if ([SkCheck.U]::IsIconic([IntPtr]${hwnd})) { 'yes' } else { 'no' }`) === 'yes';
const visible = () => win32(`if ([SkCheck.U]::IsWindowVisible([IntPtr]${hwnd})) { 'yes' } else { 'no' }`) === 'yes';
// The window's own ✕, as the system sends it: puts the window away
const closeBox = () => win32(`[void][SkCheck.U]::PostMessageW([IntPtr]${hwnd}, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

// The board's page, found again whenever it is made again: a window put away
// drops its page, and the one it comes back with is a new DevTools target
let ws = null;
let id = 0;
const waiting = new Map();
const connect = async () => {
  if (ws) { try { ws.close(); } catch {} }
  ws = null;
  for (let i = 0; i < 120 && !ws; i++) {
    try {
      const pages = (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json())
        .filter((t) => t.type === 'page' && /^http:\/\/127\.0\.0\.1:\d+\/(\?|$)/.test(t.url));
      if (pages.length) {
        const s = new WebSocket(pages[0].webSocketDebuggerUrl);
        await new Promise((r, j) => { s.addEventListener('open', r, { once: true }); s.addEventListener('error', j, { once: true }); });
        s.addEventListener('message', (e) => {
          const m = JSON.parse(e.data);
          if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
        });
        ws = s;
      }
    } catch {}
    if (!ws) await sleep(250);
  }
  if (!ws) throw new Error('the board\'s page never came up');
};
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
const until = async (test, what, ms = 15000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};
const shown = (id) => run(`!document.getElementById(${JSON.stringify(id)}).hidden`);
const key = async (name) => {
  const base = { key: name, code: name, windowsVirtualKeyCode: { Escape: 27 }[name] };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};

try {
  await connect();
  await until(() => run(`!!(S && S.groups && S.groups.length === 1)`), 'the board');
  hwnd = win32(`$app.MainWindowHandle.ToInt64()`);
  if (!/^[1-9][0-9]*$/.test(hwnd)) die('the window was not found: ' + hwnd);
  // Registered and working, or this copy did not get them
  await until(() => run(`!!(S.hotkeys && S.hotkeys.quick_commands && S.hotkeys.ideas)`), 'the keys to be registered', 8000)
    .catch(() => die('the keys are not this copy\'s: another program, or another copy of the app, has Alt+Shift+K or Alt+Shift+M'));
  const keys = await run(`JSON.stringify(S.hotkeys)`);
  check(keys.includes('"quick_commands":"Alt+Shift+K"') && keys.includes('"ideas":"Alt+Shift+M"'), 'out of the box: Alt+Shift+K and Alt+Shift+M -- ' + keys);

  console.log('1. minimised, another program in front: the key brings the window back with the ideas open');
  minimise();
  await until(async () => minimised() && !inFront(), 'the window minimised');
  press('m');
  await until(() => shown('ideas'), 'the ideas');
  await until(async () => inFront() && !minimised(), 'the window in front');
  check(true, 'the window came forward, no longer minimised, with the ideas open');

  console.log('2. in front, the same key puts them away again, and the other key opens the quick commands');
  press('m');
  await until(async () => !(await shown('ideas')), 'the ideas put away');
  check(true, 'the ideas were put away');
  press('k');
  await until(() => shown('quick'), 'the quick commands');
  check(true, 'the quick commands opened');

  console.log('3. minimised with the quick commands open: the key shows them, it does not close them');
  minimise();
  await until(async () => minimised() && !inFront(), 'the window minimised');
  press('k');
  await until(async () => inFront() && !minimised(), 'the window in front');
  await sleep(600);
  check(await shown('quick'), 'the quick commands are still open, in front');
  await key('Escape');
  await until(async () => !(await shown('quick')), 'the quick commands put away');

  console.log('4. put away in the notification area: the key brings it back and opens what it asked for');
  closeBox();
  await until(async () => !visible(), 'the window put away');
  await sleep(1500);
  press('k');
  await sleep(1500);
  await connect();
  await until(() => shown('quick'), 'the quick commands on the page that came back', 20000);
  check(true, 'the window came back with the quick commands open');
  check(inFront(), 'the window is in front');
} catch (e) {
  check(false, e.message);
} finally {
  if (ws) ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);
