/**
 * A folder on a server over SSH, worked on the way a folder on a MicroVM is:
 * what was made for a MicroVM and reaches any machine that is not this one,
 * checked on a real server.
 *
 *   1. a tab with no command is that server's shell, standing in the folder
 *   2. the column's file list and both searches run on the server
 *   3. a file opens in the editor from the server and saves back to it, a
 *      Shift_JIS one byte for byte, and a save over a file changed since it
 *      was read is refused
 *   4. an AI tab starts under a conversation id the app chose, and after a
 *      restart comes back on that conversation, there
 *
 * The server is given a stand-in `claude` in a folder of this run's own, that
 * writes down its arguments and makes the record of its conversation where
 * Claude Code keeps one -- in a folder of that record's own, removed after.
 *
 *     cargo build
 *     node tools/debug/ssh-far.win.mjs
 *
 * Needs Windows, Node, and the server ssh-flow.win.mjs uses, in .private/.env
 * (SSH_TEST_HOST, SSH_TEST_PORT, SSH_TEST_USER, SSH_TEST_PASSWORD or
 * SSH_TEST_KEY) -- or another one named in SSH_FAR_HOST, SSH_FAR_PORT,
 * SSH_FAR_USER and SSH_FAR_KEY, such as the OpenSSH that sftp-server.wsl.sh
 * starts in WSL. Everything it puts on the server is removed on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
// A folder of this run's own: two sessions running this at once must not
// stage over each other or stop each other's app (it is found by this path)
const RUN = path.join(os.tmpdir(), 'sk-ssh-far-' + process.pid);
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

// The server, asked directly
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

const MARK = 'shikisha-far-check-' + process.pid;
const HOME = (await there('printf %s "$HOME"')).trim();
if (!HOME.startsWith('/')) die('the server did not answer: ' + HOME);
const DIR = `${HOME}/${MARK}`;
const FOLDER = `${DIR}/proj`;
const CALLS = `${DIR}/calls.log`;
// Claude Code's own arguments, and where it keeps a conversation's record
const STAND_IN = `#!/bin/sh
echo "$*" >> ${CALLS}
id=""
while [ $# -gt 0 ]; do case "$1" in --session-id|--resume) id="$2"; shift;; esac; shift; done
mkdir -p "$HOME/.claude/projects/${MARK}"
touch "$HOME/.claude/projects/${MARK}/$id.jsonl"
echo "stand-in claude on $id"
while read -r line; do echo "GOT:$line"; done
`;
const b64 = (s) => Buffer.from(s).toString('base64');
const cleanThere = () => there(`rm -rf ${DIR} "$HOME/.claude/projects/${MARK}"; pkill -f "${DIR}/bin/claude" 2>/dev/null; true`);
const calls = async () => (await there(`cat ${CALLS} 2>/dev/null`)).split('\n').filter(Boolean);
const remembered = () => {
  const f = path.join(APP, 'data', 'last-session');
  if (!fs.existsSync(f)) return null;
  return (JSON.parse(fs.readFileSync(f, 'utf8')).desks || []).flatMap((d) => d.tabs || []).find((t) => t.id === 'ai') || null;
};

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const start = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
const appPid = () => ps('-Command', `(Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}*' } | Select-Object -First 1).Id`).stdout.trim();

// The window's page
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
  return (expression) => new Promise((res, rej) => {
    const id = ++n;
    waiting.set(id, (m) => (m.result && m.result.exceptionDetails ? rej(new Error(m.result.exceptionDetails.text))
      : res(m.result && m.result.result ? m.result.result.value : undefined)));
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, returnByValue: true, awaitPromise: true } }));
    setTimeout(() => res(undefined), 8000);
  });
};
// One of the app's own primitives, through its --mcp door
const primitive = (name, params) => new Promise((res) => {
  const door = spawn(exe, ['--mcp', '--pid', appPid(), '--token-file', path.join(APP, 'data', 'api-token')], { stdio: ['pipe', 'pipe', 'ignore'] });
  let out = '';
  const done = (v) => { try { door.kill(); } catch {} res(v); };
  door.stdout.on('data', (b) => {
    out += b;
    for (const line of out.split('\n')) {
      try { const m = JSON.parse(line); if (m.id === 2) return done(m.result ? (m.result.content || []).map((c) => c.text).join('') : 'ERROR ' + JSON.stringify(m.error)); } catch {}
    }
  });
  const say = (m) => door.stdin.write(JSON.stringify(m) + '\n');
  say({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'check', version: '1' } } });
  say({ jsonrpc: '2.0', method: 'notifications/initialized' });
  say({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'shikisha_' + name, arguments: { params } } });
  setTimeout(() => done('TIMEOUT'), 30000);
});

// Shift_JIS for 氏名,メモ / 山田,済
const SJIS = Buffer.from([0x8e, 0x81, 0x96, 0xbc, 0x2c, 0x83, 0x81, 0x83, 0x82, 0x0d, 0x0a, 0x8e, 0x52, 0x93, 0x63, 0x2c, 0x8d, 0xcf, 0x0d, 0x0a]);

try {
  console.log('the server: ' + (await there('uname -sr')).trim());
  await cleanThere();
  await there(`mkdir -p ${FOLDER}/src ${DIR}/bin && printf %s ${b64(STAND_IN)} | base64 -d > ${DIR}/bin/claude && chmod +x ${DIR}/bin/claude`
    + ` && printf '# proj\\nhello NEEDLE here\\n' > ${FOLDER}/readme.md && printf 'fn main() {}\\n' > ${FOLDER}/src/main.rs`
    + ` && printf %s ${SJIS.toString('base64')} | base64 -d > ${FOLDER}/sjis.csv`
    + ` && cd ${FOLDER} && git init -q && git add -A && git -c user.email=a@b -c user.name=a commit -qm init`);

  stopApp();
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'ja',
    remote: { enabled: false },
    // Closing the window quits, rather than going to the notification area
    resident: false,
    // Leaves the key in data/api-token, for the calls below to use
    external_api: { access: 'user' },
    hosts: [{ name: 'srv', at: `ssh://${USER}@${HOST}:${PORT}`, ...(KEY ? { key: KEY } : {}) }],
    desks: [{ name: 'Check', id: 'check', folders: [{
      cwd: FOLDER, host: 'srv',
      tabs: [
        { name: 'コマンド', id: 'sh1', command: '' },
        { name: 'ai', id: 'ai', command: `${DIR}/bin/claude` },
        { name: 'ed', id: 'ed', command: 'editor' },
      ],
    }] }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: PASSWORD ? { 'ssh/host/srv/password': PASSWORD } : {} }, null, 2));

  start();
  const board = await boardOf();
  await until(() => board('!!(S && S.tabs && S.tabs.some(t => t.id === "sh1"))'), 'the folder\'s tabs', 40000);

  console.log('1. a tab with no command is the server\'s shell');
  await until(async () => (await primitive('tab_screen', ['sh1'])).includes('proj'), 'a prompt in the folder', 30000)
    .then(() => check(true, 'a shell there, in the folder'))
    .catch(async () => check(false, 'a shell there, in the folder: ' + (await primitive('tab_screen', ['sh1'])).slice(-200)));

  console.log('2. the file list and the searches run on the server');
  await board('(() => { window.__heard = []; const o = window.__files; window.__files = d => { window.__heard.push(d); return o(d); }; return true; })()');
  const ask = (panel, act, args) => board(`(send({kind: "files", panel: ${JSON.stringify(panel)}, act: ${JSON.stringify(act)}, args: ${JSON.stringify(args)}}), true)`);
  const answer = async (test, what) => { let d; await until(async () => (d = await board(`(window.__heard || []).filter(${test}).pop() || null`)), what, 30000); return d; };
  await ask('sh1', 'ls', { at: '' });
  let d = await answer('d => d.act === "ls" && d.at === ""', 'the folder listed');
  const names = (d.rows || []).map((r) => r.name);
  check(d.ok && names.includes('readme.md') && names.includes('src') && !names.includes('.git'), 'the folder there: ' + names.join(', '));
  await ask('sh1', 'find', { q: 'main' });
  d = await answer('d => d.act === "find" && d.q === "main"', 'a search by name');
  check(d.ok && (d.hits || []).map((h) => h.path).join() === 'src/main.rs', 'by name: ' + JSON.stringify(d.hits));
  await ask('sh1', 'grep', { q: 'needle' });
  d = await answer('d => d.act === "grep" && d.q === "needle"', 'a search by contents');
  check(d.ok && d.hits?.length === 1 && d.hits[0].path === 'readme.md' && d.hits[0].line === 2, 'by contents: ' + JSON.stringify(d.hits));

  console.log('3. the editor reads from the server and saves back');
  await board('(send({kind: "editopen", panel: "sh1", path: "readme.md"}), true)');
  await until(() => board('ED.path === "readme.md" && !ED.loading && !!edAce && edAce.getValue().includes("NEEDLE")'), 'the file in the editor', 60000);
  await board('edAce.setValue("# proj\\nfrom the editor\\n", 1); true');
  await until(() => board('ED.dirty'), 'typed');
  await board('editSave(); true');
  await until(() => board('!ED.dirty && !ED.bad'), 'the save', 60000);
  check((await there(`cat ${FOLDER}/readme.md`)) === '# proj\nfrom the editor\n', 'the server has what was saved');
  await board('(send({kind: "editopen", panel: "sh1", path: "sjis.csv"}), true)');
  await until(() => board('ED.path === "sjis.csv" && !ED.loading && edAce.getValue().includes("山田")'), 'the Shift_JIS file', 60000);
  check(await board('ED.encoding') === 'Shift_JIS', 'read as Shift_JIS');
  await board('edAce.setValue(edAce.getValue().replace("済", "未"), 1); true');
  await until(() => board('ED.dirty'), 'typed');
  await board('editSave(); true');
  await until(() => board('!ED.dirty && !ED.bad'), 'the save', 60000);
  const want = Buffer.from(SJIS); want[16] = 0x96; want[17] = 0xa2; // 未
  const got = Buffer.from((await there(`base64 < ${FOLDER}/sjis.csv`)).replace(/\s/g, ''), 'base64');
  check(got.equals(want), 'saved back in Shift_JIS, byte for byte: ' + got.toString('hex'));
  await board('edAce.setValue(edAce.getValue() + "mine,1\\r\\n", 1); true');
  await until(() => board('ED.dirty'), 'typed');
  await there(`printf 'somebody else\\r\\n' > ${FOLDER}/sjis.csv`);
  await board('editWrite({}); true');
  await until(() => board('ED.bad'), 'the refusal', 60000).catch(() => {});
  check((await there(`cat ${FOLDER}/sjis.csv`)) === 'somebody else\r\n', 'a save over a changed file is refused: ' + await board('ED.said'));

  console.log('4. an AI tab comes back on its conversation after a restart');
  await until(async () => (await calls()).length >= 1, 'the stand-in to start', 60000);
  const first = (await calls())[0];
  const minted = (first.match(/^--session-id ([0-9a-f-]{36})$/) || [])[1];
  check(!!minted, 'started under the app\'s id: ' + first);
  await until(() => remembered()?.session === minted, 'the id written down', 20000)
    .then(() => check(true, 'the id is written down'))
    .catch(() => check(false, 'the id is written down: ' + JSON.stringify(remembered())));
  await sleep(12000);
  await board('typeof winAct === "function" && winAct("close")');
  await until(() => !appPid(), 'the app to quit', 30000);
  start();
  await until(async () => (await calls()).length >= 2, 'the stand-in to start again', 60000);
  check((await calls())[1] === '--resume ' + minted, 'resumed there: ' + (await calls())[1]);
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + (e.stack || e));
} finally {
  stopApp();
  await cleanThere();
  await sleep(500);
  fs.rmSync(RUN, { recursive: true, force: true });
  console.log('the server is as it was');
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
