/**
 * A page's alert, confirm and prompt, through the running app's own window.
 *
 * This checkout's build in a folder of its own, with a small site served here
 * in a browser tab. A person's press on the page is made through the page's
 * debugging port (turned on only for this check); the question is read off
 * the board and answered there, as a person would; then a script meets the
 * same boxes and answers them with `browser_dialog`.
 *
 *     cargo build
 *     node tools/debug/page-dialog.win.mjs [--ja]
 *
 * Checked:
 *   person   a confirm is held (the page stays stopped) and asked on the
 *            board with the site's name; OK and Cancel reach the page as
 *            themselves; a prompt takes what was typed, spaces and all; an
 *            alert has one button and putting it away acknowledges it
 *   script   a click whose handler asks ends at once saying a box opened;
 *            whatever is asked next says a box is open and quotes it;
 *            `browser_dialog` reads it and answers it, and the board's
 *            question goes with it
 *   leaving  "leave this page?" is not asked: the page is left
 *   window   the app's window, photographed while a box is held, for a look
 *            at whether the engine drew a box of its own as well
 *
 * Needs Windows and Node. Photographs land in target/shots.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-page-dialog');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const JA = process.argv.includes('--ja');

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

// The site: one button for each kind of box, each writing what it was
// answered into the page's title, and a page that asks before it is left
const PAGE = (title) => `<!doctype html><html><head><meta charset="utf-8"><title>${title}</title></head><body>
<h1>Shop</h1>
<p><button id="a" onclick="alert('Saved.\\nSecond line'); document.title = 'alerted'">alert</button>
<button id="c" onclick="document.title = confirm('Delete the order?') ? 'yes' : 'no'">confirm</button>
<button id="p" onclick="document.title = 'p:' + JSON.stringify(prompt('Your name?', 'nobody'))">prompt</button>
<button id="stay" onclick="window.onbeforeunload = e => { e.preventDefault(); e.returnValue = 'x'; return 'x'; }; document.title = 'guarded'">guard</button>
<a id="next" href="/next">next</a></p></body></html>`;
const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(PAGE(req.url.startsWith('/next') ? 'Next' : 'Shop'));
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const site = `http://127.0.0.1:${server.address().port}/`;
const host = `127.0.0.1:${server.address().port}`;

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Dialogs', id: 'dialogs', folders: [{ cwd: WORK, tabs: [
    { name: 'shop', id: 'shop', command: `browser ${site}`,
      nav: { back: true, forward: true, reload: true, url: true, menu: true } },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const app = spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
app.unref();

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
    fs.writeFileSync(path.join(SHOTS, `page-dialog-${JA ? 'ja-' : ''}${name}-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, send, run, shot };
}
const logged = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '');

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
  await until(() => page.run('document.readyState === "complete"'), 'the page loaded');
  // A press a person makes. Not waited for: the browser does not say the
  // press is done until the box it opened is answered
  const press = async (sel) => {
    const at = JSON.parse(await page.run(`(() => { const e = document.querySelector(${JSON.stringify(sel)});
      const r = e.getBoundingClientRect(); return JSON.stringify([r.left + r.width / 2, r.top + r.height / 2]); })()`));
    for (const type of ['mousePressed', 'mouseReleased']) {
      page.send('Input.dispatchMouseEvent', { type, x: at[0], y: at[1], button: 'left', clickCount: 1 }).catch(() => {});
    }
  };
  // What the page is called now, asked of the browser and not of the page:
  // a stopped page does not answer
  // (the list writes it as HTML does)
  const titleNow = async () => (((await targetsOf(portOf(path.join('profiles', 'default')))).find((t) => t.id === pageTarget.id) || {}).title || '')
    .replaceAll('&quot;', '"').replaceAll('&amp;', '&');
  const asked = () => board.run('JSON.stringify((S.page_dialogs || [])[0] || null)').then(JSON.parse);
  const boxUp = () => board.run('!document.getElementById("sask").hidden && document.getElementById("sask").classList.contains("pagesays")');
  const noBox = () => until(() => board.run('!(S.page_dialogs || []).length && document.getElementById("sask").hidden'), 'the question gone', 8000);

  console.log('1. a person at the page');
  await press('#c');
  await until(async () => (await asked()) && (await boxUp()), 'the confirm asked on the board', 10000);
  let d = await asked();
  check(d.kind === 'confirm' && d.message === 'Delete the order?' && d.host === host, 'the board holds the confirm, with the site: ' + JSON.stringify(d));
  check((await board.run('document.querySelector("#sask .vtitle").textContent')).includes(host), 'the site that asks is in the title');
  check(await board.run('document.querySelector("#sask .vsay").textContent === "Delete the order?" && !document.querySelector("#sask .brow > .quiet").hidden'),
    'the page\'s words, and both of its answers');
  await sleep(1500);
  check((await titleNow()) === 'Shop', 'the page waits: nothing answered for the person');
  await board.shot('1-confirm');
  ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'), '-ProcessId', String(app.pid), '-Out',
    path.join(SHOTS, `page-dialog-${JA ? 'ja-' : ''}native-1-confirm.png`));
  await board.run('document.querySelector("#sask .brow > .go").click()');
  await until(async () => (await titleNow()) === 'yes', 'OK reaching the page', 8000).catch(() => {});
  check((await titleNow()) === 'yes', 'OK is the page\'s OK: ' + await titleNow());
  await noBox();

  await press('#c');
  await until(boxUp, 'the confirm again', 10000);
  await board.run('document.querySelector("#sask .brow > .quiet").click()');
  await until(async () => (await titleNow()) === 'no', 'Cancel reaching the page', 8000).catch(() => {});
  check((await titleNow()) === 'no', 'Cancel is the page\'s Cancel: ' + await titleNow());
  await noBox();

  await press('#p');
  await until(boxUp, 'the prompt', 10000);
  check(await board.run('!document.getElementById("sq").hidden && document.getElementById("sq").value === "nobody"'), 'a prompt has its field, starting with what the page put there');
  await board.shot('2-prompt');
  await board.run('document.getElementById("sq").value = " Ada "; document.querySelector("#sask .brow > .go").click()');
  await until(async () => (await titleNow()) === 'p:" Ada "', 'the typed words reaching the page', 8000).catch(() => {});
  check((await titleNow()) === 'p:" Ada "', 'what was typed reaches the page as typed, its spaces too: ' + await titleNow());
  await noBox();

  await press('#p');
  await until(boxUp, 'the prompt again', 10000);
  await board.run('closeAsk()');
  await until(async () => (await titleNow()) === 'p:null', 'the prompt put away', 8000).catch(() => {});
  check((await titleNow()) === 'p:null', 'a prompt put away is cancelled: ' + await titleNow());
  await noBox();

  await press('#a');
  await until(boxUp, 'the alert', 10000);
  check(await board.run('document.querySelector("#sask .brow > .quiet").hidden && document.querySelector("#sask .vsay").textContent === "Saved.\\nSecond line"'),
    'an alert has one button, and its lines are the page\'s lines');
  await board.shot('3-alert');
  await board.run('closeAsk()');
  await until(async () => (await titleNow()) === 'alerted', 'the alert acknowledged', 8000).catch(() => {});
  check((await titleNow()) === 'alerted', 'putting an alert away acknowledges it: ' + await titleNow());
  await noBox();

  console.log('2. a script at the page');
  const lua = (code) => board.run(`send({kind:"runlua", code:${JSON.stringify(code)}})`);
  const said = async (mark) => {
    await until(() => logged().includes(mark + ' '), mark + ' in the log', 30000);
    return logged().split(/\r?\n/).filter((l) => l.includes(mark + ' ')).pop();
  };
  const t0 = Date.now();
  await lua(`local ok, e = pcall(shikisha.browser_click, "shop", "#c") shikisha.log("DLG1 " .. tostring(ok) .. " " .. tostring(e))`);
  const clicked = await said('DLG1');
  const took = Date.now() - t0;
  check(clicked.includes('DLG1 false') && clicked.includes('Delete the order?') && clicked.includes('browser_dialog("shop"'),
    'the click that opened it says so, quotes it, and says how to answer: ' + clicked.slice(clicked.indexOf('DLG1')));
  check(took < 8000, 'and says so at once, not when its time runs out: ' + took + ' ms');
  await lua(`local ok, e = pcall(shikisha.browser_text, "shop", "h1") shikisha.log("DLG2 " .. tostring(ok) .. " " .. tostring(e))`);
  const next = await said('DLG2');
  check(next.includes('DLG2 false') && next.includes('Delete the order?'), 'what is asked of the page next is refused with the box\'s words: ' + next.slice(next.indexOf('DLG2')));
  check(await boxUp(), 'the person is asked the same question meanwhile');
  await lua(`local d = shikisha.browser_dialog("shop") shikisha.log("DLG3 " .. d.type .. "|" .. d.message .. "|" .. d.default)`);
  check((await said('DLG3')).includes('DLG3 confirm|Delete the order?|'), 'the script reads the box');
  await lua(`shikisha.browser_dialog("shop", "dismiss") shikisha.log("DLG4 " .. tostring(shikisha.browser_dialog("shop")))`);
  check((await said('DLG4')).includes('DLG4 nil'), 'and answers it: none is open after');
  await until(async () => (await titleNow()) === 'no', 'the script\'s Cancel reaching the page', 8000).catch(() => {});
  check((await titleNow()) === 'no', 'the page got the script\'s Cancel: ' + await titleNow());
  await noBox();
  check(true, 'the board\'s question went with the script\'s answer');
  await lua(`pcall(shikisha.browser_click, "shop", "#p") shikisha.browser_dialog("shop", "accept", "Grace") shikisha.log("DLG5 " .. tostring(shikisha.browser_text("shop", "h1")))`);
  check((await said('DLG5')).includes('DLG5 Shop'), 'after the answer the page can be read again');
  check((await titleNow()) === 'p:"Grace"', 'a prompt takes the script\'s words: ' + await titleNow());
  await lua(`pcall(shikisha.browser_click, "shop", "#p") shikisha.browser_dialog("shop", "accept") shikisha.log("DLG6 done")`);
  await said('DLG6');
  await until(async () => (await titleNow()) === 'p:"nobody"', 'the default kept', 8000).catch(() => {});
  check((await titleNow()) === 'p:"nobody"', 'accepted with no words, a prompt keeps what the page put there: ' + await titleNow());
  await lua(`local ok, e = pcall(shikisha.browser_dialog, "shop", "accept") shikisha.log("DLG7 " .. tostring(ok) .. " " .. tostring(e))`);
  check((await said('DLG7')).includes('DLG7 false'), 'answering when nothing is open is an error, not a silent yes');

  console.log('3. leaving a page that asks to be stayed on');
  await press('#stay');
  await until(async () => (await titleNow()) === 'guarded', 'the guard set', 8000);
  await press('#next');
  await until(async () => (await titleNow()) === 'Next', 'the next page', 10000).catch(() => {});
  check((await titleNow()) === 'Next', 'the page is left without a question: ' + await titleNow());
  check(await board.run('!(S.page_dialogs || []).length'), 'and nothing was asked on the board');
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  stopApp();
  server.close();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
