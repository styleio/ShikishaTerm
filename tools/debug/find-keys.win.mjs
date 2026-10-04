/**
 * Ctrl+F and F3 pressed inside a page, with real keys.
 *
 * The one part of the search no protocol can press: a key the operating system
 * delivers to the page's own window, which the browser keeps from the page
 * and hands to the board. So this TAKES THE FOREGROUND for a few seconds: the
 * copy's window is brought in front, its page clicked, and the keys sent as a
 * person would press them. Run it when nobody else is using the mouse and the
 * keyboard of this PC (RULES.md, debugging), and say so first.
 *
 *     cargo build
 *     node tools/debug/find-keys.win.mjs
 *
 * Checked: on a page whose controls offer the search, Ctrl+F in the page opens
 * the board's row and F3 moves to the next match; on a page that leaves it out,
 * Ctrl+F opens nothing of the board's (the browser's own box answers it, as the
 * photograph shows). Nothing of a copy somebody is using is touched.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-find-keys');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

const PAGE = '<!doctype html><meta charset=utf-8><title>keys</title><body style="font:20px sans-serif">'
  + '<p>apple one</p><p>apple two</p><p>apple three</p><div style="height:1500px"></div></body>';
const server = http.createServer((q, s) => { s.writeHead(200, { 'content-type': 'text/html; charset=utf-8' }); s.end(PAGE); });
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const site = `http://127.0.0.1:${server.address().port}/`;

stopApp();
await sleep(500);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja', remote: { enabled: false }, resident: false,
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Keys', id: 'keys', folders: [{ cwd: RUN, tabs: [
    { name: 'on', id: 'on', command: `browser ${site}`, nav: { back: true, reload: true, url: true, find: true } },
    { name: 'off', id: 'off', command: `browser ${site}off`, nav: { back: true, reload: true, url: true, find: false } },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const app = spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' });
app.unref();

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());

// The copy's window to the front, a click in the middle of its page, then keys.
// The page's middle is measured on the board in CSS pixels and turned into the
// screen's by the window's own scale
const pressInPage = (rect, dpr, keys, click = true) => ps('-Command', `
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class K {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte v, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public struct POINT { public int X; public int Y; }
}
'@
$p = Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}\\*' -and $_.MainWindowHandle -ne 0 } | Select-Object -First 1
$h = $p.MainWindowHandle
if ([K]::GetForegroundWindow() -ne $h) {
  [K]::ShowWindow($h, 9) | Out-Null
  # Windows lets a program bring a window forward only just after a key: an Alt
  # pressed and let go is the usual way past that. Only when it is not in
  # front already -- the Alt moves the keyboard within the window
  [K]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero); [K]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
  [K]::SetForegroundWindow($h) | Out-Null
}
"front=" + ([K]::GetForegroundWindow() -eq $h)
Start-Sleep -Milliseconds 400
$pt = New-Object K+POINT
$pt.X = [int](${rect.x} * ${dpr}); $pt.Y = [int](${rect.y} * ${dpr})
[K]::ClientToScreen($h, [ref]$pt) | Out-Null
if ('${click}' -eq 'true') {
  [K]::SetCursorPos($pt.X, $pt.Y) | Out-Null
  [K]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); [K]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}
Start-Sleep -Milliseconds 400
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait('${keys}')
`);

try {
  let boardTarget;
  await until(async () => { const p = portOf('shell'); return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page')); }, 'the window\'s page', 40000);
  const board = await connectCdp(boardTarget, { timeout: 8000 });
  await until(() => board.run('!!S && S.tabs.filter(t => t.kind === "browser").length === 2'), 'the two pages', 30000);
  const index = (id) => board.run(`S.tabs.find(t => (t.id || t.name) === ${JSON.stringify(id)}).index`);
  const middle = async () => JSON.parse(await board.run(`(() => { const r = document.getElementById("page").getBoundingClientRect();
    return JSON.stringify({x: r.left + r.width / 2, y: r.top + r.height / 2, dpr: devicePixelRatio}); })()`));

  console.log('1. a page that offers the search');
  await board.run(`send({kind:"select", tab:${await index('on')}})`);
  await until(() => board.run('!!S.nav && S.nav.find'), 'the page with the search in front');
  await sleep(1200);
  let m = await middle();
  console.log('  ' + pressInPage(m, m.dpr, '^f').stdout.trim());
  await until(() => board.run('!!S.seek'), 'the search row', 8000).catch(() => {});
  check(await board.run('!!S.seek && !document.getElementById("seek").hidden'), 'Ctrl+F in the page opens the board\'s search row');
  await board.run('send({kind:"seek", what:"find", text:"apple"})');
  await until(() => board.run('S.seek && S.seek.of === 3 && !S.seek.asked'), 'three matches', 8000).catch(() => {});
  m = await middle();
  // The page still has the keyboard from the press that opened the search: a
  // person goes on pressing F3 without clicking again
  check(await board.run('document.activeElement === document.querySelector("#seek input")'), 'the cursor is in the search box');
  pressInPage(m, m.dpr, '{F3}', false);
  await until(() => board.run('S.seek && S.seek.at === 2'), 'the next match', 8000).catch(() => {});
  check(await board.run('S.seek && S.seek.at === 2 && S.seek.of === 3'), 'F3 in the search box moves to the next match: ' + await board.run('JSON.stringify(S.seek)'));
  // Back in the page, as a person who clicked it to read on: F3 there goes on
  // from where the page was clicked, as a browser's own search does -- the
  // middle of the page, below every match, so the next one is the first
  pressInPage(m, m.dpr, '{F3}', true);
  await until(() => board.run('S.seek && S.seek.at === 1'), 'the first match again', 8000).catch(() => {});
  check(await board.run('S.seek && S.seek.at === 1 && S.seek.of === 3'), 'F3 in the page goes on from where it was clicked: ' + await board.run('JSON.stringify(S.seek)'));
  ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'), '-Under', APP, '-Out', path.join(SHOTS, 'find-keys-on.png'));
  await board.run('send({kind:"seek", what:"close"})');

  console.log('2. a page that leaves it out');
  await board.run(`send({kind:"select", tab:${await index('off')}})`);
  await until(() => board.run('!!S.nav && S.nav.find === false'), 'the page without the search in front');
  await sleep(1200);
  m = await middle();
  pressInPage(m, m.dpr, '^f');
  await sleep(1500);
  check(await board.run('!S.seek && document.getElementById("seek").hidden'), 'Ctrl+F in the page opens nothing of the board\'s');
  ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'), '-Under', APP, '-Out', path.join(SHOTS, 'find-keys-off.png'));
  console.log('  the browser\'s own box: target/shots/find-keys-off.png');
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  stopApp();
  server.close();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
