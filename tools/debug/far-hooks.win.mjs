/**
 * An AI's hook on a server is written only after the person says yes, and
 * comes out again when they say no -- checked on a real server, through the
 * running app's own window.
 *
 *   1. an AI tab on the server brings up the question, naming the server and
 *      the file there, and nothing is written before it is answered; "allow"
 *      writes one entry, the person's own settings kept, the file as it was
 *      beside it as .bak, and the answer kept under the server's name
 *   2. the answer turned to "off" in the settings (what unticking it on the
 *      server's page saves) takes the entry out again, the rest of the file
 *      as it was
 *   3. a server an earlier version wrote into without asking is asked about
 *      once -- keep or take out -- with the file untouched until then, and
 *      "take out" takes it out
 *
 * The server is given a stand-in `claude` in a folder of this run's own. The
 * hook goes where Claude Code's settings are on that account
 * (`~/.claude/settings.json`): what is there is kept aside first and put back
 * on the way out, with its `.bak` beside it, so the account is left as found.
 * This PC's own CLIs are marked answered ("off") in the copy's settings, so
 * the question about this PC never comes up and nothing of this PC is touched.
 *
 *     cargo build
 *     node tools/debug/far-hooks.win.mjs
 *
 * Needs Windows, Node, and the server ssh-flow.win.mjs uses, in .private/.env
 * (SSH_TEST_HOST, SSH_TEST_PORT, SSH_TEST_USER, SSH_TEST_PASSWORD or
 * SSH_TEST_KEY) -- or another one named in SSH_FAR_HOST, SSH_FAR_PORT,
 * SSH_FAR_USER and SSH_FAR_KEY. That server is shared with other sessions:
 * say so to them before running this.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-far-hooks-' + process.pid);
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(400); }
  throw new Error('timed out waiting for ' + what);
};

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const far = process.env.SSH_FAR_HOST ? {
  host: process.env.SSH_FAR_HOST, port: process.env.SSH_FAR_PORT, user: process.env.SSH_FAR_USER, key: process.env.SSH_FAR_KEY,
} : { host: dotenv.SSH_TEST_HOST, port: dotenv.SSH_TEST_PORT, user: dotenv.SSH_TEST_USER, key: dotenv.SSH_TEST_KEY, password: dotenv.SSH_TEST_PASSWORD };
const HOST = far.host, PORT = Number(far.port || 22), USER = far.user;
const PASSWORD = far.password, KEY = far.key;
if (!HOST || !USER || !(PASSWORD || KEY)) die('SSH_TEST_HOST, SSH_TEST_USER and a password or key are needed in .private/.env');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

const sdk = path.join(ROOT, 'target', 'ssh2-sdk');
if (!fs.existsSync(path.join(sdk, 'node_modules', 'ssh2'))) {
  fs.mkdirSync(sdk, { recursive: true });
  spawnSync('npm', ['init', '-y'], { cwd: sdk, shell: true });
  spawnSync('npm', ['i', 'ssh2', '--silent'], { cwd: sdk, shell: true });
}
const { Client } = createRequire(path.join(sdk, 'package.json'))('ssh2');
const there = (cmd) => new Promise((resolve) => {
  const c = new Client();
  let out = '';
  c.on('ready', () => c.exec(cmd, (err, st) => {
    if (err) { c.end(); resolve('ERR ' + err.message); return; }
    st.on('data', (d) => { out += d; }).stderr.on('data', (d) => { out += d; });
    st.on('close', () => { c.end(); resolve(out); });
  })).on('error', (e) => resolve('ERR ' + e.message))
    .connect({ host: HOST, port: PORT, username: USER, password: PASSWORD || undefined,
      privateKey: KEY ? fs.readFileSync(KEY) : undefined, readyTimeout: 20000 });
});
const b64 = (s) => Buffer.from(s).toString('base64');

const MARK = 'shikisha-far-hooks-' + process.pid;
const HOME = (await there('printf %s "$HOME"')).trim();
if (!HOME.startsWith('/')) die('the server did not answer: ' + HOME);
const DIR = `${HOME}/${MARK}`;
const FOLDER = `${DIR}/proj`;
const FILE = `${HOME}/.claude/settings.json`;
const BAK = `${HOME}/.claude/settings.bak`;
const STAND_IN = '#!/bin/sh\necho "stand-in claude"\nwhile read -r line; do echo "GOT:$line"; done\n';
// The person's own settings, as the file starts in each part
const THEIRS = '{\n  "model": "opus-from-the-person"\n}\n';

// What is there now, kept aside and put back on the way out
const kept = async (f) => {
  const out = (await there(`if [ -e ${f} ]; then echo HAS; base64 < ${f}; fi`)).trim();
  return out.startsWith('HAS') ? out.slice(3).replace(/\s/g, '') : null;
};
const putBack = async (f, b) => there(b === null ? `rm -f ${f}` : `printf %s ${b} | base64 -d > ${f}`);
const read = async (f) => (await there(`cat ${f} 2>/dev/null`));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const start = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
const appPid = () => ps('-Command', `(Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}*' } | Select-Object -First 1).Id`).stdout.trim();

const boardOf = async () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  let target;
  await until(async () => {
    if (!fs.existsSync(f)) return false;
    const port = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
    target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
    return !!target;
  }, 'the window\'s page', 40000);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let n = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => { const m = JSON.parse(e.data); if (waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); } });
  const call = (method, params) => new Promise((res) => { const id = ++n; waiting.set(id, (m) => res(m.result)); ws.send(JSON.stringify({ id, method, params })); });
  const run = (expression) => new Promise((res, rej) => {
    const id = ++n;
    waiting.set(id, (m) => (m.result && m.result.exceptionDetails ? rej(new Error(m.result.exceptionDetails.text))
      : res(m.result && m.result.result ? m.result.result.value : undefined)));
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, returnByValue: true, awaitPromise: true } }));
    setTimeout(() => res(undefined), 8000);
  });
  // A picture of the window, into target/shots, for the screen rules' marking
  run.shot = async (name) => {
    const r = await call('Page.captureScreenshot', { format: 'png' });
    if (!r || !r.data) return;
    const dir = path.join(ROOT, 'target', 'shots');
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, name + '.png'), Buffer.from(r.data, 'base64'));
  };
  return run;
};

// Every CLI a profile names, marked answered for this PC, so the question
// about this PC stays down and nothing on this PC is written or taken out
const localAnswers = Object.fromEntries(fs.readdirSync(path.join(ROOT, 'profiles'))
  .filter((f) => f.endsWith('.json'))
  .map((f) => JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8')).name)
  .filter(Boolean)
  .map((n) => [n, 'off']));
const writeConfig = (farHooks) => {
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'ja',
    remote: { enabled: false },
    resident: false,
    agent_hooks: localAnswers,
    ...(farHooks ? { far_hooks: farHooks } : {}),
    hosts: [{ name: 'srv', at: `ssh://${USER}@${HOST}:${PORT}`, ...(KEY ? { key: KEY } : {}) }],
    desks: [{ name: 'Check', id: 'check', folders: [{
      cwd: FOLDER, host: 'srv',
      tabs: [{ name: 'claude', id: 'ai', command: `${DIR}/bin/claude` }],
    }] }],
  }, null, 2));
};
const answers = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8')).far_hooks || {};
const question = (board) => board('S && S.hook_ask ? JSON.stringify(S.hook_ask) : null').then((s) => (s ? JSON.parse(s) : null));

const wasFile = await kept(FILE);
const wasBak = await kept(BAK);
let ourText = null;
try {
  console.log('the server: ' + (await there('uname -sr')).trim());
  await there(`rm -rf ${DIR}; mkdir -p ${FOLDER} ${DIR}/bin ${HOME}/.claude && printf %s ${b64(STAND_IN)} | base64 -d > ${DIR}/bin/claude && chmod +x ${DIR}/bin/claude`);
  await there(`printf %s ${b64(THEIRS)} | base64 -d > ${FILE}; rm -f ${BAK}`);

  stopApp();
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
  writeConfig(null);
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: PASSWORD ? { 'ssh/host/srv/password': PASSWORD } : {} }, null, 2));

  console.log('1. asked before anything is written; "allow" writes one entry');
  start();
  let board = await boardOf();
  let q;
  await until(async () => (q = await question(board)) && q.machine === 'srv', 'the question about the server', 90000);
  check(q.clis.length === 1 && q.clis[0].name === 'Claude Code' && q.clis[0].file.endsWith('.claude/settings.json'),
    'it names the server, the AI and the file there: ' + JSON.stringify(q.clis.map((c) => [c.name, c.file])));
  check(!q.found, 'not taken for one an earlier version wrote');
  await sleep(800);
  await board.shot('far-hooks-question');
  check((q.clis[0].preview || []).some((l) => l.ours) && (q.clis[0].preview || []).some((l) => !l.ours && l.text.includes('opus-from-the-person')),
    'the file is shown as it would be, the person\'s lines and ours told apart');
  check((await read(FILE)) === THEIRS, 'nothing written before the answer');
  await board(`(send({kind:"agenthooks", answer:"on", seq:${q.seq}}), true)`);
  await until(async () => (await read(FILE)).includes('shikisha --hook'), 'the entry written', 60000);
  ourText = await read(FILE);
  check(JSON.parse(ourText).model === 'opus-from-the-person', 'the person\'s own setting kept');
  check((await read(BAK)) === THEIRS, 'the file as it was is beside it as .bak');
  check(answers().srv?.['Claude Code'] === 'on', 'the answer is kept under the server\'s name: ' + JSON.stringify(answers()));
  check(!(await question(board)), 'the question is down');

  console.log('2. turned off in the settings, the entry comes out');
  const cfg = JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
  cfg.far_hooks.srv['Claude Code'] = 'off';
  fs.writeFileSync(CONFIG, JSON.stringify(cfg, null, 2));
  await until(async () => !(await read(FILE)).includes('shikisha --hook'), 'the entry taken out', 60000)
    .then(() => check(true, 'the entry is taken out'))
    .catch(async () => check(false, 'the entry is taken out: ' + await read(FILE)));
  check(JSON.parse(await read(FILE)).model === 'opus-from-the-person', 'the person\'s own setting still there');
  check(!(await question(board)), 'nothing asked for it');

  console.log('3. written by an earlier version without asking: asked once, keep or take out');
  await board('typeof winAct === "function" && winAct("close")');
  await until(() => !appPid(), 'the app to quit', 30000).catch(() => stopApp());
  stopApp();
  await there(`printf %s ${b64(ourText)} | base64 -d > ${FILE}; rm -f ${BAK}`);
  writeConfig(null);
  start();
  board = await boardOf();
  await until(async () => (q = await question(board)) && q.machine === 'srv', 'the question about the server', 90000);
  check(q.found === true, 'said to be one written without asking');
  await sleep(800);
  await board.shot('far-hooks-found');
  check((await read(FILE)) === ourText, 'left as it is until answered');
  await board(`(send({kind:"agenthooks", answer:"off", seq:${q.seq}}), true)`);
  await until(async () => !(await read(FILE)).includes('shikisha --hook'), 'the entry taken out', 60000)
    .then(() => check(true, '"take out" takes it out'))
    .catch(async () => check(false, '"take out" takes it out: ' + await read(FILE)));
  check(JSON.parse(await read(FILE)).model === 'opus-from-the-person', 'the person\'s own setting kept');
  check(answers().srv?.['Claude Code'] === 'off', 'the answer kept: ' + JSON.stringify(answers()));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + (e.stack || e));
} finally {
  stopApp();
  await putBack(FILE, wasFile);
  await putBack(BAK, wasBak);
  await there(`rm -rf ${DIR}`);
  await sleep(500);
  fs.rmSync(RUN, { recursive: true, force: true });
  console.log('the server is as it was');
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
