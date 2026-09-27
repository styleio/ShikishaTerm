/**
 * How often one AI tab can hand work to another with ask_tab and get the reply back.
 *
 * The proof of concept behind the @ mention in the input bar: a real Claude
 * Code and a real Codex, each in a tab of this checkout's build, each given
 * the app's MCP server the way the app would register it (the call held for up
 * to an hour). A person's request is typed into one tab, naming another as
 * [[tab:ID]]; the trial passes when the answer the other tab alone could give
 * comes back and is reported by the first.
 *
 *     cargo build
 *     node tools/debug/ask-tab-poc.win.mjs [--short=20] [--long=6] [--loop=5] [--only=short|long|loop]
 *
 * What is asked:
 *   short  "ask X what is in secret-N.txt"         -- claude->codex, codex->claude, claude->claude
 *   long   the same after X has slept 90 seconds  -- longer than Codex's own default of 60s
 *   loop   "ask codex to review calc.js, fix what it finds, again until it finds nothing"
 *          -- judged by tests the AIs never see
 *
 * Every secret sits only in the answering tab's folder, under a name used once,
 * so an answer cannot come from memory or from the asking tab's own folder.
 *
 * Needs Windows, Node, git, and `claude` and `codex` signed in on this machine.
 * Spends real turns of both accounts. Isolated the way keep-running.win.mjs is:
 * its own folder and LOCALAPPDATA, nothing of a copy somebody is using touched.
 * Codex's settings are passed as -c on the tab's own command line, so the
 * user's ~/.codex/config.toml is not written. Results go to
 * target/ask-tab-poc/results.json and a line per trial to the console.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const OUT = path.join(ROOT, 'target', 'ask-tab-poc');
const arg = (name, dflt) => {
  const a = process.argv.find((x) => x.startsWith(`--${name}=`));
  return a ? a.slice(name.length + 3) : dflt;
};
const COUNTS = { short: Number(arg('short', 20)), long: Number(arg('long', 6)), loop: Number(arg('loop', 5)) };
const ONLY = arg('only', '');
const RUN = path.join(os.tmpdir(), 'sk-asktab');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const F = { a: path.join(WORK, 'fa'), b: path.join(WORK, 'fb'), c: path.join(WORK, 'fc'), repo: path.join(WORK, 'repo') };
const HIDDEN = path.join(RUN, 'hidden-test.js');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const git = (cwd, ...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
  return r.stdout;
};
const nonce = () => Math.random().toString(36).slice(2, 8).toUpperCase();

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
for (const cli of ['claude', 'codex']) {
  if (spawnSync('where.exe', [cli], { encoding: 'utf8' }).status !== 0) die(`${cli} is not on PATH`);
}

// ── The copy, its folders and its tabs ─────────────────────────────────────
console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, ...Object.values(F), path.join(RUN, 'localappdata'), OUT]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// The review: three bugs, and tests kept outside the folder the AIs work in
fs.writeFileSync(path.join(F.repo, 'calc.js'), [
  '// Small helpers used by the billing page.',
  '',
  '// Sum of a list of numbers.',
  'function sum(xs) {',
  '  let total = 0;',
  '  for (let i = 1; i < xs.length; i++) total += xs[i];',
  '  return total;',
  '}',
  '',
  '// Average of a list of numbers; 0 for an empty list.',
  'function average(xs) {',
  '  return sum(xs) / xs.length;',
  '}',
  '',
  '// Price after a discount given in percent (10 means 10% off).',
  'function discounted(price, percent) {',
  '  return price - price * percent;',
  '}',
  '',
  'module.exports = { sum, average, discounted };',
  '',
].join('\n'));
fs.writeFileSync(HIDDEN, [
  "const assert = require('assert');",
  `const { sum, average, discounted } = require(${JSON.stringify(path.join(F.repo, 'calc.js'))});`,
  'assert.strictEqual(sum([1, 2, 3]), 6);',
  'assert.strictEqual(average([]), 0);',
  'assert.strictEqual(average([2, 4]), 3);',
  'assert.strictEqual(discounted(200, 10), 180);',
  "console.log('ok');",
].join('\n'));
git(F.repo, 'init', '-q');
git(F.repo, 'add', '.');
git(F.repo, 'commit', '-q', '-m', 'start');

// The app's MCP server, as the app would register it for each CLI
const mcpJson = path.join(RUN, 'mcp-claude.json');
fs.writeFileSync(mcpJson, JSON.stringify({ mcpServers: { shikisha: {
  command: appExe, args: ['--mcp'], timeout: 3600000,
  env: { SHIKISHA_PIPE: '${SHIKISHA_PIPE}', SHIKISHA_TOKEN: '${SHIKISHA_TOKEN}', SHIKISHA_TAB: '${SHIKISHA_TAB}' },
} } }, null, 2));
const exeFwd = appExe.replace(/\\/g, '/');
// As argv, not one line: a line is split at spaces and its quotes kept, and
// Codex reads each -c value as TOML
const claudeCmd = ['claude', '--dangerously-skip-permissions', '--mcp-config', mcpJson];
const codexCmd = ['codex', '--dangerously-bypass-approvals-and-sandbox',
  '-c', `mcp_servers.shikisha.command=${exeFwd}`,
  '-c', 'mcp_servers.shikisha.args=["--mcp"]',
  '-c', 'mcp_servers.shikisha.env_vars=["SHIKISHA_PIPE","SHIKISHA_TOKEN","SHIKISHA_TAB"]',
  '-c', 'mcp_servers.shikisha.tool_timeout_sec=3600',
  '-c', 'mcp_servers.shikisha.default_tools_approval_mode="auto"'];

const TABS = {
  'claude-a': { cwd: F.a, command: claudeCmd },
  'codex-b': { cwd: F.b, command: codexCmd },
  'claude-c': { cwd: F.c, command: claudeCmd },
  'rev-claude': { cwd: F.repo, command: claudeCmd },
  'rev-codex': { cwd: F.repo, command: codexCmd },
};
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
const byFolder = {};
for (const [id, t] of Object.entries(TABS)) (byFolder[t.cwd] ||= []).push({ name: id, id, command: t.command });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  // The key written down, so this script can call the pipe as the person
  external_api: { access: 'user' },
  desks: [{ name: 'POC', id: 'poc', folders: Object.entries(byFolder).map(([cwd, tabs]) => ({ cwd, tabs })) }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
const child = spawn(appExe, [], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

// ── The door: the same pipe an AI's MCP server uses, as the person ─────────
const tokenFile = () => {
  for (const p of [path.join(APP, 'data', 'api-token'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data', 'api-token')]) {
    if (fs.existsSync(p)) return p;
  }
  return null;
};
let door;
const openDoor = async () => {
  const end = Date.now() + 60000;
  let file;
  while (!(file = tokenFile()) && Date.now() < end) await sleep(500);
  if (!file) die('no api-token appeared');
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i); buf = buf.slice(i + 1);
      const w = waiting.shift(); if (w) w(JSON.parse(line));
    }
  });
  const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
  const hello = await line({ token: fs.readFileSync(file, 'utf8').trim() });
  if (!hello.ok) die('the door refused: ' + JSON.stringify(hello));
  let n = 0;
  door = async (method, ...params) => {
    const a = await line({ id: String(++n), method, params });
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};
const screen = (id) => door('tab_screen', id).then((s) => String(s || ''));
const state = (id) => door('state', id);

// ── Bringing each CLI up to its prompt ─────────────────────────────────────
const ready = {
  claude: /bypass permissions|for shortcuts|\? for/i,
  codex: /Ask Codex to do anything|YOLO mode|send\s+⏎|⏎ send|context left|Ctrl\+J newline|esc to interrupt|\/ commands|full access/i,
};
const bringUp = async (id) => {
  const kind = id.includes('codex') ? 'codex' : 'claude';
  const end = Date.now() + 120000;
  while (Date.now() < end) {
    const s = await screen(id);
    if (/I trust this folder|Do you trust|trust the files|allow Codex to work/i.test(s)) {
      if (kind === 'claude') { await door('send', id, '\x1b[B'); await sleep(400); }
      await door('send', id, '\r');
      await sleep(2500);
      continue;
    }
    if (ready[kind].test(s)) return true;
    if (/Skip until next version/i.test(s) && kind === 'codex') {
      await door('send', id, '\x1b[B'); await sleep(300); await door('send', id, '\r'); await sleep(2000); continue;
    }
    await sleep(1000);
  }
  console.log(`--- ${id} did not come up; its screen ---\n${await screen(id)}\n---`);
  throw new Error(`${id} did not come up`);
};

// ── One trial ──────────────────────────────────────────────────────────────
const logLines = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/) : []);
const askEvents = (from) => logLines().slice(from).filter((l) => l.includes('ask_tab:'));

const QUIET = ['DONE', 'WAIT', 'BACKGROUND'];
/** Asks still out on the caller's behalf: sent and not yet answered, or answered
 *  "still working" and not yet handed back */
const outstanding = (from, callee) => {
  const ev = askEvents(from);
  const asked = ev.filter((l) => l.includes(`asks ${callee}`)).length;
  const answered = ev.filter((l) => l.includes(`${callee} -> `) && !l.includes('-> PENDING')).length;
  const handed = ev.filter((l) => l.includes('handed')).length;
  return asked > answered + handed;
};

/**
 * Waits for the caller to show what only the other tab could have told it.
 * A quiet caller is not the end: Claude Code moves a long MCP call into the
 * background and says so, then carries on when the answer arrives. So a
 * quiet caller fails only once nothing is still out on its behalf.
 */
const settles = async (id, got, from, callee, ms) => {
  const start = Date.now();
  let busy = false;
  let quiet = null;
  while (Date.now() - start < ms) {
    const st = await state(id).catch(() => '?');
    const q = QUIET.includes(st);
    if (['QUESTION', 'EXIT', 'FAILED', 'LIMIT'].includes(st)) return { ok: false, why: st };
    if (!q) { busy = true; quiet = null; }
    if (await got()) {
      if (q) return { ok: true };
    } else if (q && (busy || Date.now() - start > 15000)) {
      quiet ??= Date.now();
      if (Date.now() - quiet > 20000 && !outstanding(from, callee)) return { ok: false, why: 'finished without it' };
    }
    await sleep(1000);
  }
  return { ok: false, why: 'TIMEOUT' };
};

const results = [];
const record = (r) => {
  results.push(r);
  fs.writeFileSync(path.join(OUT, 'results.json'), JSON.stringify(results, null, 2));
  console.log(`${r.pass ? 'PASS' : 'FAIL'} ${r.kind} ${r.pair} #${r.n} ${Math.round(r.ms / 1000)}s` +
    (r.pass ? '' : ` -- ${r.why}`) + (r.rounds != null ? ` rounds=${r.rounds}` : ''));
};

const secretTrial = async (kind, n, caller, callee) => {
  const name = `memo-${nonce()}.txt`;
  const value = `${nonce()}-${nonce()}`;
  fs.writeFileSync(path.join(TABS[callee].cwd, name), value + '\n');
  const wait = kind === 'long'
    ? 'first run a shell command that waits 90 seconds (in the foreground, so it finishes before you answer), and after that '
    : '';
  const ask = `Ask [[tab:${callee}]] to ${wait}tell you the text written in ${name} in its own folder. ` +
    'That memo is only in its folder. When you have its answer, reply to me with only that text.';
  const from = logLines().length;
  const t0 = Date.now();
  await door('send_to_tab', caller, ask);
  const done = await settles(caller, async () => (await screen(caller)).includes(value), from, callee,
    kind === 'long' ? 10 * 60000 : 6 * 60000);
  const out = await screen(caller);
  const events = askEvents(from);
  const asked = events.some((l) => l.includes(`asks ${callee}`));
  const answered = events.find((l) => l.includes(`${callee} -> `));
  const pass = done.ok && out.includes(value);
  const why = pass ? '' : !done.ok ? `caller ended ${done.why}` : !asked ? 'the caller never called ask_tab'
    : !answered ? 'ask_tab never answered' : !/-> DONE/.test(answered) ? `ask_tab said ${answered.split('-> ')[1]}`
    : 'the answer was not reported';
  record({ kind, pair: `${caller}->${callee}`, n, pass, why, ms: Date.now() - t0, events, value,
    backgrounded: /MCP task/.test(out) });
  if (!pass) fs.writeFileSync(path.join(OUT, `fail-${kind}-${caller}-${callee}-${n}.txt`), out);
};

const loopTrial = async (n) => {
  git(F.repo, 'checkout', '-q', '--', '.');
  git(F.repo, 'clean', '-qfd');
  // A mark of this trial's own: the screen still shows the last trial's answer
  const mark = `LOOP${nonce()}`;
  const said = new RegExp(`${mark}-\\d`);
  const ask = 'Ask [[tab:rev-codex]] to review calc.js in this folder for bugs. Fix every bug it reports, ' +
    'then ask it to review again. Repeat until it reports no remaining bugs. ' +
    `Then reply to me with only ${mark}- followed by how many reviews it took, like ${mark}-N.`;
  const from = logLines().length;
  const t0 = Date.now();
  await door('send_to_tab', 'rev-claude', ask);
  const done = await settles('rev-claude', async () => said.test(await screen('rev-claude')), from, 'rev-codex', 30 * 60000);
  const out = await screen('rev-claude');
  const events = askEvents(from);
  const rounds = events.filter((l) => l.includes('asks rev-codex')).length;
  const tests = spawnSync('node', [HIDDEN], { encoding: 'utf8' });
  const fixed = tests.status === 0;
  const pass = done.ok && fixed && said.test(out) && rounds >= 2;
  const why = pass ? '' : [!done.ok && `caller ended ${done.why}`, !fixed && 'hidden tests fail',
    !said.test(out) && 'no final mark', rounds < 2 && `only ${rounds} round(s)`].filter(Boolean).join('; ');
  record({ kind: 'loop', pair: 'rev-claude->rev-codex', n, pass, why, ms: Date.now() - t0, rounds, events });
  if (!pass) fs.writeFileSync(path.join(OUT, `fail-loop-${n}.txt`), out + '\n\n--- tests ---\n' + tests.stderr);
};

const summary = () => {
  const groups = {};
  for (const r of results) (groups[`${r.kind} ${r.pair}`] ||= []).push(r);
  console.log('\n=== success rate ===');
  let all = 0, ok = 0;
  for (const [k, rs] of Object.entries(groups)) {
    const p = rs.filter((r) => r.pass).length;
    all += rs.length; ok += p;
    const secs = rs.map((r) => r.ms / 1000).sort((x, y) => x - y);
    console.log(`${k}: ${p}/${rs.length} (${Math.round((100 * p) / rs.length)}%), median ${Math.round(secs[secs.length >> 1])}s`);
  }
  if (all) console.log(`all: ${ok}/${all} (${Math.round((100 * ok) / all)}%)`);
};

try {
  await openDoor();
  console.log(`the copy is up (pid ${pid}); bringing the AIs up`);
  for (const id of Object.keys(TABS)) {
    await until(async () => (await state(id)) !== undefined, `tab ${id}`, 60000);
    await bringUp(id);
    console.log(`  ${id} is up`);
  }
  await sleep(3000);
  const pairs = [['claude-a', 'codex-b'], ['codex-b', 'claude-c'], ['claude-a', 'claude-c']];
  for (const kind of ['short', 'long']) {
    if (ONLY && ONLY !== kind) continue;
    for (let n = 1; n <= COUNTS[kind]; n++) {
      for (const [caller, callee] of pairs) await secretTrial(kind, n, caller, callee);
    }
  }
  if (!ONLY || ONLY === 'loop') {
    for (let n = 1; n <= COUNTS.loop; n++) await loopTrial(n);
  }
} catch (e) {
  console.error('stopped: ' + e.message);
} finally {
  summary();
  if (!process.argv.includes('--keep')) stopApp();
}
