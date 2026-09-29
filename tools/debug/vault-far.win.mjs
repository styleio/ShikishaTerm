/**
 * Find on another machine, end to end: past conversations on a server or a
 * MicroVM searched, read whole, and picked back up -- once with no bridge
 * there, and once with the bridge the person agreed to.
 *
 * This checkout's build in a folder of its own, with a desk that has a folder
 * on the other machine and a stand-in AI running in it (a `claude` script
 * there, so the app takes it for Claude Code and connects the bridge). Three
 * conversations are written there the way Claude Code writes them:
 *
 *   - one whose word is said after two megabytes of tool output
 *   - one whose word is only in the folder it ran in
 *   - one had in a folder that has since been removed
 *
 * Checked, in each of the two passes:
 *   1. the word said after the tool output is found, on that machine
 *   2. a word only in a conversation's folder finds nothing
 *   3. the conversation opens in the conversation panel, the word marked,
 *      Resume one press
 *   4. its tool runs open when their kind is shown
 *   5. a conversation whose folder is gone offers the desk's folder there,
 *      and writes nothing until one is chosen; chosen, it is written there
 *   6. which way it was found and placed: by the bridge when it is there, the long way
 *      when it is not
 *
 *     cargo build
 *     node tools/debug/vault-far.win.mjs --where=ssh     (or --where=vm)
 *
 * The bridge program is taken from bridge/ beside the build (dist.list): put a
 * Linux build there first, e.g. with cargo zigbuild in WSL.
 *
 * Needs Windows, Node, and in .private/.env the test server (SSH_TEST_*) or
 * E2B_API_TOKEN. No AI account is used. Everything put there is removed.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const WHERE = (process.argv.find((a) => a.startsWith('--where=')) || '--where=ssh').slice(8);
const RUN = path.join(os.tmpdir(), 'sk-vault-far');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const MARK = 'shikisha-vault-far-' + process.pid;
const BRIDGE_DIR = '.local/share/shikisha/bridge';

const LATE = 'aaaa1111-0000-4000-8000-00000000000' + (process.pid % 10);
const NAMED = 'bbbb2222-0000-4000-8000-00000000000' + (process.pid % 10);
const GONE = 'cccc3333-0000-4000-8000-00000000000' + (process.pid % 10);
const WORD = 'zephyrquartz' + process.pid;
const FOLDER_WORD = 'amberfolder' + process.pid;
const GONE_WORD = 'glimmerwick' + process.pid;

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
const log = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '');

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (!fs.existsSync(path.join(ROOT, 'bridge', 'shikisha-bridge-x86_64-linux'))) die('no Linux bridge in bridge/ -- build one first');

// The far AI: there so the app keeps a line to the machine, and nothing more
const STAND_IN = '#!/bin/sh\necho "stand-in claude"\nwhile read -r line; do :; done\n';

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
    body: JSON.stringify({ templateID: 'base', timeout: 1200, autoPause: true, autoResume: { enabled: true },
      metadata: { shikisha: '1', project: 'vault-far-check' } }),
  })).json();
  if (!made.sandboxID) die('no machine: ' + JSON.stringify(made));
  const box = await Sandbox.connect(made.sandboxID, { apiKey: KEY });
  there = async (cmd) => {
    const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
    return r.stdout + r.stderr;
  };
  HOME = '/home/user';
  farHost = { name: 'vm', kind: 'e2b', template: 'base', minutes: 20 };
  farFolder = { host: 'vm', sandbox: box.sandboxId };
  farSecrets = { e2b_api_key: KEY };
  cleanup = async () => {
    await fetch('https://api.e2b.app/sandboxes/' + box.sandboxId, { method: 'DELETE', headers: { 'X-API-Key': KEY } }).catch(() => {});
  };
}
const DIR = `${HOME}/${MARK}`;
const RECORDS = `${HOME}/.claude/projects/${MARK}`;
const b64 = (s) => Buffer.from(s).toString('base64');

// Three conversations there, written the way Claude Code writes them
const line = (who, cwd, content) => JSON.stringify({ type: who, cwd, gitBranch: 'main', message: { role: who, content } });
const text = (t) => [{ type: 'text', text: t }];
const records = {
  [LATE]: [
    line('user', DIR, 'the checkout page is slow'),
    JSON.stringify({ type: 'assistant', cwd: DIR, message: { role: 'assistant', content: [{ type: 'tool_use', id: 't1', name: 'Bash', input: { command: 'tail app.log' } }] } }),
    'BIG',
    line('assistant', DIR, text(`The cart total goes through the ${WORD} module on every request.`)),
  ],
  [NAMED]: [line('user', `${HOME}/${FOLDER_WORD}`, 'hello there')],
  [GONE]: [
    line('user', `${HOME}/${MARK}-gone`, `rename the ${GONE_WORD} helper`),
    line('assistant', `${HOME}/${MARK}-gone`, text('Renamed it.')),
  ],
};
// Two megabytes of a tool's output, made there rather than sent
const bigHead = `{"type":"user","cwd":"${DIR}","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"`;

fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, WORK]) fs.mkdirSync(d, { recursive: true });
const writeConfig = (bridges) => fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  resident: false,
  hosts: [farHost],
  bridges,
  desks: [{ name: 'Far', id: 'far', folders: [
    { cwd: WORK, tabs: [{ name: 'here', id: 'here', command: 'cmd' }] },
    { cwd: DIR, ...farFolder, tabs: [{ name: 'farai', id: 'farai', command: `${DIR}/bin/claude` }] },
  ] }],
}, null, 2));

// The window's page, found by the port its WebView2 wrote down
const boardOf = async () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  let target;
  await until(async () => {
    if (!fs.existsSync(f)) return false;
    const port = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
    target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
    return !!target;
  }, 'the window\'s page', 60000);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let n = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => { const m = JSON.parse(e.data); if (waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); } });
  return (expression) => new Promise((res) => {
    const id = ++n;
    waiting.set(id, (m) => res(m.result && m.result.result ? m.result.result.value : undefined));
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, returnByValue: true, awaitPromise: true } }));
    setTimeout(() => res(undefined), 15000);
  });
};

const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';

/** One pass: the app started over the settings, Find used, and checked */
const pass = async (bridged) => {
  stopApp();
  await sleep(1500);
  fs.rmSync(path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort'), { force: true });
  writeConfig(bridged ? [farHost.name] : []);
  const from = log().length;
  const since = () => log().slice(from);
  spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  const run = await boardOf();
  await until(() => run('!!(S && S.tabs && S.tabs.some(t => t.id === "farai"))'), 'the tabs', 60000);
  // The far tab in front: a MicroVM is woken for it, a server is already there
  const fi = await run('S.tabs.find(t => t.id === "farai").index');
  await run(`(send({kind:"select", tab:${fi}}), true)`);
  if (bridged) {
    await until(() => /bridge: connected to/.test(since()), 'the bridge to connect', 240000)
      .catch(() => note('log: ' + since().split(/\r?\n/).filter((l) => /bridge/.test(l)).slice(-4).join(' | ')));
  } else {
    await until(async () => /stand-in claude/.test(await run('(document.body.innerText || "")') || '') || true, 'the far tab', 5000).catch(() => {});
    await sleep(WHERE === 'vm' ? 20000 : 5000);
  }
  await run('window.__openVault(); true');
  await until(() => run('!!(cvUi && cvAll)'), 'the panel on every conversation', 20000);
  const search = async (q) => {
    // Asked with the paused machines too: a MicroVM is only searched when awake
    await run(`(() => { cvUi.q.value = ${JSON.stringify(q)}; CV.q = ${JSON.stringify(q)};
      send({kind:"vaultsearch", query:${JSON.stringify(q)}, wake:true}); return true; })()`);
    await until(() => run(`!!(S && S.vault && S.vault.query === ${JSON.stringify(q)} && !S.vault.searching && !S.vault.asking)`),
      'the search for ' + q, 180000);
    await until(() => run('document.querySelectorAll("#convopanel .vrow").length === S.vault.hits.length'), 'the list to be drawn');
    return run('S.vault.hits.filter(h => h.host).map(h => ({id: h.id, snippet: h.snippet, title: h.title, host: h.host, at: h.at}))');
  };
  const openRow = async (id) => {
    await run(`(() => { const i = S.vault.hits.findIndex(h => h.id === ${JSON.stringify(id)});
      document.querySelectorAll("#convopanel .vrow")[i].click(); return true; })()`);
    await until(() => run(`!!(CV.past && CV.past.id === ${JSON.stringify(id)} && !CV.loading && (CV.rows.length || CV.said))`),
      'the conversation to be read', 180000);
    await until(() => run('!!document.querySelector("#convoHead .hgo")'), 'the way to pick it back up', 120000);
  };

  console.log('1. the word said after the tool output is found there');
  const late = await search(WORD);
  check(late.length === 1 && late[0].id === LATE, 'found on that machine: ' + JSON.stringify(late));
  check(late[0] && late[0].host === farHost.name && late[0].title.startsWith(farHost.name + ': '), 'the row says which machine');
  if (bridged) check(typeof (late[0] && late[0].at) === 'number', 'and, by the bridge, where in the record it is');

  console.log('2. a word only in a conversation\'s folder finds nothing');
  const named = await search(FOLDER_WORD);
  check(named.length === 0, 'nothing: ' + JSON.stringify(named));

  console.log('3. the conversation opens in the panel, the word marked, Resume one press');
  await search(WORD);
  await openRow(LATE);
  await until(() => run('!!cvUi.list.querySelector("mark.vmark")'), 'the word to be marked', 60000).catch(() => {});
  const read = await run(`({ said: CV.said, marks: cvUi.list.querySelectorAll("mark.vmark").length,
    back: !!document.querySelector("#convoHead .hback"), go: document.querySelector("#convoHead .hgo").textContent })`);
  check(!read.said && read.marks >= 1, 'read, the word marked: ' + JSON.stringify(read));
  check(read.back, 'the way back to the list stands over it');
  check(read.go === 'Resume', 'its folder is there, so Resume is one press: ' + read.go);

  console.log('4. the tool runs open when their kind is shown');
  await run('(() => { cvUi.q.value = ""; cvUi.q.dispatchEvent(new Event("input")); return true; })()');
  await run('(() => { const b = cvUi.boxes.work; if (!b.checked) b.click(); return true; })()');
  await until(() => run('!!cvUi.list.querySelector(".vwork")'), 'the tool runs to be listed', 60000);
  await run('(() => { const w = cvUi.list.querySelector(".vwork"); if (!w.dataset.filled) w.querySelector(".vmore").click(); return true; })()');
  await until(() => run('!!cvUi.list.querySelector(".vwork[data-filled] .vpiece")'), 'the work to open', 120000);
  const work = await run(`[...cvUi.list.querySelectorAll(".vwork .vpiece")].map(p => p.className + ":" + ((p.querySelector(".vpname") || {}).textContent || "") + ":" + ((p.querySelector(".vpcut") || {}).textContent || ""))`);
  check(work.some((w) => /call:Bash/.test(w)) && work.some((w) => /out:.*not shown/.test(w)), 'the call and its cut output: ' + JSON.stringify(work));

  console.log('5. a conversation whose folder is gone offers the folder there');
  const before = fs.readFileSync(CONFIG, 'utf8');
  await run('document.querySelector("#convoHead .hback").click(); true');
  await search(GONE_WORD);
  await openRow(GONE);
  check(await run('document.querySelector("#convoHead .hgo").textContent') === 'Resume ▾', 'Resume lists where it can go instead');
  await run('document.querySelector("#convoHead .hgo").click(); true');
  await until(() => run('!!document.querySelector(".fmenu .vwhere")'), 'the list of places', 10000);
  const places = await run('[...document.querySelectorAll(".fmenu > div")].map(d => d.textContent)');
  check(places.some((p) => p.includes(`${MARK}-gone`)) && places.some((p) => p.includes(DIR)),
    'the gone folder named, and the folder there offered: ' + JSON.stringify(places));
  check(fs.readFileSync(CONFIG, 'utf8') === before, 'nothing was written yet');
  await run(`[...document.querySelectorAll(".fmenu .vwhere")].find(d => d.textContent.includes(${JSON.stringify(DIR)})).click(); true`);
  await until(() => fs.readFileSync(CONFIG, 'utf8').includes(GONE), 'the tab to be written', 20000);
  const folders = JSON.parse(fs.readFileSync(CONFIG, 'utf8')).desks[0].folders;
  const far = folders.find((f) => f.cwd === DIR);
  check(far && (far.tabs || []).some((t) => t.resume === GONE), 'written into the folder there: ' + JSON.stringify(far && far.tabs));
  check(!folders.some((f) => (f.cwd || '').includes('-gone')), 'the gone folder is not written back');

  console.log('6. which way it was found and placed');
  const byBridge = ['vault_search', 'vault_where'].map((op) => new RegExp(`vault: ${op} answered by the bridge`).test(since()));
  if (bridged) check(byBridge.every(Boolean), 'the search and where it was had, by the bridge: ' + JSON.stringify(byBridge));
  else check(!byBridge.some(Boolean), 'no bridge was asked');
  check(!/asking the long way/.test(since()) || !bridged, 'the bridge never fell back to the long way');
};

try {
  console.log(`the other machine (${WHERE}): ` + (await there('uname -srm')).trim());
  const put = [`rm -rf ${HOME}/${BRIDGE_DIR}; mkdir -p ${DIR}/bin ${RECORDS}`,
    `printf %s ${b64(STAND_IN)} | base64 -d > ${DIR}/bin/claude && chmod +x ${DIR}/bin/claude`];
  for (const [id, lines] of Object.entries(records)) {
    put.push(`: > ${RECORDS}/${id}.jsonl`);
    for (const l of lines) {
      if (l === 'BIG') put.push(`{ printf '%s' '${bigHead}'; head -c 2000000 /dev/zero | tr '\\0' x; printf '%s\\n' '"}]}}'; } >> ${RECORDS}/${id}.jsonl`);
      else put.push(`printf '%s\\n' ${`'${l.replace(/'/g, "'\\''")}'`} >> ${RECORDS}/${id}.jsonl`);
    }
  }
  const made = await there(put.join(' && ') + ` && wc -c ${RECORDS}/*.jsonl`);
  note(made.trim().split('\n').join(' | '));
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: farSecrets }, null, 2));

  console.log('== without a bridge ==');
  await pass(false);
  console.log('== with the bridge ==');
  await pass(true);
} catch (e) {
  failures += 1;
  console.error('stopped: ' + e.message);
} finally {
  stopApp();
  await there(`rm -rf ${DIR} ${RECORDS} ${HOME}/${BRIDGE_DIR}`).catch(() => {});
  await cleanup();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
