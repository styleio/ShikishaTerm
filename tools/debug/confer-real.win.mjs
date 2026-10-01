/**
 * AIConfer with the AIs themselves: one AI tab asks another through ask_tab,
 * and what the conference records is checked -- the asker's line, the answer
 * kept whole, and the answer's own line (said in its own words after the stop
 * hook held its turn's end once, or its first sentence when it could not be).
 *
 *     cargo build
 *     node tools/debug/confer-real.win.mjs [--n=10] [--pair=claude-a,codex-b] [--keep]
 *
 * Pairs: claude-a -> codex-b, codex-b -> claude-c, claude-a -> claude-c, --n
 * asks each. A trial passes when the asker reports what only the other tab's
 * folder holds (as ask-tab-poc does), the ask is in the record with a line,
 * the reply kept holds that text (the answer, not a word said after it), and
 * the answer has a line. How many answers said their own line and how many
 * had their first sentence taken is counted apart.
 *
 * Needs Windows, Node 22, git, and `claude` and `codex` signed in on this
 * machine. Spends real turns of both accounts.
 *
 * Isolated all the way down, as agent-hooks-consent.win.mjs is: a home folder
 * of its own (USERPROFILE, HOME, CODEX_HOME), so the hooks this copy writes
 * and Codex's approval of them go into this run's .claude and .codex, never
 * the person's. The two CLIs' sign-in files are copied into that home for the
 * run, and the whole of it is deleted at the end (--keep leaves it).
 * Results go to target/confer-real/results.json.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const OUT = path.join(ROOT, 'target', 'confer-real');
const arg = (name, dflt) => {
  const a = process.argv.find((x) => x.startsWith(`--${name}=`));
  return a ? a.slice(name.length + 3) : dflt;
};
const N = Number(arg('n', 10));
const KEEP = process.argv.includes('--keep');
const RUN = path.join(os.tmpdir(), 'sk-confer');
const APP = path.join(RUN, 'app');
const HOME = path.join(RUN, 'home');
const WORK = path.join(RUN, 'work');
const F = { a: path.join(WORK, 'fa'), b: path.join(WORK, 'fb'), c: path.join(WORK, 'fc') };
const CONFIG = path.join(APP, 'config', 'config.json');
const TOKEN = 'confer-check-token-0123456789';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const nonce = () => Math.random().toString(36).slice(2, 8).toUpperCase();
const freePort = () => new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
// A check of an older build says nothing about the code in front of you
// (cargo test does not build the app itself)
{
  const built = fs.statSync(exe).mtimeMs;
  const newer = (dir) => fs.readdirSync(dir, { withFileTypes: true }).some((e) => {
    const p = path.join(dir, e.name);
    return e.isDirectory() ? newer(p) : fs.statSync(p).mtimeMs > built;
  });
  if (['src', path.join('crates', 'core', 'src')].some((d) => newer(path.join(ROOT, d)))) {
    die('the build at target\\debug is older than the sources -- run cargo build first');
  }
}
for (const cli of ['claude', 'codex']) {
  if (spawnSync('where.exe', [cli], { encoding: 'utf8' }).status !== 0) die(`${cli} is not on PATH`);
}
const REAL = os.homedir();
const SIGN_INS = [
  [path.join(REAL, '.claude', '.credentials.json'), path.join(HOME, '.claude', '.credentials.json')],
  [path.join(REAL, '.claude.json'), path.join(HOME, '.claude.json')],
  [path.join(REAL, '.codex', 'auth.json'), path.join(HOME, '.codex', 'auth.json')],
];
for (const [from] of SIGN_INS) if (!fs.existsSync(from)) die(`not signed in: ${from} is missing`);

// ── The copy, its home, its folders and its tabs ───────────────────────────
console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, HOME, path.join(HOME, '.claude'), path.join(HOME, '.codex'), ...Object.values(F),
  path.join(RUN, 'localappdata'), OUT]) fs.mkdirSync(d, { recursive: true });
for (const [from, to] of SIGN_INS) fs.copyFileSync(from, to);
// Running without asking is agreed to once, here: it is what the tabs are
// started with, and the question it asks would stand in front of every tab
fs.writeFileSync(path.join(HOME, '.claude', 'settings.json'), JSON.stringify({ skipDangerousModePermissionPrompt: true }, null, 2));
// Codex's notice about WSL, answered once here for the same reason, and its
// sandbox kept to the one that needs no administrator: the tabs run with it
// bypassed, and the administrator one stops a fresh home at a question
// Never an update: a CLI updating itself here updates the one this PC runs
// everywhere (2026-10-01 an Enter pressed at Codex's update question ran
// npm install -g and moved this PC from 0.155 to 0.159)
fs.writeFileSync(path.join(HOME, '.codex', 'config.toml'),
  'windows_wsl_setup_acknowledged = true\ncheck_for_update_on_startup = false\n\n[windows]\nsandbox = "unelevated"\n');
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
const skillText = spawnSync(appExe, ['--cli', 'skill'], { encoding: 'utf8' }).stdout;
if (!skillText.includes('name: shikisha')) die('the app did not print its skill:\n' + skillText);
for (const dir of Object.values(F)) {
  for (const where of ['.claude', '.agents']) {
    const at = path.join(dir, where, 'skills', 'shikisha');
    fs.mkdirSync(at, { recursive: true });
    fs.writeFileSync(path.join(at, 'SKILL.md'), skillText);
  }
  spawnSync('git', ['init', '-q'], { cwd: dir });
}

const claudeCmd = ['claude', '--dangerously-skip-permissions'];
const codexCmd = ['codex', '--dangerously-bypass-approvals-and-sandbox'];
const TABS = {
  'claude-a': { cwd: F.a, command: claudeCmd },
  'codex-b': { cwd: F.b, command: codexCmd },
  'claude-c': { cwd: F.c, command: claudeCmd },
};
const port = await freePort();
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: true, bind: '127.0.0.1', port, sticky_token: true, fixed_token: TOKEN },
  external_api: { access: 'user' },
  // The hooks agreed to, so this copy writes them -- into this run's home
  agent_hooks: { 'Claude Code': 'on', 'Codex CLI': 'on' },
  desks: [{ name: 'Confer', id: 'confer', folders: Object.entries(TABS).map(([id, t]) => ({ cwd: t.cwd, tabs: [{ name: id, id, command: t.command }] })) }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX_HOME)/i.test(k)));
Object.assign(env, { LOCALAPPDATA: path.join(RUN, 'localappdata'), USERPROFILE: HOME, HOME, CODEX_HOME: path.join(HOME, '.codex'),
  DISABLE_AUTOUPDATER: '1' });
const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

// ── The door: the pipe, as the person ─────────────────────────────────────
const dataDir = () => [path.join(APP, 'data'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data')]
  .find((d) => fs.existsSync(path.join(d, 'api-token')));
let door;
const openDoor = async () => {
  const end = Date.now() + 60000;
  let dir;
  while (!(dir = dataDir()) && Date.now() < end) await sleep(500);
  if (!dir) die('no api-token appeared');
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
  const hello = await line({ token: fs.readFileSync(path.join(dir, 'api-token'), 'utf8').trim() });
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
const hooksLog = () => {
  const f = path.join(APP, 'logs', 'hooks.log');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8').split(/\r?\n/) : [];
};

// ── The hooks, agreed to the way a person agrees ───────────────────────────
// The settings say yes, but this run's home holds none of the hooks: the
// copy asks again before writing them (agenthook::asked_again), on the board.
// Answered there, the hooks are written into this home, and the AI tabs --
// started before them -- are started again to read them
const agreeToHooks = async () => {
  let cookies = '';
  const base = `http://127.0.0.1:${port}`;
  const board = async () => {
    const r = await fetch(`${base}/?t=${TOKEN}`, { redirect: 'manual', signal: AbortSignal.timeout(10000) });
    cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
  };
  const state = async () => (await fetch(`${base}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) })).json();
  await until(async () => { await board(); return /(^|; )rs=/.test(cookies); }, 'the board', 60000);
  await until(async () => !!(await state()).ui?.hook_ask, 'the question about the hooks', 60000);
  const asked = (await state()).ui.hook_ask;
  await fetch(`${base}/api/intent?t=${TOKEN}`, { method: 'POST', headers: { Cookie: cookies },
    body: JSON.stringify({ kind: 'agenthooks', answer: 'on', seq: asked.seq }), signal: AbortSignal.timeout(10000) });
  const written = (f) => fs.existsSync(f) && fs.readFileSync(f, 'utf8').includes('--hook') && fs.readFileSync(f, 'utf8').includes('line');
  await until(async () => written(path.join(HOME, '.claude', 'settings.json')) && written(path.join(HOME, '.codex', 'hooks.json')),
    'the hooks to be written', 60000);
  for (const id of Object.keys(TABS)) await door('restart', id, 'fresh');
  await sleep(3000);
};

// ── Bringing each CLI up to its prompt ─────────────────────────────────────
const ready = {
  claude: /bypass permissions|for shortcuts|\? for/i,
  codex: /Ask Codex to do anything|YOLO mode|send\s+⏎|⏎ send|context left|Ctrl\+J newline|esc to interrupt|\/ commands|full access/i,
};
const bringUp = async (id) => {
  const kind = id.includes('codex') ? 'codex' : 'claude';
  // A Codex with a home it has not seen sets its sandbox up first, for some
  // minutes, and drops what is typed meanwhile
  const end = Date.now() + 10 * 60000;
  while (Date.now() < end) {
    const s = await screen(id);
    if (/Setting up sandbox|Input disabled until setup/i.test(s)) { await sleep(3000); continue; }
    // ...and when the administrator sandbox cannot be had, the other one
    if (/Couldn't set up your sandbox/i.test(s) && /non-admin sandbox/i.test(s)) {
      await door('send', id, '2'); await sleep(300); await door('send', id, '\r'); await sleep(3000); continue;
    }
    if (/I trust this folder|Do you trust|trust the files|allow Codex to work|Trust this folder\?[\s\S]*Trust and continue/i.test(s)) {
      if (kind === 'claude') { await door('send', id, '\x1b[B'); await sleep(400); }
      await door('send', id, '\r');
      await sleep(2500);
      continue;
    }
    // Running without asking, asked about once: "Yes, I accept" is the second choice
    if (/Bypass Permissions mode/i.test(s) && /Yes, I accept/i.test(s)) {
      await door('send', id, '\x1b[B'); await sleep(400); await door('send', id, '\r'); await sleep(2500); continue;
    }
    // An update offered is put away, never taken (see config.toml above);
    // a question this does not know is left on the screen and fails the run,
    // rather than answered with whatever its first choice is
    if (/Update available|Update now/i.test(s) && /Skip/i.test(s)) {
      await door('send', id, '\x1b'); await sleep(2000); continue;
    }
    if (/Press enter to confirm|Enter to confirm/i.test(s)) { await sleep(1000); continue; }
    if (ready[kind].test(s)) return true;
    await sleep(1000);
  }
  console.log(`--- ${id} did not come up; its screen ---\n${await screen(id)}\n---`);
  throw new Error(`${id} did not come up`);
};

// ── What the conference recorded ───────────────────────────────────────────
const recorded = (sinceMs, target) => {
  const db = new DatabaseSync(path.join(dataDir(), 'conversations.db'), { readOnly: true });
  try {
    const ask = db.prepare('SELECT id, caller, target, text, reply, state, thread_id FROM asks WHERE target = ? AND asked_at >= ? ORDER BY id DESC LIMIT 1')
      .get(target, sinceMs);
    if (!ask) return { ask: null, lines: [], members: [] };
    const lines = db.prepare('SELECT tab, text, how, thread_id FROM lines WHERE ask_id = ? ORDER BY id').all(ask.id);
    const members = db.prepare('SELECT tab FROM thread_tabs WHERE thread_id = ? ORDER BY tab').all(ask.thread_id).map((r) => r.tab);
    return { ask, lines, members };
  } finally { db.close(); }
};

const QUIET = ['DONE', 'WAIT', 'BACKGROUND'];
const results = [];
const record = (r) => {
  results.push(r);
  fs.writeFileSync(path.join(OUT, 'results.json'), JSON.stringify(results, null, 2));
  console.log(`${r.pass ? 'PASS' : 'FAIL'} ${r.pair} #${r.n} ${Math.round(r.ms / 1000)}s line=${r.answerHow || '-'}` +
    (r.pass ? '' : ` -- ${r.why}`));
};

const trial = async (n, caller, callee) => {
  const name = `memo-${nonce()}.txt`;
  const value = `${nonce()}-${nonce()}`;
  fs.writeFileSync(path.join(TABS[callee].cwd, name), value + '\n');
  const ask = `Ask <@${callee}> to tell you the text written in ${name} in its own folder. ` +
    'That memo is only in its folder. When you have its answer, reply to me with only that text.';
  const t0 = Date.now();
  const since = Date.now() - 1000;
  const logFrom = hooksLog().length;
  await door('send_to_tab', caller, ask);
  // Done when the asker shows the text and is quiet, and the ask is answered
  let pass = false, why = 'TIMEOUT';
  const end = Date.now() + 6 * 60000;
  while (Date.now() < end) {
    const st = await state(caller).catch(() => '?');
    if (['QUESTION', 'EXIT', 'FAILED', 'LIMIT'].includes(st)) { why = `caller ${st}`; break; }
    const rec = recorded(since, callee);
    if (QUIET.includes(st) && (await screen(caller)).includes(value) && rec.ask && rec.ask.state !== 'waiting') {
      pass = true; why = ''; break;
    }
    await sleep(1500);
  }
  await sleep(2000);
  const rec = recorded(since, callee);
  const asked = rec.lines.find((l) => l.how === 'ask');
  const answer = rec.lines.find((l) => l.how === 'said' || l.how === 'auto');
  const replyWhole = !!(rec.ask && rec.ask.reply && rec.ask.reply.includes(value));
  const held = hooksLog().slice(logFrom).some((l) => l.includes(`confer: ${callee} is asked for its line`));
  const problems = [
    !rec.ask && 'no ask in the record',
    rec.ask && rec.ask.thread_id == null && 'the ask is in no conversation',
    rec.ask && rec.lines.some((l) => l.thread_id !== rec.ask.thread_id) && 'a line is in another conversation than its ask',
    rec.ask && !(rec.members.includes(caller) && rec.members.includes(callee)) && `the conversation's members are ${rec.members.join(',')}`,
    rec.ask && !asked && 'the ask has no line',
    rec.ask && !replyWhole && `the reply kept is not the answer: ${JSON.stringify((rec.ask.reply || '').slice(0, 120))}`,
    rec.ask && !answer && 'the answer has no line',
  ].filter(Boolean);
  if (pass && problems.length) { pass = false; why = problems.join('; '); }
  else if (!pass && problems.length) why += '; ' + problems.join('; ');
  record({ pair: `${caller}->${callee}`, n, pass, why, ms: Date.now() - t0, value,
    askLine: asked && asked.text, answerLine: answer && answer.text, answerHow: answer && answer.how, held, reply: rec.ask && rec.ask.reply,
    thread: rec.ask && rec.ask.thread_id });
  if (!pass) fs.writeFileSync(path.join(OUT, `fail-${caller}-${callee}-${n}.txt`), await screen(caller));
};

const summary = () => {
  console.log('\n=== AIConfer with real AIs ===');
  const groups = {};
  for (const r of results) (groups[r.pair] ||= []).push(r);
  let all = 0, ok = 0, said = 0, auto = 0, held = 0;
  for (const [k, rs] of Object.entries(groups)) {
    const p = rs.filter((r) => r.pass).length;
    all += rs.length; ok += p;
    said += rs.filter((r) => r.answerHow === 'said').length;
    auto += rs.filter((r) => r.answerHow === 'auto').length;
    held += rs.filter((r) => r.held).length;
    const secs = rs.map((r) => r.ms / 1000).sort((x, y) => x - y);
    console.log(`${k}: ${p}/${rs.length}, median ${Math.round(secs[secs.length >> 1])}s, ` +
      `own line ${rs.filter((r) => r.answerHow === 'said').length}, first sentence ${rs.filter((r) => r.answerHow === 'auto').length}`);
  }
  if (all) console.log(`all: ${ok}/${all} passed; answer lines: ${said} in their own words, ${auto} first sentences; held once for a line: ${held}`);
  // Each asker's CLI conversation is one conversation of AIs, whoever it asks
  const by = {};
  for (const r of results) (by[r.pair.split('->')[0]] ||= new Set()).add(r.thread);
  console.log('conversations by asker:', Object.entries(by).map(([k, v]) => `${k}: ${v.size} (thread ${[...v].join(', ')})`).join('; '));
};

try {
  await openDoor();
  console.log(`the copy is up (pid ${pid}, board on port ${port}); agreeing to the hooks`);
  await agreeToHooks();
  console.log('the hooks are in; bringing the AIs up');
  for (const id of Object.keys(TABS)) {
    await until(async () => (await state(id)) !== undefined, `tab ${id}`, 60000);
    await bringUp(id);
    console.log(`  ${id} is up`);
  }
  await sleep(3000);
  const only = arg('pair', '').split(',').filter(Boolean);
  const pairs = [['claude-a', 'codex-b'], ['codex-b', 'claude-c'], ['claude-a', 'claude-c']]
    .filter(([a, b]) => !only.length || (a === only[0] && b === only[1]));
  for (let n = 1; n <= N; n++) {
    for (const [caller, callee] of pairs) await trial(n, caller, callee);
  }
} catch (e) {
  console.error('stopped: ' + e.message);
} finally {
  summary();
  if (!KEEP) {
    stopApp();
    await sleep(1500);
    fs.rmSync(HOME, { recursive: true, force: true });
    console.log('the run\'s home, with its copy of the sign-ins, is deleted');
  } else {
    console.log(`kept: ${RUN} (board: http://127.0.0.1:${port}/?t=${TOKEN}); delete ${HOME} when done -- it holds a copy of the sign-ins`);
  }
}
