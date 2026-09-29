/**
 * The bridge on another machine, end to end: put there because the settings
 * say the person agreed, used by an AI tab there to report a task handed to it
 * from here, and taken off again when the agreement is.
 *
 * This checkout's build in a folder of its own, with a stand-in lead here and a
 * stand-in AI on the other machine (a `claude` script there, so the app takes
 * it for Claude Code). The lead hands the far AI a task; the far AI runs
 * `shikisha report` there -- which exists only because the bridge is there --
 * and the report reaches the lead here.
 *
 *     cargo build
 *     node tools/debug/bridge-far.win.mjs --where=ssh     (or --where=vm)
 *
 * The bridge program is taken from bridge/ beside the build (dist.list): put a
 * Linux build there first, e.g. with cargo zigbuild in WSL.
 *
 * Checked:
 *   1. with the machine agreed to, the bridge is put there and connected
 *   2. the far AI's program is started with the command on its PATH
 *   3. the task goes there, and the far AI's report comes back here
 *   4. the far AI's conversation is read by the bridge, not piece by piece
 *   5. once the app lets go, nothing of the bridge is left running there
 *   6. unticked, the bridge's folder is deleted there
 *
 * Needs Windows, Node, and in .private/.env the test server (SSH_TEST_*) or
 * E2B_API_TOKEN. No AI account is used. Everything put there is removed.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { createRequire } from 'node:module';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const WHERE = (process.argv.find((a) => a.startsWith('--where=')) || '--where=ssh').slice(8);
const RUN = path.join(os.tmpdir(), 'sk-bridge-far');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const WORK = path.join(RUN, 'work');
const STUB = path.join(RUN, 'stub');
const LOGS = path.join(RUN, 'logs');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const MARK = 'shikisha-bridge-far-' + process.pid;
const BRIDGE_DIR = '.local/share/shikisha/bridge';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const note = (what) => console.log('  .... ' + what);
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '' + $_.ExecutablePath) -like '*${path.basename(RUN)}*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(700); }
  throw new Error('timed out waiting for ' + what);
};
const logSince = (from) => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/).slice(from) : []);
const logLen = () => logSince(0).length;

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
// --exe=<path> runs another build instead of this checkout's: the one
// installed, say, to try exactly what people will run. What it reads beside
// itself -- the bridge among it -- then comes from beside it too, not from
// this checkout
const installed = (process.argv.find((a) => a.startsWith('--exe=')) || '').slice(6);
const exe = installed || path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
const fromCopy = installed ? ['-From', path.dirname(installed)] : [];
const payload = installed ? path.dirname(installed) : ROOT;
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (!fs.existsSync(path.join(payload, 'bridge', 'shikisha-bridge-x86_64-linux'))) die(`no Linux bridge in ${path.join(payload, 'bridge')} -- ${installed ? 'the installed copy is missing it' : 'build one first'}`);

// The far AI: reads lines; on its brief, reports through `shikisha` and writes
// down what it was told. Keeps a record where Claude Code would, so reading
// its conversation has something to read
const STAND_IN = `#!/bin/sh
id=""
while [ $# -gt 0 ]; do case "$1" in --session-id|--resume) id="$2"; shift;; esac; shift; done
rec="$HOME/.claude/projects/${MARK}/$id.jsonl"
mkdir -p "$(dirname "$rec")"
log="$HOME/${MARK}/said.txt"
{ echo "PATH=$PATH"; echo "SOCK=$SHIKISHA_BRIDGE_SOCK"; echo "KEY=$SHIKISHA_KEY_FILE"; command -v shikisha; } >> "$log"
echo "stand-in claude"
while read -r line; do
  printf '{"type":"user","message":{"role":"user","content":"%s"}}\\n' "brief line" >> "$rec"
  case "$line" in
    *"--- t"*)
      sleep 1
      shikisha report done "far did it." "nothing odd." "nothing" >> "$log" 2>&1
      echo "report exit $?" >> "$log"
      printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]}}\\n' "reported from far" >> "$rec"
      echo "DONE-FAR";;
  esac
done
`;

let there, farHost, farFolder, farSecrets, HOME;
let cleanup = async () => {};
if (WHERE === 'ssh') {
  const HOST = dotenv.SSH_TEST_HOST, PORT = Number(dotenv.SSH_TEST_PORT || 22), USER = dotenv.SSH_TEST_USER;
  const PASSWORD = dotenv.SSH_TEST_PASSWORD, KEY = dotenv.SSH_TEST_KEY;
  if (!HOST || !USER || !(PASSWORD || KEY)) die('SSH_TEST_HOST, SSH_TEST_USER and a password or key are needed in .private/.env');
  const sdk = path.join(ROOT, 'target', 'ssh2-sdk');
  if (!fs.existsSync(path.join(sdk, 'node_modules', 'ssh2'))) {
    fs.mkdirSync(sdk, { recursive: true });
    spawnSync('npm', ['init', '-y'], { cwd: sdk, shell: true });
    spawnSync('npm', ['i', 'ssh2', '--silent'], { cwd: sdk, shell: true });
  }
  const { Client } = createRequire(path.join(sdk, 'package.json'))('ssh2');
  there = (cmd) => new Promise((resolve) => {
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
  HOME = (await there('printf %s "$HOME"')).trim();
  if (!HOME.startsWith('/')) die('the server did not answer: ' + HOME);
  farHost = { name: 'srv', at: `ssh://${USER}@${HOST}:${PORT}`, ...(KEY ? { key: KEY } : {}) };
  farFolder = { host: 'srv' };
  farSecrets = PASSWORD ? { 'ssh/host/srv/password': PASSWORD } : {};
} else {
  const KEY = dotenv.E2B_API_TOKEN;
  if (!KEY) die('E2B_API_TOKEN is needed in .private/.env');
  const dir = path.join(ROOT, 'target', 'e2b-sdk');
  const entry = path.join(dir, 'node_modules', 'e2b', 'dist', 'index.mjs');
  if (!fs.existsSync(entry)) {
    fs.mkdirSync(dir, { recursive: true });
    spawnSync('npm', ['init', '-y'], { cwd: dir, shell: true });
    spawnSync('npm', ['i', 'e2b', '--silent'], { cwd: dir, shell: true });
  }
  const { Sandbox } = await import('file://' + entry.replace(/\\/g, '/'));
  const made = await (await fetch('https://api.e2b.app/sandboxes', {
    method: 'POST', headers: { 'X-API-Key': KEY, 'Content-Type': 'application/json' },
    body: JSON.stringify({ templateID: 'base', timeout: 900, autoPause: true, autoResume: { enabled: true },
      metadata: { shikisha: '1', project: 'bridge-far-check' } }),
  })).json();
  if (!made.sandboxID) die('no machine: ' + JSON.stringify(made));
  const box = await Sandbox.connect(made.sandboxID, { apiKey: KEY });
  there = async (cmd) => {
    const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
    return r.stdout + r.stderr;
  };
  HOME = '/home/user';
  farHost = { name: 'vm', kind: 'e2b', template: 'base', minutes: 15 };
  farFolder = { host: 'vm', sandbox: box.sandboxId };
  farSecrets = { e2b_api_key: KEY };
  cleanup = async () => {
    await fetch('https://api.e2b.app/sandboxes/' + box.sandboxId, { method: 'DELETE', headers: { 'X-API-Key': KEY } }).catch(() => {});
  };
}
const DIR = `${HOME}/${MARK}`;
const b64 = (s) => Buffer.from(s).toString('base64');

// The lead, here: the same kind of stand-in as orch.win.mjs, told to hand one
// task to the far AI and see it through
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, WORK, STUB, LOGS]) fs.mkdirSync(d, { recursive: true });
fs.writeFileSync(path.join(STUB, 'lead.js'), String.raw`
const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const logs = process.argv[2];
const said = path.join(logs, 'said-lead.jsonl');
const bin = (process.env.PATH || process.env.Path || '').split(';')[0];
const app = fs.readFileSync(path.join(bin, 'shikisha.cmd'), 'utf8').match(/"([^"]+)"/)[1];
const sh = (...args) => {
  const r = spawnSync(app, ['--cli', ...args], { encoding: 'utf8', env: process.env });
  const out = { args, code: r.status, out: (r.stdout || '').trim(), err: (r.stderr || '').trim() };
  fs.appendFileSync(said, JSON.stringify(out) + '\n');
  return out;
};
const json = (o) => { try { return JSON.parse(o.out.split('\n\n[shikisha]')[0]); } catch { return {}; } };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdout.write('\x1b[?2004hlead ready\r\n> ');
let buf = '';
process.stdin.on('data', async (d) => {
  buf += d.toString('utf8');
  if (!buf.includes('GO')) return;
  buf = '';
  sh('set_state', 'BUSY');
  sh('job_open', 'Have the far AI do one thing');
  sh('task_add', 'Say that you are there, then report.');
  let sent = null;
  for (let n = 0; n < 40 && !sent; n++) {
    const r = sh('assign', 't1', 'farai');
    if (r.code === 0) sent = r; else await sleep(5000);
  }
  let dealt = null;
  for (let n = 0; n < 8; n++) {
    const v = json(sh('inbox', 'wait', ...(dealt ? [JSON.stringify({ dealt })] : [])));
    dealt = v.handover || null;
    if ((v.mail || []).some((m) => m.kind === 'report')) break;
  }
  if (dealt) sh('inbox', JSON.stringify({ dealt }));
  sh('keep', 'a1');
  sh('job_close', 'The far AI reported');
  sh('set_state', 'DONE');
});
`);
fs.writeFileSync(path.join(STUB, 'claude.cmd'), `@"${process.execPath}" "${path.join(STUB, 'lead.js')}" "${LOGS}"\r\n`);

const leadSaid = () => {
  const f = path.join(LOGS, 'said-lead.jsonl');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)) : [];
};
const writeConfig = (bridges) => fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  resident: false,
  external_api: { access: 'user' },
  hosts: [farHost],
  bridges,
  desks: [{ name: 'Far', id: 'far', folders: [
    { cwd: WORK, tabs: [{ name: 'lead', id: 'lead', command: [path.join(STUB, 'claude.cmd')] }] },
    { cwd: DIR, ...farFolder, tabs: [{ name: 'farai', id: 'farai', command: `${DIR}/bin/claude` }] },
  ] }],
}, null, 2));

let door;
const openDoor = async (pid) => {
  const tokenFile = path.join(APP, 'data', 'api-token');
  await until(() => fs.existsSync(tokenFile), 'the api token', 60000);
  const token = fs.readFileSync(tokenFile, 'utf8').trim();
  door = async (method, ...params) => {
    const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
    await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
    let b = '';
    const lines = [];
    let wake = null;
    sock.on('data', (d) => {
      b += d.toString('utf8');
      let i;
      while ((i = b.indexOf('\n')) >= 0) { lines.push(JSON.parse(b.slice(0, i))); b = b.slice(i + 1); if (wake) wake(); }
    });
    const next = async () => { while (!lines.length) await new Promise((r) => { wake = r; }); return lines.shift(); };
    sock.write(JSON.stringify({ token }) + '\n');
    await next();
    sock.write(JSON.stringify({ id: '1', method, params }) + '\n');
    const a = await next();
    sock.end();
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
};
const screen = (id) => door('tab_screen', id).then((s) => String(s || '')).catch(() => '');
// The window's page, for the person's own words through the input bar
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
  return (expression) => new Promise((res) => {
    const id = ++n;
    waiting.set(id, (m) => res(m.result && m.result.result ? m.result.result.value : undefined));
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, returnByValue: true, awaitPromise: true } }));
    setTimeout(() => res(undefined), 8000);
  });
};

try {
  console.log(`the other machine (${WHERE}): ` + (await there('uname -srm')).trim());
  await there(`rm -rf ${HOME}/${BRIDGE_DIR}; mkdir -p ${DIR}/bin && printf %s ${b64(STAND_IN)} | base64 -d > ${DIR}/bin/claude && chmod +x ${DIR}/bin/claude`);
  stopApp();
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe, ...fromCopy);
  const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
  if (staged.status !== 0 || !fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
  check(fs.existsSync(path.join(APP, 'bridge', 'shikisha-bridge-x86_64-linux')), 'the bridge travels beside the app (dist.list)');
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  writeConfig([farHost.name]);
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: farSecrets }, null, 2));
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
  const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
  child.unref();
  await openDoor(child.pid);
  // A server's terminal opens at once; a MicroVM's waits until something needs
  // it -- here, the task handed to it
  if (WHERE === 'ssh') {
    await until(async () => (await screen('farai')).includes('stand-in claude'), 'the far AI to start', 120000)
      .catch(async () => note('farai: ' + (await screen('farai')).slice(-300)));
  }
  // The person's words, through the input bar: the lead may hand work to farai
  const board = await boardOf();
  await until(() => board('!!(S && S.tabs && S.tabs.some(t => t.id === "lead"))'), 'the tabs on the page', 30000);
  const idx = await board('S.tabs.find(t => t.id === "lead").index');
  const from = logLen();
  await board(`(send({kind:"say", tab:${idx}, text:"GO <@farai>"}), true)`);

  console.log('1. agreed to, the bridge is put there and connected');
  await until(() => logSince(0).some((l) => l.includes('bridge: connected to')), 'the bridge to connect', 180000)
    .catch(() => note('log: ' + logSince(0).filter((l) => l.includes('bridge')).slice(-5).join(' | ')));
  const listing = await there(`ls -la ${HOME}/${BRIDGE_DIR} ${HOME}/${BRIDGE_DIR}/bin ${HOME}/${BRIDGE_DIR}/keys 2>&1`);
  check(/shikisha-bridge-\d/.test(listing) && /shikisha\b/.test(listing), 'the program and the tabs\' command are there');
  check(/drwx------.*keys|^d.{9}/m.test(listing) && /-rw-------/.test(listing), 'the keys are readable by this account alone');
  if (failures) note(listing);

  console.log('2. the far AI was started with the command on its PATH');
  let farSaid = '';
  await until(async () => (farSaid = await there(`cat ${DIR}/said.txt 2>&1`)).includes('SOCK='), 'the far AI to start', 120000).catch(() => {});
  check(farSaid.includes(`${BRIDGE_DIR}/bin`) && /SOCK=.*shikisha\.sock/.test(farSaid), 'PATH and the socket were given: ' + farSaid.split('\n').slice(0, 3).join(' | '));

  console.log('3. the task goes there, and the far AI\'s report comes back');
  if (WHERE === 'vm') check(logSince(from).some((l) => l.includes('opened farai')), 'the MicroVM was opened for its task');
  await until(() => leadSaid().some((s) => s.args[0] === 'job_close'), 'the lead to close the job', 240000)
    .catch(() => note('lead said: ' + JSON.stringify(leadSaid()).slice(0, 1500)));
  const assigned = leadSaid().filter((s) => s.args[0] === 'assign');
  check(assigned.some((d) => d.code === 0), 'the task went to the far AI' + (assigned.length ? ' (' + assigned.map((d) => d.code === 0 ? 'ok' : d.err.slice(0, 80)).join('; ') + ')' : ''));
  const farLog = await there(`cat ${DIR}/said.txt 2>&1`);
  check(/"state": "reported"|reported/.test(farLog) && /report exit 0/.test(farLog), 'shikisha report ran there and was taken: ' + farLog.split('\n').filter((l) => /report|state/.test(l)).join(' | ').slice(0, 200));
  check(logSince(from).some((l) => /orchestration: a\d+ reported done/.test(l)), 'the report reached the job here');
  const closed = leadSaid().find((s) => s.args[0] === 'job_close');
  check(closed && closed.code === 0, 'the job was closed');

  console.log('4. its conversation is read by the bridge');
  const lf = logLen();
  const conv = await door('tab_conversation', 'farai', { want: 2 }).catch((e) => ({ error: e.message }));
  note('conversation: ' + JSON.stringify(conv).slice(0, 300));
  check((conv.turns || []).some((t) => t.text.includes('reported from far')), 'what the far AI said is read');
  check(!logSince(lf).some((l) => l.includes('reading it the long way')), 'read in one request by the bridge');

  console.log('5. once the app lets go, nothing of the bridge runs there');
  stopApp();
  // Over SSH the line ends with the app; on a MicroVM the bridge stops
  // hearing it and exits a minute later
  // Counted by state: a program that has ended but whose parent has not yet
  // collected it (Z) is not running
  const live = async () => (await there('ps -eo stat=,args= | grep "[s]hikisha-bridge-[0-9]" | grep -v "^Z"')).trim();
  let running = '';
  await until(async () => (running = await live()) === '', 'the bridge to exit', WHERE === 'ssh' ? 15000 : 120000).catch(() => {});
  check(running === '', 'no bridge process is left' + (running ? ': ' + running : ''));

  console.log('6. unticked, the bridge comes off the next time the machine is in use');
  writeConfig([]);
  const again = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
  again.unref();
  if (WHERE === 'vm') {
    // A person looks at the MicroVM's tab: the machine is in use again
    const b2 = await boardOf();
    await until(() => b2('!!(S && S.tabs && S.tabs.some(t => t.id === "farai"))'), 'the tabs again', 60000);
    const fi = await b2('S.tabs.find(t => t.id === "farai").index');
    await b2(`(send({kind:"select", tab:${fi}}), true)`);
  }
  await until(async () => !(await there(`test -d ${HOME}/${BRIDGE_DIR} && echo yes`)).includes('yes'), 'the folder to go', 180000)
    .catch(() => {});
  check(!(await there(`test -d ${HOME}/${BRIDGE_DIR} && echo yes`)).includes('yes'), 'the bridge\'s folder is deleted');
} catch (e) {
  failures += 1;
  console.error('stopped: ' + e.message);
} finally {
  stopApp();
  await there(`rm -rf ${DIR} ${HOME}/.claude/projects/${MARK} ${HOME}/${BRIDGE_DIR}`).catch(() => {});
  await cleanup();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
