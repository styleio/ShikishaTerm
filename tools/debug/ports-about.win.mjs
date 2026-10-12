/**
 * What the ports panel says about each server, end to end: what it is, where its row opens it,
 * and the name the AI for automatic names gives it -- read off the running app.
 *
 * This checkout's build in a folder of its own, over a home of its own, with
 * one folder (Auto on) and a shell tab in it. The shell starts three small
 * servers from three folders below it:
 *
 *   - apps/admin: bound to 127.0.0.1, prints `Local: http://127.0.0.1:<port>/`
 *   - apps/site:  bound everywhere, prints a localhost and a LAN address
 *   - api:        bound to 127.0.0.1, prints nothing, started with a token
 *
 * The assistant AI is a stand-in `claude.cmd` on the PATH: asked with `-p`, it
 * reads the prompt and answers a name for each port it was told about.
 *
 *     cargo build
 *     node tools/debug/ports-about.win.mjs [--keep]
 *
 * Checked, through the window's own DevTools port:
 *   - each row says the folder its server runs in and what it runs
 *   - a server's printed address is where its row opens; one bound to
 *     127.0.0.1 that printed nothing opens at 127.0.0.1, not localhost
 *   - the command is on the row's tooltip, with the token hidden
 *   - each server is named by the AI, once, and the name heads its row
 *   - the prompt the AI was sent has the printed line and no token
 *
 * Needs Windows and Node. Nothing of a copy somebody is using is read, written
 * or stopped; the app is stopped on the way out unless --keep is given.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9330;
const RUN = path.join(os.tmpdir(), 'sk-ports-about');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const HOME = path.join(RUN, 'home');
const STUB = path.join(RUN, 'stub');
const ASKED = path.join(RUN, 'asked.txt');
const CONFIG = path.join(APP, 'config', 'config.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM','node' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and ($_.Path -like '${RUN}\\*' -or ($_.Name -eq 'node' -and (Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)").CommandLine -like '*${RUN}*')) } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const freePort = () => new Promise((resolve) => {
  const s = net.createServer();
  s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => resolve(p)); });
});

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, HOME, STUB, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });

// One server script, copied into each folder so each runs from its own
const server = String.raw`
const http = require('http');
const arg = (k) => { const i = process.argv.indexOf(k); return i >= 0 ? process.argv[i + 1] : ''; };
const host = arg('--host'), port = Number(arg('--port')), say = arg('--say');
http.createServer((q, r) => r.end('ok')).listen(port, host, () => {
  for (const line of say.split('|').filter(Boolean)) process.stdout.write('  ' + line.replaceAll('{port}', port) + '\r\n');
});
`;
for (const f of ['apps/admin', 'apps/site', 'api']) {
  fs.mkdirSync(path.join(WORK, f), { recursive: true });
  fs.writeFileSync(path.join(WORK, f, 'server.js'), server);
}
fs.writeFileSync(path.join(WORK, 'apps', 'admin', 'package.json'), JSON.stringify({ name: '@shop/admin' }));

// The stand-in assistant AI: a name for each port the prompt tells it about
fs.writeFileSync(path.join(STUB, 'claude.js'), String.raw`
const fs = require('fs');
if (process.argv.includes('--version')) { console.log('2.1.0 (Claude Code)'); process.exit(0); }
let text = '';
process.stdin.on('data', (d) => { text += d; });
process.stdin.on('end', () => {
  fs.appendFileSync(process.env.SK_PORTS_ASKED, text + '\n=====\n');
  const named = {};
  for (const m of text.matchAll(/^Port (\d+)$/gm)) named[m[1]] = 'Stub name ' + m[1];
  process.stdout.write(JSON.stringify(named));
});
`);
fs.writeFileSync(path.join(STUB, 'claude.cmd'), `@"${process.execPath}" "${path.join(STUB, 'claude.js')}" %*\r\n`);

const [admin, site, api] = [await freePort(), await freePort(), await freePort()];

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  ai_engine: 'claude',
  desks: [{ name: 'Ports', id: 'ports', folders: [{ cwd: WORK, auto_label: true, tabs: [
    { name: 'shell', id: 'shell', command: 'cmd.exe' },
  ] }] }],
}, null, 2));

const starter = path.join(RUN, 'start.ps1');
fs.writeFileSync(starter, [
  '$ErrorActionPreference = "Stop"',
  `$env:LOCALAPPDATA = "${path.join(RUN, 'localappdata')}"`,
  `$env:USERPROFILE = "${HOME}"`,
  `$env:PATH = "${STUB};" + $env:PATH`,
  `$env:SK_PORTS_ASKED = "${ASKED}"`,
  `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=${PORT}"`,
  'foreach ($e in @(Get-ChildItem env: | Where-Object { $_.Name -match "^(CLAUDE|ANTHROPIC|SHIKISHA)" })) { Remove-Item ("env:" + $e.Name) }',
  `Start-Process -FilePath "${path.join(APP, 'SHIKISHA-TERM.exe')}" -ArgumentList '--behind' -WorkingDirectory "${APP}"`,
].join('\r\n') + '\r\n');
const pages = async () => {
  try {
    const found = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
    return found.filter((t) => t.type === 'page' && /127\.0\.0\.1:\d+\/$/.test(t.url));
  } catch { return []; }
};
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};

let board;
try {
  let targets = [];
  for (let attempt = 1; attempt <= 3 && !targets.length; attempt++) {
    const started = ps('-File', starter);
    if (started.status !== 0) die('the copy did not start:\n' + started.stdout + started.stderr);
    const end = Date.now() + 45000;
    while (Date.now() < end && !(targets = await pages()).length) await sleep(500);
    if (!targets.length) { console.log(`  (start ${attempt} came up without its DevTools port -- again)`); stopApp(); await sleep(2000); }
  }
  if (!targets.length) throw new Error('the app\'s page never opened its DevTools port');
  board = await connectCdp(targets[0]);
  const { run } = board;
  await until(() => run(`!!(S && S.tabs && S.tabs.find(t => t.id === "shell"))`), 'the shell tab');
  const shell = await run(`S.tabs.find(t => t.id === "shell").index`);
  await sleep(2000);

  console.log('1. three servers started from the shell');
  const start = (dir, host, port, say, extra = '') =>
    `start "" /b node "${path.join(WORK, dir, 'server.js')}" --host ${host} --port ${port} --say "${say}" ${extra}`;
  const line = [
    `cd /d "${path.join(WORK, 'apps', 'admin')}" && ` + start('apps/admin', '127.0.0.1', admin, '➜  Local:   http://127.0.0.1:{port}/'),
    `cd /d "${path.join(WORK, 'apps', 'site')}" && ` + start('apps/site', '0.0.0.0', site, '➜  Local:   http://localhost:{port}/|➜  Network: http://192.168.99.99:{port}/'),
    `cd /d "${path.join(WORK, 'api')}" && ` + start('api', '127.0.0.1', api, '', '--token sekrit-value-123'),
  ].join(' & ');
  await run(`send({kind:"say", tab:${shell}, text:${JSON.stringify(line)}}); true`);
  const about = (port) => run(`JSON.stringify(((S.tabs.find(t => t.id === "shell").place || {}).about || {})[${port}] || null)`).then(JSON.parse);
  await until(async () => (await about(admin)) && (await about(site)) && (await about(api)), 'the three ports in the state', 30000);

  console.log('2. what each one is, and where it opens');
  const a = await about(admin), s = await about(site), p = await about(api);
  check(a.what === 'apps/admin · server.js', 'admin reads ' + JSON.stringify(a.what));
  check(s.what === 'apps/site · server.js', 'site reads ' + JSON.stringify(s.what));
  check(p.what === 'api · server.js', 'api reads ' + JSON.stringify(p.what));
  check(a.url === `http://127.0.0.1:${admin}`, 'admin opens where it said: ' + a.url);
  check(s.url === `http://localhost:${site}`, 'site, bound everywhere, opens at its loopback address: ' + s.url);
  check(!p.url, 'api printed nothing, so no address is taken for it');
  const host = await run(`(S.tabs.find(t => t.id === "shell").place.hosts || {})[${api}] || ""`);
  check(host === '127.0.0.1', 'api, bound to 127.0.0.1 alone, opens at 127.0.0.1: ' + JSON.stringify(host));
  check(p.command.includes('--token ***') && !p.command.includes('sekrit'), 'the token is hidden: ' + p.command);

  console.log('3. named by the AI for automatic names');
  await until(async () => (await about(admin)).name && (await about(site)).name && (await about(api)).name, 'the names', 60000);
  check((await about(admin)).name === `Stub name ${admin}`, 'admin is named');
  // Only the asks about ports: the command typed into the shell is a request
  // like any other, and the folder's own name is asked for from it
  const asks = fs.readFileSync(ASKED, 'utf8').split('\n=====\n').filter((x) => x.includes('listening on ports'));
  const asked = asks.join('\n');
  check(asks.length === 1, `the three were named in one ask (${asks.length})`);
  check(asked.includes('Local: http://127.0.0.1') || asked.includes('printed beside its address: Local:'), 'the printed line was sent');
  check(asked.includes('@shop/admin'), 'the project name was sent');
  check(!asked.includes('sekrit'), 'the token was not sent');

  console.log('4. the panel');
  await run(`sideReveal("ports"); true`);
  await until(() => run(`document.querySelectorAll("#portpanel .prt").length >= 3`), 'the rows');
  const rows = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll("#portpanel .prt")].map(r => ({
    port: r.querySelector(".pp").textContent, head: r.querySelector(".pg").textContent,
    under: r.querySelector(".po").textContent, tip: r.title })))`));
  const row = (port) => rows.find((r) => r.port === ':' + port) || {};
  check(row(admin).head === `Stub name ${admin}`, 'the name heads the row: ' + row(admin).head);
  check(row(admin).under.includes('127.0.0.1') && row(admin).under.includes('apps/admin · server.js'),
    'under it, the address and what runs: ' + row(admin).under);
  check(!row(site).under.includes('localhost'), 'localhost is not said: ' + row(site).under);
  check(row(api).tip.includes('--token ***'), 'the tooltip has the command: ' + row(api).tip.replace(/\n/g, ' / '));
  // The address a press opens: the same one the row was drawn with
  // (what the page hands the window is caught on its way out, not passed on)
  const target = await run(`(() => { let got = ""; const w = window.chrome.webview, was = w.postMessage;
    w.postMessage = (m) => { try { const o = typeof m === "string" ? JSON.parse(m) : m; got = o.target || got; } catch {} };
    try { [...document.querySelectorAll("#portpanel .prt")].find(r => r.querySelector(".pp").textContent === ":${admin}").click(); }
    finally { w.postMessage = was; } return got; })()`);
  check(target === `http://127.0.0.1:${admin}/`, 'a press opens ' + target);
} catch (e) {
  failures += 1;
  console.error('stopped: ' + e.message);
} finally {
  try { board?.ws.close(); } catch {}
  if (!process.argv.includes('--keep')) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
