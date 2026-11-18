/**
 * A page in the window app whose own process dies is opened again at its
 * address -- and a page reached by sending a form does not send it again.
 *
 * This checkout's build in a folder of its own, with a small site served
 * here in a browser tab. The page sends a form; its process is then made to
 * die through the pages' debugging port (which only this check turns on).
 *
 *     cargo build
 *     node tools/debug/page-crash.win.mjs
 *
 * Checked:
 *   back     the page is there again, at the address it had moved to
 *   once     the form was sent once: what came back is asked for, not sent
 *   said     the log says what was done
 *
 * Needs Windows and Node. Nothing of a copy somebody is using is read,
 * written or stopped.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-page-crash');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');

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

// The site: a form, and what sending it answers with (at an address of its own)
const asked = { posts: 0, results: 0 };
const server = http.createServer((req, res) => {
  const html = (body) => {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
    res.end(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Order</title></head><body>${body}</body></html>`);
  };
  if (req.method === 'POST') {
    asked.posts += 1;
    req.resume();
    res.writeHead(303, { location: '/done' });
    return res.end();
  }
  if (req.url.startsWith('/done')) { asked.results += 1; return html('<h1 id="done">Ordered</h1>'); }
  html('<form method="post" action="/order"><button id="go">Order</button></form>');
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const site = `http://127.0.0.1:${server.address().port}`;

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Crash', id: 'crash', folders: [{ cwd: WORK, tabs: [
    { name: 'shop', id: 'shop', command: `browser ${site}/` },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const portOf = () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'profiles', 'default', 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const shopPage = async () => {
  const p = portOf();
  if (!p) return null;
  const list = await (await fetch(`http://127.0.0.1:${p}/json/list`)).json();
  return list.find((t) => t.type === 'page' && t.url.startsWith(site)) || null;
};

try {
  let target;
  await until(async () => (target = await shopPage()), 'the shop page', 60000);
  let page = await connectCdp(target);
  await until(() => page.run('!!document.getElementById("go")'), 'the form');

  console.log('1. the form is sent');
  await page.run('document.getElementById("go").click()');
  await until(() => asked.posts === 1 && asked.results === 1, 'the order and what it answered');
  await until(async () => (await shopPage())?.url === `${site}/done`, 'the page at its answer');
  page.ws.close();

  console.log('2. the page\'s process dies');
  target = await shopPage();
  page = await connectCdp(target);
  const before = { ...asked };
  page.send('Page.crash').catch(() => {});
  await sleep(500);
  try { page.ws.close(); } catch {}

  await until(() => asked.results > before.results, 'the page asked for again', 20000).catch(() => {});
  check(asked.results === before.results + 1, `the page was opened again at its address (${before.results} -> ${asked.results})`);
  check(asked.posts === 1, `the form was sent once (${asked.posts})`);
  let again;
  await until(async () => (again = await shopPage()) && again.url === `${site}/done`, 'the page there again', 10000).catch(() => {});
  if (again) {
    const back = await connectCdp(again);
    const shown = await back.run('document.getElementById("done") && document.getElementById("done").textContent').catch(() => null);
    check(shown === 'Ordered', 'it shows what it showed: ' + shown);
    back.ws.close();
  } else {
    check(false, 'the page is listed again');
  }
  const log = fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '';
  check(/the process of the page '[^']*shop' died; it is opened again at its address/.test(log), 'the log says what was done');
} catch (e) {
  failures += 1;
  console.error(e);
} finally {
  stopApp();
  server.close();
}
console.log(failures ? `${failures} FAILED` : 'all passed');
process.exit(failures ? 1 : 0);
