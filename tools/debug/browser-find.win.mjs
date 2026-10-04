/**
 * Find in page and downloads, through the running app's own window.
 *
 * This checkout's build in a folder of its own, with a small site served here
 * in a browser tab. The search is asked for the way the board asks; the page's
 * links are pressed through the page's debugging port (turned on only for this
 * check); what came of both is read off the board, and off a phone-sized
 * Chrome on the remote door.
 *
 *     cargo build
 *     node tools/debug/browser-find.win.mjs [--ja]
 *
 * Checked:
 *   search    the row opens, WebView2's own search counts the matches a person
 *             sees, Enter and Shift+Enter move through them and go round, Esc
 *             (close) takes the highlights away; the page is searched again for
 *             the same words after it navigates
 *   download  a link that saves a file: the list comes up beside the page, the
 *             line ends "done" with the file in the Downloads folder under its
 *             own name; a second one of the same name is numbered
 *   cancel    a download still coming is stopped from its line
 *   phone     the phone draws the same search and the same list, and "Save to
 *             this device" hands it the file with the board's sign-in, no key
 *             in the address
 *
 * The files saved land in the real Downloads folder (the browser decides
 * that); they carry a name of this run and are deleted at the end.
 * Needs Windows, Node and Chrome (the phone). Photographs land in target/shots.
 */
import {findChrome, connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-browser-find');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const JA = process.argv.includes('--ja');
const TAG = 'sk-find-' + Date.now().toString(36);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// The menu's words, from the language the copy speaks
const L = JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', (process.argv.includes('--ja') ? 'ja' : 'en') + '.json'), 'utf8'));
const L_FIND = L['tui.menu.find'], L_DOWNLOADS = L['tui.nav.downloads'];
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

// The site: words to find, a file to save, and one that comes slowly enough
// to be stopped
const PAGE = (title) => `<!doctype html><html><head><meta charset="utf-8"><title>${title}</title></head><body>
<p>Apple pie, then APPLE   PIE again.</p>
<p>And ap<b>ple</b> pie.</p>
<p style="display:none">apple pie hidden</p>
<p>apple</p><p>pie</p>
<p><a id="dl" href="/file">save</a> <a id="slow" href="/slow">slow</a> <a id="next" href="/next">next</a></p>
<div style="height:2000px"></div><p>apple pie far down</p></body></html>`;
const SLOW_BYTES = 8 * 1024 * 1024;
const server = http.createServer((req, res) => {
  if (req.url.startsWith('/file')) {
    res.writeHead(200, { 'content-type': 'text/plain', 'content-disposition': `attachment; filename="${TAG}.txt"` });
    return res.end('saved by the check\n');
  }
  if (req.url.startsWith('/slow')) {
    res.writeHead(200, { 'content-type': 'application/octet-stream', 'content-length': String(SLOW_BYTES),
      'content-disposition': `attachment; filename="${TAG}-slow.bin"` });
    let sent = 0;
    const chunk = Buffer.alloc(64 * 1024);
    const t = setInterval(() => {
      if (sent >= SLOW_BYTES || res.destroyed) { clearInterval(t); return res.end(); }
      sent += chunk.length; res.write(chunk);
    }, 250);
    req.on('close', () => clearInterval(t));
    return;
  }
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(PAGE(req.url.startsWith('/next') ? 'Next' : 'Shop'));
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const site = `http://127.0.0.1:${server.address().port}/`;

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
const PHONE_KEY = 'browserfindphone012345678';
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Find', id: 'find', folders: [{ cwd: WORK, tabs: [
    { name: 'shop', id: 'shop', command: `browser ${site}`,
      nav: { back: true, forward: true, reload: true, url: true, menu: true, find: true, develop: true, point: true } },
    // The same page with the search left out of its controls
    { name: 'plain', id: 'plain', command: `browser ${site}plain`,
      nav: { back: true, forward: true, reload: true, url: true, menu: true, find: false } },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
async function connect(target, name) {
  const {ws, send, run} = await connectCdp(target);
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `browser-find-${JA ? 'ja-' : ''}${name}-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, send, run, shot };
}

let chrome = null;
const saved = [];
try {
  let boardTarget;
  await until(async () => {
    const p = portOf('shell');
    return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page'));
  }, 'the window\'s page', 40000);
  const board = await connect(boardTarget, 'window');
  await until(() => board.run('!!S && S.tabs.some(t => t.kind === "browser")'), 'the board and the page', 30000);
  const tab = await board.run('S.tabs.find(t => t.kind === "browser").index');
  await board.run(`send({kind:"select", tab:${tab}})`);
  await until(() => board.run(`S.active === ${tab} && !!S.nav`), 'the page in front with its bar');
  let pageTarget;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    return p && (pageTarget = (await targetsOf(p)).find((t) => t.type === 'page' && t.url.startsWith(site)));
  }, 'the page\'s own debugging port', 30000);
  const page = await connect(pageTarget, 'page');
  // A press a person makes: a site's second download in a row is refused to a
  // script's click, as it is in every browser, and let through to a hand's
  const press = async (sel) => {
    const at = JSON.parse(await page.run(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); e.scrollIntoView({block:"center"});
      const r = e.getBoundingClientRect(); return JSON.stringify([r.left + r.width / 2, r.top + r.height / 2]); })()`));
    for (const type of ['mousePressed', 'mouseReleased']) {
      await page.send('Input.dispatchMouseEvent', { type, x: at[0], y: at[1], button: 'left', clickCount: 1 });
    }
  };
  await until(() => page.run('document.readyState === "complete"'), 'the page loaded');

  console.log('1. find in page');
  check((await board.run("(() => { document.querySelector(\"#nav .navmenu\").click(); const rows = [...document.querySelectorAll(\".fmenu.pagemenu > div\")].map(d => d.textContent); closeFolderMenu(); return rows; })()")).some((t) => t.startsWith(L_FIND)), 'the bar\'s menu offers Find in page');
  await board.run('send({kind:"seek", what:"open"})');
  await until(() => board.run('!!S.seek && !document.getElementById("seek").hidden'), 'the search row');
  check(await board.run('document.activeElement === document.querySelector("#seek input")'), 'the cursor is in the box');
  await board.run('send({kind:"seek", what:"find", text:"apple pie"})');
  await until(() => board.run('S.seek && S.seek.of > 0 && !S.seek.asked'), 'an answer', 10000);
  const first = await board.run('JSON.stringify([S.seek.at, S.seek.of])');
  check(first === '[1,4]', 'the matches a person sees are counted (shown, any case, across a tag; not hidden, not across blocks): ' + first);
  await board.shot('1-find');
  await board.run('send({kind:"seek", what:"next"})');
  await until(() => board.run('S.seek.at === 2'), 'the next match', 5000).catch(() => {});
  check(await board.run('S.seek.at === 2'), 'Enter goes to the next match');
  for (let i = 0; i < 3; i++) await board.run('send({kind:"seek", what:"next"})');
  await until(() => board.run('S.seek.at === 1'), 'round to the first', 5000).catch(() => {});
  check(await board.run('S.seek.at === 1'), 'after the last it goes round to the first: ' + await board.run('S.seek.at'));
  await board.run('send({kind:"seek", what:"prev"})');
  await until(() => board.run('S.seek.at === 4'), 'round to the last', 5000).catch(() => {});
  check(await board.run('S.seek.at === 4'), 'before the first it goes round to the last');
  check(await page.run('scrollY > 500'), 'the page scrolled to the match far down');
  await page.run('document.getElementById("next").click()');
  await until(() => page.run('document.title === "Next"'), 'the next page', 10000).catch(() => {});
  await until(() => board.run('S.seek && S.seek.of === 4 && !S.seek.asked'), 'the new page searched', 10000).catch(() => {});
  check(await board.run('S.seek && S.seek.text === "apple pie" && S.seek.of === 4'), 'the next page is searched for the same words');
  await board.run('send({kind:"seek", what:"close"})');
  await until(() => board.run('!S.seek && document.getElementById("seek").hidden'), 'the row closed', 5000).catch(() => {});
  check(await board.run('!S.seek'), 'close takes the row away');

  console.log('1b. a page that leaves the search out');
  const plain = await board.run('S.tabs.find(t => (t.id || t.name) === "plain").index');
  await board.run(`send({kind:"select", tab:${plain}})`);
  await until(() => board.run(`S.active === ${plain} && !!S.nav`), 'the plain page in front');
  check(await board.run('S.nav.find === false') && !(await board.run("(() => { document.querySelector(\"#nav .navmenu\").click(); const rows = [...document.querySelectorAll(\".fmenu.pagemenu > div\")].map(d => d.textContent); closeFolderMenu(); return rows; })()")).some((t) => t.startsWith(L_FIND)), 'no Find in page in its menu');
  await board.run('send({kind:"seek", what:"open"})');
  await sleep(800);
  check(await board.run('!S.seek && document.getElementById("seek").hidden'), 'and asking for the search there opens nothing');
  await board.run(`send({kind:"select", tab:${tab}})`);
  await until(() => board.run(`S.active === ${tab}`), 'the shop page in front again');

  console.log('2. a download');
  for (let i = 0; i < 2; i++) {
    await press('#dl');
    await until(() => board.run(`S.downloads.length === ${i + 1} && S.downloads[0].state === "done"`), 'the download done', 20000)
      .catch(async (e) => { throw new Error(e.message + ': ' + await board.run('JSON.stringify(S.downloads)')); });
  }
  const lines = JSON.parse(await board.run('JSON.stringify(S.downloads)'));
  for (const d of lines) saved.push(d.path);
  check(lines[1].name === `${TAG}.txt` && lines[0].name === `${TAG} (1).txt`, 'saved under its own name, the second numbered: ' + lines.map((d) => d.name).join(', '));
  check(lines.every((d) => fs.existsSync(d.path) && fs.readFileSync(d.path, 'utf8') === 'saved by the check\n'), 'the files are where the list says');
  check(lines.every((d) => d.page === 'shop' && d.site.startsWith('127.0.0.1:')), 'each line names the page and the site');
  check(await board.run('!document.getElementById("dlpanel").hidden'), 'the list came up beside the page');
  check((await board.run("(() => { document.querySelector(\"#nav .navmenu\").click(); const rows = [...document.querySelectorAll(\".fmenu.pagemenu > div\")].map(d => d.textContent); closeFolderMenu(); return rows; })()")).some((t) => t.startsWith(L_DOWNLOADS)), 'the bar\'s menu offers Downloads');
  await board.shot('2-downloads');

  console.log('3. cancel');
  await press('#slow');
  await until(() => board.run('S.downloads[0].state === "going" && S.downloads[0].got > 0'), 'the slow one coming', 20000);
  check(await board.run('S.downloads[0].total === ' + SLOW_BYTES), 'its whole size is known');
  await board.shot('3-going');
  const slowId = await board.run('S.downloads[0].id');
  await board.run(`send({kind:"download", act:"cancel", id:${JSON.stringify(slowId)}})`);
  await until(() => board.run('S.downloads[0].state === "cancelled"'), 'cancelled', 10000).catch(() => {});
  check(await board.run('S.downloads[0].state === "cancelled"'), 'stopped from its line: ' + await board.run('S.downloads[0].state'));

  console.log('4. the phone');
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
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
  await until(() => phone.run('!!S && S.tabs.some(t => t.kind === "browser")'), 'the board on the phone', 30000);
  await phone.run(`send({kind:"select", tab:${tab}})`);
  await until(() => phone.run(`S.active === ${tab} && !!S.nav`), 'the page in front on the phone');
  await phone.run('send({kind:"seek", what:"open"}); send({kind:"seek", what:"find", text:"apple"})');
  await until(() => phone.run('S.seek && S.seek.of > 0 && !S.seek.asked && !document.getElementById("seek").hidden'), 'the search on the phone', 10000).catch(() => {});
  check(await phone.run('!!S.seek && S.seek.of > 0'), 'the phone searches the same page: ' + await phone.run('JSON.stringify(S.seek)'));
  check(await board.run('!!S.seek && S.seek.text === "apple"'), 'and the window shows the same search');
  await phone.shot('1-find');
  await phone.run('send({kind:"seek", what:"close"}); sideReveal("downloads")');
  await until(() => phone.run('document.querySelectorAll("#dlpanel .dl").length === 3'), 'the list on the phone', 10000).catch(() => {});
  const href = await phone.run('(document.querySelector("#dlpanel a[download]") || {}).href || ""');
  check(href.includes('/api/download?id=') && !href.includes('t='), 'a finished file is offered to the phone, with no key in its address: ' + href);
  const got = await phone.run(`fetch(${JSON.stringify(href)}).then(r => r.status + ":" + r.headers.get("content-disposition"))`);
  check(got.startsWith('200:attachment;'), 'the phone is handed the file: ' + got);
  const stranger = await fetch(href.replace(/^https?:\/\/[^/]+/, `http://127.0.0.1:${PHONE_PORT}`)).then((r) => r.status);
  check(stranger === 403, 'somebody without the board\'s sign-in is not: ' + stranger);
  const none = await phone.run(`fetch("api/download?id=nope").then(r => r.status)`);
  check(none === 404, 'a line that is not in the list is nothing: ' + none);
  await phone.shot('2-downloads');
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
  server.close();
  for (const p of saved) fs.rmSync(p, { force: true });
  // The cancelled one leaves nothing behind, or a partial file the browser keeps
  const dl = path.join(os.homedir(), 'Downloads');
  for (const f of fs.existsSync(dl) ? fs.readdirSync(dl) : []) if (f.startsWith(TAG)) fs.rmSync(path.join(dl, f), { force: true });
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
