/**
 * Tabs on another machine -- a server over SSH, or a MicroVM -- as the @ in
 * the input bar offers them, and what an AI here can do with them.
 *
 * This checkout's build in a folder of its own, with a real Claude Code tab
 * here (the skill in its folder as a project skill, so the real home is not
 * touched) and a folder on the other machine holding two tabs:
 *
 *   far    a tab with no command: that machine's shell, in the folder
 *   farai  a stand-in `claude` there, which answers each line with GOT:<line>
 *          on its screen, and with eighty lines in its record, where Claude
 *          Code keeps one -- more than a screen holds
 *
 * Checked:
 *   0. what the @ list offers from the Claude tab, and how each far tab is
 *      counted (an AI or a terminal)
 *   1. run   "run `cat value.txt` in <@far>": the value only that machine
 *            has comes back through the real Claude
 *   2. ask   ask_tab to <@farai>, straight through the pipe: taken as an AI,
 *            and its whole answer read from the record on that machine
 *   3. tab_conversation, as the phone's reader reads it: the far AI's
 *      question and answer, the Claude tab's here, a shell told it keeps
 *      none, and a tab that is not there
 *
 *     cargo build
 *     node tools/debug/mention-far.win.mjs --where=ssh
 *     node tools/debug/mention-far.win.mjs --where=vm
 *
 * Needs Windows, Node, `claude` signed in, and in .private/.env the server
 * ssh-far.win.mjs uses (SSH_TEST_*) or E2B_API_TOKEN. Spends one turn of
 * Claude Code. Everything put on the other machine is removed on the way out,
 * and a MicroVM made for this is deleted.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { createRequire } from 'node:module';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const WHERE = (process.argv.find((a) => a.startsWith('--where=')) || '--where=ssh').slice(8);
if (!['ssh', 'vm'].includes(WHERE)) { console.error('--where=ssh or --where=vm'); process.exit(2); }
const RUN = path.join(os.tmpdir(), 'sk-mention-far-' + process.pid);
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const VALUE = 'V' + Math.random().toString(36).slice(2, 8).toUpperCase();
const MARK = 'shikisha-mention-far-' + process.pid;

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

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

// A stand-in claude: one line in, GOT:<line> out on the screen -- and in its
// record, where Claude Code keeps one, an answer of eighty lines, more than a
// screen holds, so an answer read whole can only have come from the record
const STAND_IN = `#!/bin/sh
id=""
while [ $# -gt 0 ]; do case "$1" in --session-id|--resume) id="$2"; shift;; esac; shift; done
rec="$HOME/.claude/projects/${MARK}/$id.jsonl"
mkdir -p "$(dirname "$rec")"
echo "stand-in claude"
while read -r line; do
  printf '{"type":"user","message":{"role":"user","content":"%s"}}\\n' "$line" >> "$rec"
  ans=""; i=1
  while [ $i -le 80 ]; do ans="\${ans}answer line $i of $line\\\\n"; i=$((i+1)); done
  printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]}}\\n' "$ans" >> "$rec"
  echo "GOT:$line"
done
`;

// ── The other machine ─────────────────────────
let there;          // runs a command there, returns what it printed
let farHost;        // the settings' hosts entry
let farFolder;      // the folder's settings, beyond cwd and tabs
let farSecrets;     // secrets.json tokens
let cleanup = async () => {};
let HOME;
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
  console.log('making a MicroVM');
  const made = await (await fetch('https://api.e2b.app/sandboxes', {
    method: 'POST', headers: { 'X-API-Key': KEY, 'Content-Type': 'application/json' },
    body: JSON.stringify({ templateID: 'base', timeout: 900, autoPause: true, autoResume: { enabled: true },
      metadata: { shikisha: '1', project: 'mention-far-check' } }),
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
    console.log('the MicroVM is deleted');
  };
}
const DIR = `${HOME}/${MARK}`;
const b64 = (s) => Buffer.from(s).toString('base64');

// ── The copy ──────────────────────────────────
let door;
const openDoor = async (pid) => {
  const tokenFile = path.join(APP, 'data', 'api-token');
  await until(() => fs.existsSync(tokenFile), 'the api token', 60000);
  const token = fs.readFileSync(tokenFile, 'utf8').trim();
  // One connection per call: a held call (ask_tab) keeps its line busy
  door = async (method, ...params) => {
    const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
    await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
    let buf = '';
    const lines = [];
    let wake = null;
    sock.on('data', (d) => {
      buf += d.toString('utf8');
      let i;
      while ((i = buf.indexOf('\n')) >= 0) { lines.push(JSON.parse(buf.slice(0, i))); buf = buf.slice(i + 1); if (wake) wake(); }
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
const screen = (id) => door('tab_screen', id).then((s) => String(s || '')).catch(() => '');
const logLen = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/).length : 0);
const logSince = (from) => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/).slice(from) : []);

try {
  console.log(`the other machine (${WHERE}): ` + (await there('uname -sr')).trim());
  await there(`mkdir -p ${DIR}/bin && printf %s ${b64(STAND_IN)} | base64 -d > ${DIR}/bin/claude && chmod +x ${DIR}/bin/claude`
    + ` && printf '%s\\n' ${VALUE} > ${DIR}/value.txt`);

  stopApp();
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL, WORK]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
  if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
  const skillText = spawnSync(appExe, ['--cli', 'skill'], { encoding: 'utf8' }).stdout;
  const skillAt = path.join(WORK, '.claude', 'skills', 'shikisha');
  fs.mkdirSync(skillAt, { recursive: true });
  fs.writeFileSync(path.join(skillAt, 'SKILL.md'), skillText);
  spawnSync('git', ['init', '-q'], { cwd: WORK });
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'en',
    remote: { enabled: false },
    resident: false,
    external_api: { access: 'user' },
    hosts: [farHost],
    desks: [{ name: 'Far', id: 'far', folders: [
      { cwd: WORK, tabs: [{ name: 'claude', id: 'claude', command: ['claude', '--dangerously-skip-permissions'] }] },
      { cwd: DIR, ...farFolder, tabs: [
        { name: 'far', id: 'far', command: '' },
        { name: 'farai', id: 'farai', command: `${DIR}/bin/claude` },
      ] },
    ] }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: farSecrets }, null, 2));

  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
  const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
  child.unref();
  await openDoor(child.pid);
  const board = await boardOf();
  await until(() => board('!!(S && S.tabs && S.tabs.some(t => t.id === "farai"))'), 'the tabs', 60000);
  await until(async () => (await screen('far')).includes(MARK), 'the far shell in its folder', 90000)
    .catch(async () => note('the far shell: ' + (await screen('far')).slice(-200)));
  await until(async () => (await screen('farai')).includes('stand-in claude'), 'the stand-in there', 90000)
    .catch(async () => note('farai: ' + (await screen('farai')).slice(-200)));

  console.log('0. what @ offers from the Claude tab');
  const idx = await board('S.tabs.find(t => t.id === "claude").index');
  await board(`(send({kind:"select", tab:${idx}}), true)`);
  await until(() => board(`S.active === ${idx}`), 'the Claude tab in front', 10000);
  const offered = await board('mentionCandidates().map(t => ({id: t.id, kind: t.kind, ai: !!t.ai}))');
  note('offered: ' + JSON.stringify(offered));
  check((offered || []).some((t) => t.id === 'far'), 'the far shell is offered');
  const farai = (offered || []).find((t) => t.id === 'farai');
  note('farai is counted as ' + (farai ? (farai.ai ? 'an AI' : 'a terminal') : 'not offered'));

  // Claude Code up, its folder trusted
  await until(async () => {
    const s = await screen('claude');
    if (/I trust this folder/i.test(s)) { await door('send', 'claude', '\x1b[B'); await sleep(300); await door('send', 'claude', '\r'); }
    return /bypass permissions|for shortcuts/i.test(s);
  }, 'Claude Code to come up', 120000);
  await sleep(3000);

  console.log('1. run: a command on the other machine, through the real Claude');
  let from = logLen();
  await board(`send({kind:"say", tab:${idx}, text:${JSON.stringify('Run the command `cat value.txt` in <@far> and tell me exactly what it printed.')}}); true`);
  const got = await until(async () => {
    const s = await screen('claude');
    return s.includes(VALUE) && (await door('state', 'claude').catch(() => '?')) !== 'BUSY';
  }, 'the value back', 6 * 60000).catch(() => false);
  check(got, 'the value only that machine has came back: ' + VALUE);
  check(logSince(from).some((l) => /ask_tab: \S+ asks far\b/.test(l)), 'it went through tab_run');
  if (!got) note('claude: ' + (await screen('claude')).split('\n').slice(-25).join('\n'));

  console.log('2. ask: the stand-in AI there, straight through the pipe');
  from = logLen();
  const asked = await door('ask_tab', 'farai', 'ping ' + VALUE, { timeout_ms: 45000 })
    .then((r) => ({ ok: true, r })).catch((e) => ({ ok: false, r: e.message }));
  note('answer: ' + JSON.stringify(asked).slice(0, 600));
  check(asked.ok, 'ask_tab is taken');
  const reply = asked.ok ? String(asked.r.reply || '') : '';
  check(asked.ok && asked.r.source === 'record', 'read from the record on that machine, not the screen: ' + (asked.ok ? asked.r.source : '-'));
  check(reply.includes('answer line 1 of ping ' + VALUE) && reply.includes('answer line 80 of ping ' + VALUE),
    'the whole eighty-line answer came back (' + reply.split('\n').length + ' lines)');
  note('farai screen: ' + (await screen('farai')).split('\n').filter(Boolean).slice(-4).join(' | '));

  console.log('3. tab_conversation: the conversations, as the phone\'s reader reads them');
  const far = await door('tab_conversation', 'farai', { want: 2 }).catch((e) => ({ error: e.message }));
  const farTurns = far.turns || [];
  note('farai: ' + JSON.stringify({ source: far.source, turns: farTurns.map((t) => [t.who, t.text.slice(0, 40)]), more: far.more, error: far.error }));
  check(farTurns.length === 2 && farTurns[0].who === 'you' && farTurns[0].text === 'ping ' + VALUE
    && farTurns[1].who === 'ai' && farTurns[1].text.includes('answer line 80'), 'the far AI\'s question and whole answer');
  const here = await door('tab_conversation', 'claude', { want: 2 }).catch((e) => ({ error: e.message }));
  const hereTurns = here.turns || [];
  note('claude: ' + JSON.stringify({ source: here.source, turns: hereTurns.map((t) => [t.who, t.text.slice(0, 60)]), error: here.error }));
  check(hereTurns.some((t) => t.who === 'you' && t.text.includes('cat value.txt'))
    && hereTurns.some((t) => t.who === 'ai' && t.text.includes(VALUE)), 'the Claude tab here: what the person asked, and its answer');
  const shell = await door('tab_conversation', 'far', {}).catch((e) => ({ error: e.message }));
  check(shell.source === 'none' && /tab_screen/.test(shell.note || ''), 'a shell keeps no conversation, and is told where to look: ' + (shell.note || shell.error));
  const gone = await door('tab_conversation', 'nobody', {}).then(() => '').catch((e) => e.message);
  check(/no tab <@nobody>/.test(gone), 'a tab that is not there is said to be not there: ' + gone);

  console.log('4. the real Claude reads the far AI\'s conversation with the shikisha command');
  from = logLen();
  await board(`send({kind:"say", tab:${idx}, text:${JSON.stringify('Without asking it anything, read the last answer in the conversation of <@farai> and tell me its final line exactly.')}}); true`);
  const read = await until(async () => {
    const s = await screen('claude');
    return s.includes('answer line 80 of ping ' + VALUE) && (await door('state', 'claude').catch(() => '?')) !== 'BUSY';
  }, 'the last line of the far answer', 6 * 60000).catch(() => false);
  check(read, 'the last of the eighty lines came back through the real Claude');
  check(!logSince(from).some((l) => /ask_tab: \S+ asks farai/.test(l)), 'the far AI was not asked anything');
  if (!read) note('claude: ' + (await screen('claude')).split('\n').slice(-25).join('\n'));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + (e.stack || e));
} finally {
  stopApp();
  await there(`rm -rf ${DIR} "$HOME/.claude/projects/${MARK}"; pkill -f "${DIR}/bin/claude" 2>/dev/null; true`).catch(() => {});
  await cleanup();
  await sleep(500);
  fs.rmSync(RUN, { recursive: true, force: true });
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
