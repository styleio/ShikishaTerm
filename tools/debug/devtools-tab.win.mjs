/**
 * A page's DevTools opened beside it, through the running app's own window.
 *
 * This checkout's build in a folder of its own, with a small page served here
 * in a browser tab. "Developer tools" is asked for the way the tab's
 * menu asks, and what follows is checked from both ends: the DevTools screen
 * (a page of the app's own, read through the pages' debugging port that only
 * this check turns on) and the bridge it talks through.
 *
 *     cargo build
 *     node tools/debug/devtools-tab.win.mjs [--ja]
 *
 * Checked:
 *   opened    a page named `<page>-devtools` is opened, in a pane of its own
 *             beside the page (the pane was divided), listed in the page's
 *             folder -- asked for from the window and from a phone alike
 *   connected the screen says it is inspecting the page (its title names it)
 *   works     an expression evaluated through the screen's own connection
 *             answers from the page -- the whole road, both ways
 *   again     asking a second time shows the same screen rather than a second
 *   refused   the bridge refuses a caller without the key, and one with the
 *             key that is not the DevTools screen (a web page's origin)
 *   quiet     the key is not written in the log
 *   phone     a phone-sized Chrome on the remote door lists the screen as a tab
 *
 * Needs Windows, Node and Chrome (the phone). Photographs land in
 * target/shots. Nothing of a copy somebody is using is read, written or stopped.
 */
import {findChrome, connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-devtools');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const JA = process.argv.includes('--ja');
// --split runs the same checks with the window and the runtime as two programs
const SPLIT_MODE = process.argv.includes('--split');
// The words the menu says, from the language files it is drawn from, so a
// renamed item is not taken for a missing one
const readLang = (l) => JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', l + '.json'), 'utf8'));
const L = { ...readLang('en'), ...(JA ? readLang('ja') : {}) };

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

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end('<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Inspected</title></head><body><h1>Inspect me</h1></body></html>');
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});
const PHONE_PORT = await freePort();
const PHONE_KEY = 'devtoolsphone0123456789a';
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  ...(SPLIT_MODE ? { split: true } : {}),
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  // Answered already, so the start does not stop on the question about the
  // AI CLIs' hooks (this checks nothing about them)
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'DevTools', id: 'devtools', folders: [{ cwd: WORK, tabs: [
    { name: 'page', id: 'page', command: `browser http://127.0.0.1:${pagePort}/` },
    { name: 'other', id: 'other', command: `browser http://127.0.0.1:${pagePort}/other` },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const launch = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
launch();

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
// Where the pages are drawn: the window's own engine, or -- the window and
// the runtime being two programs -- a browser the runtime started
const pagesPort = () => {
  if (!SPLIT_MODE) return portOf(path.join('profiles', 'default'));
  const f = path.join(LOCAL, 'ShikishaTerm', 'chromium', 'default', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
async function connect(target) {
  const {ws, send, run} = await connectCdp(target);
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `devtools-tab-${JA ? 'ja-' : ''}${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, send, run, shot };
}
// The first answer of a WebSocket handshake to the bridge, from a caller of
// our choosing: the status line is all that is looked at
function knock(port, pathname, origin) {
  return new Promise((resolve) => {
    const s = net.connect(port, '127.0.0.1', () => {
      s.write(`GET ${pathname} HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n` +
        `Sec-WebSocket-Key: ${crypto.randomBytes(16).toString('base64')}\r\nSec-WebSocket-Version: 13\r\n` +
        (origin ? `Origin: ${origin}\r\n` : '') + '\r\n');
    });
    let got = '';
    s.on('data', (d) => { got += d; if (got.includes('\r\n')) { resolve(got.split('\r\n')[0]); s.destroy(); } });
    s.on('error', () => resolve('error'));
    setTimeout(() => { resolve(got || 'nothing'); s.destroy(); }, 3000);
  });
}

// The folder a tab is listed under, as the board has it
const folderOf = (on, key) => on.run(`(() => { const t = S.tabs.find(x => (x.id || x.name) === ${JSON.stringify(key)});
  const g = t && S.groups && S.groups[t.group]; return g ? String(g.folder || "") : ""; })()`);
const standsIn = async (on, key) => {
  const f = (await folderOf(on, key)).replace(/[\\/]+$/, '').toLowerCase();
  return !!f && f === WORK.replace(/[\\/]+$/, '').toLowerCase();
};

let chrome = null;
const PHONE_DIR = path.join(RUN, 'phone');


try {
  let boardTarget;
  await until(async () => {
    const p = portOf('shell');
    return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page'));
  }, 'the window\'s page', 40000);
  const board = await connect(boardTarget);
  await until(() => board.run('typeof tabMenu === "function" && !!S && S.tabs.some(t => t.kind === "browser")'), 'the board and the page', 30000);
  const pageTab = await board.run('S.tabs.find(t => t.kind === "browser").index');
  await board.run(`send({kind:"select", tab:${pageTab}})`);
  await until(() => board.run(`S.active === ${pageTab}`), 'the page in front');
  check((await board.run(`(() => { const t = S.tabs.find(x => x.index === ${pageTab}); const a = document.createElement("div"); document.body.append(a);
    tabMenu(a, t, "strip", null); const said = [...document.querySelectorAll(".fmenu div")].map(d => d.textContent); closeFolderMenu(); a.remove(); return said; })()`))
    .includes(L['tui.dev.devtools']), 'the page\'s tab menu offers it');

  console.log('1. DevTools asked for');
  const panes = () => board.run('document.querySelectorAll("#panes .pane").length');
  const panesBefore = await panes();
  await board.run('send({kind:"devtools", page:"page"})');
  await until(() => board.run('S.tabs.some(t => t.kind === "browser" && (t.id || t.name) === "page-devtools")'), 'the screen as a tab', 20000);
  check(true, 'a page named page-devtools is open');
  check(await standsIn(board, 'page-devtools'), 'it stands in the page\'s folder, not apart from every folder: ' + await folderOf(board, 'page-devtools'));
  await until(async () => (await panes()) > panesBefore, 'the pane divided', 10000).catch(() => {});
  check((await panes()) > panesBefore, `the pane was divided (${panesBefore} -> ${await panes()})`);
  const screenTab = await board.run('S.tabs.find(t => (t.id || t.name) === "page-devtools").index');
  await until(() => board.run(`S.panes.panes.some(p => p.surface === ${screenTab}) && S.panes.panes.every(p => p.surface > 0)`), 'the screen in a pane', 10000).catch(() => {});
  check(await board.run(`S.panes.panes.some(p => p.surface === ${screenTab}) && S.panes.panes.every(p => p.surface > 0)`), 'the screen stands in the new half, the page in the other: ' + await board.run('JSON.stringify(S.panes.panes.map(p => p.surface))'));

  let screenTarget;
  await until(async () => {
    const p = pagesPort();
    return p && (screenTarget = (await targetsOf(p)).find((t) => t.url.startsWith('devtools://devtools/bundled/devtools_app.html')));
  }, 'the DevTools screen', 30000);
  const screen = await connect(screenTarget);
  // The screen names the page it inspects once it has asked the page, through
  // the bridge, what it is
  const inspected = () => screen.run(`(async () => { const m = await import("./core/sdk/sdk.js");
    const t = m.TargetManager.TargetManager.instance().primaryPageTarget(); return t ? t.inspectedURL() : ""; })()`);
  await until(() => inspected().then((u) => u.includes(`127.0.0.1:${pagePort}`)), 'the screen to know the page', 30000).catch(() => {});
  const seen = await inspected().catch((e) => 'failed: ' + e.message);
  check(String(seen).includes(`127.0.0.1:${pagePort}`), 'the screen is inspecting the page: ' + seen);

  console.log('2. an expression through the screen\'s own connection');
  const answer = await screen.run(`(async () => {
    const m = await import("./core/sdk/sdk.js");
    const t = m.TargetManager.TargetManager.instance().primaryPageTarget();
    const r = await t.runtimeAgent().invoke_evaluate({expression: "document.title + '/' + document.querySelector('h1').textContent"});
    return r.result.value;
  })()`).catch((e) => 'failed: ' + e.message);
  check(answer === 'Inspected/Inspect me', 'the page answered through the bridge: ' + answer);
  await sleep(1500);
  await screen.shot('1-screen');

  console.log('3. asked again');
  await board.run('send({kind:"devtools", page:"page"})');
  await sleep(2000);
  check((await board.run('S.tabs.filter(t => (t.id || t.name) === "page-devtools").length')) === 1, 'still one screen');
  const after = await panes();
  check(after === panesBefore + 1, `and the pane was not divided again (${after})`);

  console.log('4. the bridge, knocked on from elsewhere');
  const u = new URL(screenTarget.url);
  const [host, ...rest] = u.searchParams.get('ws').split('/');
  const bport = Number(host.split(':')[1]);
  const good = '/' + rest.join('/');
  const noKey = good.replace(/\/devtools\/[0-9a-f]+\//, '/devtools/0000/');
  // Two programs: the screen is a page of the browser the runtime started,
  // pointed at that browser's own port -- there is no bridge and no key. What
  // still has to hold is that a web page's origin is not let in
  const key = SPLIT_MODE ? null : rest[1];
  if (!SPLIT_MODE) check((await knock(bport, noKey, 'devtools://devtools')).includes('403'), 'a wrong key is refused');
  check(/40[03]/.test(await knock(bport, good, 'http://evil.example')), 'the right address from a web page is refused');
  if (!SPLIT_MODE) {
    check((await knock(bport, good, '')).includes('403'), 'the right key with no origin is refused');
    const log = fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '';
    check(!log.includes(key), 'the key is not in the log');
  }

  console.log('5. the app started again');
  // The division is written down a moment after it is made
  await until(() => fs.readFileSync(CONFIG, 'utf8').includes('page-devtools'), 'the split written down', 20000);
  stopApp();
  await sleep(1000);
  for (const e of ['shell', path.join('profiles', 'default')]) {
    fs.rmSync(path.join(LOCAL, 'ShikishaTerm', 'webview2', e, 'EBWebView', 'DevToolsActivePort'), { force: true });
  }
  launch();
  let board2Target;
  await until(async () => {
    const p = portOf('shell');
    return p && (board2Target = (await targetsOf(p)).find((t) => t.type === 'page'));
  }, 'the window\'s page again', 40000);
  const board2 = await connect(board2Target);
  await until(() => board2.run('!!S && S.tabs.some(t => t.kind === "split")'), 'the split row', 30000);
  await board2.run('send({kind:"select", tab: S.tabs.find(t => t.kind === "split").index})');
  await until(() => board2.run('S.tabs.some(t => (t.id || t.name) === "page-devtools")'), 'the screen made again', 20000).catch(() => {});
  check(await board2.run('S.tabs.some(t => (t.id || t.name) === "page-devtools")'), 'the DevTools screen is made again');
  await until(() => board2.run('S.panes && S.panes.panes.length === 2 && S.panes.panes.every(p => p.surface > 0)'), 'both panes filled', 20000).catch(() => {});
  check(await board2.run('S.panes.panes.length === 2 && S.panes.panes.every(p => p.surface > 0)'),
    'the split comes back with both halves: ' + await board2.run('JSON.stringify(S.panes.panes.map(p => p.surface))'));
  let screen2Target;
  await until(async () => {
    const p = pagesPort();
    return p && (screen2Target = (await targetsOf(p)).find((t) => t.url.startsWith('devtools://devtools/bundled/devtools_app.html')));
  }, 'the screen\'s page again', 30000);
  if (key) check(!screen2Target.url.includes(key), 'with a key of this run');
  const screen2 = await connect(screen2Target);
  const again = () => screen2.run(`(async () => { const m = await import("./core/sdk/sdk.js");
    const t = m.TargetManager.TargetManager.instance().primaryPageTarget(); return t ? t.inspectedURL() : ""; })()`);
  await until(() => again().then((u) => u.includes(`127.0.0.1:${pagePort}`)), 'the screen to know the page again', 30000).catch(() => {});
  check(String(await again().catch((e) => e.message)).includes(`127.0.0.1:${pagePort}`), 'and it is inspecting the page again');

  console.log('6. the phone');
  fs.mkdirSync(PHONE_DIR, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + PHONE_DIR,
    '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(PHONE_DIR, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const pport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let pt;
  await until(async () => (pt = (await targetsOf(pport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connect(pt);
  await phone.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
  await until(() => phone.run('!!S && S.tabs.some(t => (t.id || t.name) === "page-devtools")'), 'the screen on the phone', 30000);
  check(true, 'the phone lists the screen as a tab');
  // Asked for from the phone, the way a person there would: it stands where
  // its page does, as the window's did (from the phone it once stood apart
  // from every folder)
  await phone.run('send({kind:"devtools", page:"other"})');
  await until(() => phone.run('S.tabs.some(t => (t.id || t.name) === "other-devtools")'), 'the phone\'s DevTools as a tab', 30000);
  check(await standsIn(phone, 'other-devtools'), 'opened from the phone, it stands in its page\'s folder: ' + await folderOf(phone, 'other-devtools'));
  await phone.shot('2-phone');
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
  server.close();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
