/**
 * An AI tab hands work to a terminal and to a web page, the way the @ in the
 * input bar lets a person ask it to.
 *
 * This checkout's build in a folder of its own, with a real Claude Code tab
 * (the skill in its folder as a project skill, so the real home is not
 * touched), a terminal whose environment alone holds a value, and a browser
 * tab showing a small form served here, driven by its 🗣 run with
 * `@claude/haiku` choosing and writing (agreed to in the copy's settings).
 * What the person asks is typed the way the input bar sends it, so the
 * terminal and the page count as named by the person.
 *
 *     cargo build
 *     node tools/debug/tab-verbs.win.mjs
 *
 * Checked, in three real turns of Claude Code (and the page run's own calls):
 *   run      "run echo %VALUE% in <@shell>"   -- the value only that terminal has comes back
 *   unnamed  the same without naming it     -- nothing is typed into the terminal, and the AI says why
 *   do       "on <@form>, send the name Alice" -- the form's server is sent name=Alice, and the AI reports the page
 *
 * Needs Windows, Node, and `claude` signed in. Spends a few turns of that
 * account. Nothing of a copy somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9353;
const RUN = path.join(os.tmpdir(), 'sk-tab-verbs');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const VALUE = 'V' + Math.random().toString(36).slice(2, 8).toUpperCase();

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-tab-verbs*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

// ── The page ──────────────────────────────────
let sent = null;
const server = http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  if (u.pathname === '/thanks') {
    sent = u.searchParams.get('name');
    res.end(`<!doctype html><html><body><h1>Thanks, ${sent}. Your number is 4417.</h1></body></html>`);
    return;
  }
  res.end(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Sign-up</title></head>
<body><h1>Sign-up</h1><form action="/thanks" method="get">
<label>Name <input name="name" type="text"></label>
<button type="submit">Send</button></form></body></html>`);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;

// ── The copy ──────────────────────────────────
console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
const skillText = spawnSync(appExe, ['--cli', 'skill'], { encoding: 'utf8' }).stdout;
if (!skillText.includes('shikisha run ID')) die('the app did not print the skill with run and do:\n' + skillText);
const skillAt = path.join(WORK, '.claude', 'skills', 'shikisha');
fs.mkdirSync(skillAt, { recursive: true });
fs.writeFileSync(path.join(skillAt, 'SKILL.md'), skillText);
spawnSync('git', ['init', '-q'], { cwd: WORK });

fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  external_api: { access: 'user' },
  agreed: { '@claude': ['pages'] },
  desks: [{ name: 'Verbs', id: 'verbs', folders: [{ cwd: WORK, tabs: [
    { name: 'claude', id: 'claude', command: ['claude', '--dangerously-skip-permissions'] },
    // The value is in this terminal's environment and nowhere else
    { name: 'shell', id: 'shell', command: ['cmd.exe', '/k', `set VALUE=${VALUE}`] },
    { name: 'form', id: 'form', command: `browser http://127.0.0.1:${pagePort}/`,
      choose_model: '@claude/haiku', words_model: '@claude/haiku' },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
const child = spawn(appExe, [], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

// ── Doors: the pipe to read tabs, the page to type as the person ──────────
let door;
const openDoor = async () => {
  const tokenFile = path.join(APP, 'data', 'api-token');
  const end = Date.now() + 60000;
  while (!fs.existsSync(tokenFile) && Date.now() < end) await sleep(500);
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); }
  });
  const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
  await line({ token: fs.readFileSync(tokenFile, 'utf8').trim() });
  let n = 0;
  door = async (method, ...params) => {
    const a = await line({ id: String(++n), method, params });
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
};
const pages = async () => {
  try { return (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { return []; }
};
let board;
const connect = async (target) => {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => { const m = JSON.parse(e.data); if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); } });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  return { ws, run };
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(700); }
  throw new Error('timed out waiting for ' + what);
};
const screen = (id) => door('tab_screen', id).then((s) => String(s || ''));
const logSince = (from) => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/).slice(from) : []);
const logLen = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/).length : 0);

/** What the person types, as the input bar sends it */
const say = async (text) => {
  const idx = await board.run(`S.tabs.find(t => t.id === "claude").index`);
  await board.run(`send({kind:"say", tab:${idx}, text:${JSON.stringify(text)}}); true`);
};
/** Waits for Claude to take a request up and come back to rest with `got` on its screen */
const answered = async (got, ms) => {
  const start = Date.now();
  let busy = false;
  let quiet = null;
  while (Date.now() - start < ms) {
    const st = await door('state', 'claude').catch(() => '?');
    const s = await screen('claude');
    if (st === 'BUSY') { busy = true; quiet = null; }
    if (got(s) && st !== 'BUSY') return true;
    if (busy && ['DONE', 'WAIT'].includes(st)) { quiet ??= Date.now(); if (Date.now() - quiet > 25000) return false; }
    await sleep(1000);
  }
  return false;
};

try {
  await openDoor();
  let targets = [];
  await until(async () => (targets = await pages()).length > 0, 'the app\'s page', 30000);
  board = await connect(targets.find((t) => !t.url.startsWith('http://127.0.0.1:' + pagePort)) || targets[0]);
  // Claude Code up, its folder trusted
  await until(async () => {
    const s = await screen('claude');
    if (/I trust this folder/i.test(s)) { await door('send', 'claude', '\x1b[B'); await sleep(300); await door('send', 'claude', '\r'); }
    return /bypass permissions|for shortcuts/i.test(s);
  }, 'Claude Code to come up', 120000);
  await sleep(3000);

  let from = 0;
  // --only=do goes straight to the page
  const ONLY = (process.argv.find((a) => a.startsWith('--only=')) || '').slice(7);
  if (!ONLY || ONLY === 'run') {
  console.log('1. run: a command in the terminal the person named');
  from = logLen();
  await say(`Run the command \`echo %VALUE%\` in <@shell> and tell me exactly what it printed.`);
  const ran = await answered((s) => s.includes(VALUE), 6 * 60000);
  check(ran, `the value only that terminal has came back: ${VALUE}`);
  check(logSince(from).some((l) => /ask_tab: \S+ asks shell/.test(l)), 'it went through tab_run');

  console.log('2. unnamed: the same terminal, not named');
  from = logLen();
  const before = await door('tab_read', 'shell', 0).then((r) => r[1]).catch(() => 0);
  await say('Now run the command `echo again` in the terminal tab whose id is shell, using shikisha. Tell me what happened.');
  await answered((s) => /not named|name it|named/i.test(s), 4 * 60000);
  const after = await door('tab_read', 'shell', before).then((r) => r[0]).catch(() => '');
  check(!/again/.test(after), 'nothing was typed into the terminal');
  check(/not named|name it with @|named/i.test(await screen('claude')), 'the AI says the person has to name it');

  }
  console.log('3. do: a page the person named');
  from = logLen();
  await say(`On <@form>, type the name Alice and send the form. Then tell me what the page says.`);
  const did = await answered((s) => /4417/.test(s), 8 * 60000);
  check(sent === 'Alice', 'the form\'s server was sent name=' + sent);
  check(did, 'the AI reported what the page showed (4417)');
  check(logSince(from).some((l) => /browser_do: \S+ drives form/.test(l)), 'it went through browser_do');
} catch (e) {
  failures += 1;
  console.error('stopped: ' + e.message);
} finally {
  try { board?.ws.close(); } catch {}
  server.close();
  if (!process.argv.includes('--keep')) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
