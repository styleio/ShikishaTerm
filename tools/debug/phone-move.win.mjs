/**
 * A phone watching the board when the address the board listens on changes.
 *
 * The board moves when the network under "auto" moves (the LAN to Tailscale,
 * or back) and when the person changes only where it listens. Either way the
 * old address stops answering, and a phone left on it used to try that
 * address for ever. This starts this checkout's build in a folder of its own,
 * listening on 127.0.0.1, with a phone-sized headless Chrome paired to it,
 * then changes where it listens by writing the settings file -- the road a
 * person's save takes -- and watches what the phone does:
 *
 *   follows   moved to 127.0.0.2, the phone opens the new address by itself
 *             and is connected there: the board is drawn, nothing is over it,
 *             and no token or code is left in its address or its storage
 *   far       moved to 127.0.0.3, which this phone's Chrome cannot reach (a
 *             dead proxy stands in front of that one address), the phone says
 *             so in words, names the address, and offers to try again
 *   back      moved back to 127.0.0.2, where the phone still is, it is let in
 *             there on the code it was handed for the move, and is connected,
 *             without being paired again
 *
 * Every other loopback address answers on Windows without being set up, which
 * is what lets one machine play three.
 *
 *     cargo build
 *     node tools/debug/phone-move.win.mjs [--ja]
 *
 * Needs Windows, Node and Chrome. Photographs land in target/shots. Nothing of
 * a copy somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-phone-move');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const PHONE_DIR = path.join(RUN, 'phone');
const CONFIG = path.join(APP, 'config', 'config.json');
const JA = process.argv.includes('--ja');
const L = JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', 'en.json'), 'utf8'));
if (JA) Object.assign(L, JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', 'ja.json'), 'utf8')));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-phone-move*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};
const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
// A copy of WebView2 can hold a file of an earlier run for a while after it
// was told to stop; that run's folder is then left for the next time
try { fs.rmSync(RUN, { recursive: true, force: true }); } catch (e) { console.log('  (a folder of an earlier run is still held: ' + e.code + '; its files are written over)'); }
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

const PORT = await freePort();
const KEY = 'phonemove0123456789abcd';
const settingsAt = (bind) => ({
  language: JA ? 'ja' : 'en',
  remote: { enabled: true, bind, port: PORT },
  desks: [{ name: 'Move', id: 'move', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
});
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify(settingsAt('127.0.0.1'), null, 2));
// The board's own token, known here so the phone can pair without reading the app's screen.
// Not a fixed token (that is the sticky mode's, and keeps the token in the address)
fs.mkdirSync(path.join(APP, 'data'), { recursive: true });
fs.writeFileSync(path.join(APP, 'data', 'remote-token'), KEY);
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
/** Where the board listens, written the way a person's save writes it */
const listenAt = (bind) => {
  const now = JSON.parse(fs.readFileSync(CONFIG, 'utf8').replace(/^﻿/, ''));
  now.remote.bind = bind;
  fs.writeFileSync(CONFIG, JSON.stringify(now, null, 2));
};
const answers = async (host) => {
  try { return (await fetch(`http://${host}:${PORT}/manifest.webmanifest`, { signal: AbortSignal.timeout(1500) })).ok; } catch { return false; }
};

const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
async function connect(target) {
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
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `phone-move-${JA ? 'ja-' : ''}${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { send, run, shot };
}
function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('no Chrome found; set CHROME');
}

let chrome = null;
try {
  await until(() => answers('127.0.0.1'), 'the board on 127.0.0.1', 40000);

  // The phone. Everything goes through a proxy that answers nothing, except
  // the two addresses this phone is meant to reach: that is how 127.0.0.3
  // becomes an address on a network this phone is not on
  fs.mkdirSync(PHONE_DIR, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + PHONE_DIR,
    '--no-first-run', '--no-default-browser-check',
    '--proxy-server=http://127.0.0.1:9', '--proxy-bypass-list=<-loopback>;127.0.0.1;127.0.0.2',
    'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(PHONE_DIR, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const dport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let target;
  await until(async () => (target = (await targetsOf(dport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connect(target);
  await phone.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/?t=${KEY}` });
  const onBoard = () => phone.run('typeof S === "object" && !!S && Array.isArray(S.tabs) && S.tabs.length > 0 && !(document.getElementById("netveil") && !document.getElementById("netveil").hidden)');
  await until(onBoard, 'the phone on the board', 30000).catch(async (e) => { console.log('    (the phone: ' + JSON.stringify(await phone.run('({href: location.href, S: typeof S, tabs: typeof S === "object" && S ? (S.tabs||[]).length : -1, veil: (document.getElementById("netveil")||{}).hidden, text: document.body.innerText.slice(0,200)})')) + ')'); throw e; });
  check(true, 'the phone is on the board at 127.0.0.1');

  console.log('1. the board moves to an address the phone reaches');
  listenAt('127.0.0.2');
  await until(() => phone.run('location.host') .then((h) => h === `127.0.0.2:${PORT}`), 'the phone at the new address', 40000)
    .catch(async (e) => { console.log('    (the phone shows: ' + await phone.run('(document.getElementById("netveil") || {}).textContent || location.href') + ')'); throw e; });
  await until(onBoard, 'the board drawn at the new address', 20000);
  check(await onBoard(), 'the phone followed to 127.0.0.2 by itself and is connected there');
  const where = await phone.run('location.href');
  check(!/[?#&](t|move)=/.test(where), 'no token and no code in its address: ' + where);
  check(await phone.run('!sessionStorage.getItem("shikisha_token") && !localStorage.getItem("shikisha_token")'), 'and none in its storage');
  // Once the old board's last few seconds are over
  check(await until(async () => !(await answers('127.0.0.1')), 'the old address to go quiet', 20000).catch(() => false), 'the old address has stopped answering');
  await phone.shot('1-followed');

  console.log('2. the board moves to an address this phone cannot reach');
  listenAt('127.0.0.3');
  const veil = 'document.getElementById("netveil")';
  await until(() => phone.run(`!!${veil} && !${veil}.hidden && !${veil}.querySelector(".nvagain").hidden`), 'the phone saying it cannot reach', 60000)
    .catch(async (e) => {
      console.log('    (the phone shows: ' + await phone.run(`(${veil} || {}).textContent || location.href`) + ')');
      console.log('    (a look from the phone: ' + await phone.run(`(async () => { const t = Date.now(); try { await fetch('http://127.0.0.3:${PORT}/manifest.webmanifest', {mode:'no-cors', cache:'no-store'}); return 'answered in ' + (Date.now() - t); } catch (x) { return 'failed in ' + (Date.now() - t) + ': ' + x; } })()`) + ')');
      throw e; });
  const said = await phone.run(`${veil}.textContent`);
  check(said.includes(L['tui.net.moved.title']) && said.includes(`127.0.0.3:${PORT}`), 'it says the address changed, and to where: ' + said);
  check(await phone.run('location.host') === `127.0.0.2:${PORT}`, 'and stays where it is rather than opening a page that cannot load');
  await phone.shot('2-far');

  console.log('3. the board comes back to the address the phone is on');
  listenAt('127.0.0.2');
  await until(onBoard, 'the phone connected again', 60000)
    .catch(async (e) => {
      console.log('    (the phone shows: ' + await phone.run(`(${veil} || {}).textContent || location.href`) + ')');
      const jar = await phone.send('Network.getCookies', { urls: [`http://127.0.0.2:${PORT}/`] }).catch(() => ({ cookies: [] }));
      console.log('    (its cookies there: ' + jar.cookies.map((c) => c.name).join(', ') + ')');
      const book = path.join(APP, 'data', 'clients.json');
      console.log('    (the book: ' + (fs.existsSync(book) ? fs.readFileSync(book, 'utf8').replace(/\s+/g, ' ') : 'none') + ')');
      const log = path.join(APP, 'logs', 'hooks.log');
      if (fs.existsSync(log)) console.log(fs.readFileSync(log, 'utf8').split(/\r?\n/).filter((l) => /remote/.test(l)).slice(-6).join('\n'));
      throw e; });
  check(await phone.run('location.host') === `127.0.0.2:${PORT}`, 'the phone is on the board again, without pairing again');
  await phone.shot('3-back');
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
