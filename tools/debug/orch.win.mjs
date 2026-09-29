/**
 * A job handed out between AI tabs, end to end, with stand-ins for the AIs.
 *
 * This checkout's build in a folder of its own, with three stand-in AIs in one
 * folder: a lead (`claude.cmd`), a coder (`claude.cmd`) and a reviewer
 * (`codex.cmd`), so the app takes them for Claude Code and Codex. Each is a
 * script that does what an AI following the skill would do, through the real
 * `shikisha` command on its PATH, and writes down everything it was sent and
 * everything the app answered.
 *
 *     cargo build
 *     node tools/debug/orch.win.mjs            (--keep leaves the copy up)
 *
 * The person's words go in through the input bar, naming the two workers with
 * @, exactly as a person would send them. Then the lead:
 *
 *   1. opens a job and a task, and assigns it to the coder. The coder is
 *      handed its brief as a typed line and a paste (Claude takes pasted
 *      instructions only when typed words ask for them)
 *   2. waits on its inbox. The coder asks it a question (ask_lead); the lead
 *      answers from the inbox; the coder carries on and reports
 *   3. adds a review task that waits on the first, assigns it to the
 *      reviewer (Codex: the paste alone), and ends its turn without waiting
 *   4. is told in its own tab that a message came when the review is in (the
 *      line the app types), reads it, lets both workers go and closes the job
 *
 * Checked along the way: every answer that moves the job on carries `next`;
 * a second report is "already"; the job card is on the board while the job is
 * open and gone once it is closed; an assign to a tab the person did not
 * name is refused with the way to open one.
 *
 * Needs Windows and Node. No account is used and nothing leaves the machine.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9353;
const RUN = path.join(os.tmpdir(), 'sk-orch');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const STUB = path.join(RUN, 'stub');
const LOGS = path.join(RUN, 'logs');
const CONFIG = path.join(APP, 'config', 'config.json');
const LEAD_LINE = 'Another AI tab in SHIKISHA-TERM has handed you the task pasted below';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

// --exe=<path> runs another build instead of this checkout's: the one
// installed, say, to try exactly what people will run. What it reads beside
// itself then comes from beside it too, not from this checkout
const installed = (process.argv.find((a) => a.startsWith('--exe=')) || '').slice(6);
const exe = installed || path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
const fromCopy = installed ? ['-From', path.dirname(installed)] : [];
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, STUB, LOGS, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });

// The stand-in. Reads what it is sent up to each Enter, writes it down, and
// acts on it as its role says, through the app's own command. The command is
// found where every tab finds it -- first on the PATH -- and the program it
// names is run directly, so arguments reach it untouched by a shell
fs.writeFileSync(path.join(STUB, 'agent.js'), String.raw`
const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const [role, logs] = process.argv.slice(2);
const heard = path.join(logs, 'heard-' + role + '.txt');
const said = path.join(logs, 'said-' + role + '.jsonl');
const bin = (process.env.PATH || process.env.Path || '').split(';')[0];
const shim = fs.readFileSync(path.join(bin, 'shikisha.cmd'), 'utf8');
const app = shim.match(/"([^"]+)"/)[1];
const sh = (...args) => {
  const r = spawnSync(app, ['--cli', ...args], { encoding: 'utf8', env: process.env });
  const out = { args, code: r.status, out: (r.stdout || '').trim(), err: (r.stderr || '').trim() };
  fs.appendFileSync(said, JSON.stringify(out) + '\n');
  return out;
};
const state = (s) => sh('set_state', s);
const json = (o) => { try { return JSON.parse(o.out.split('\n\n[shikisha]')[0]); } catch { return {}; } };
if (process.stdin.isTTY) process.stdin.setRawMode(true);
// Bracketed paste on, as every AI CLI turns it on: a paste arrives between
// ESC[200~ and ESC[201~, and an Enter inside it is part of the text
process.stdout.write('\x1b[?2004h' + role + ' ready\r\n> ');
let buf = '';
let pasting = false;
let busy = false;
const queue = [];
const OPEN = '\x1b[200~', SHUT = '\x1b[201~';
process.stdin.on('data', (d) => {
  let rest = d.toString('utf8');
  while (rest.length) {
    if (!pasting && rest.startsWith(OPEN)) { pasting = true; buf += OPEN; rest = rest.slice(OPEN.length); continue; }
    if (pasting && rest.startsWith(SHUT)) { pasting = false; buf += SHUT; rest = rest.slice(SHUT.length); continue; }
    const c = rest[0];
    rest = rest.slice(1);
    if (c === '\r' && !pasting) {
      const line = buf;
      buf = '';
      const text = line.replace(/\x1b\[20[01]~/g, '');
      if (!text.trim()) continue;
      fs.appendFileSync(heard, '<<<' + line + '>>>\n');
      queue.push(text);
      continue;
    }
    buf += c;
  }
  pump();
});
const pump = async () => {
  if (busy || !queue.length) return;
  busy = true;
  const text = queue.shift();
  state('BUSY');
  try { await act(text); } catch (e) { fs.appendFileSync(said, JSON.stringify({ crashed: String(e) }) + '\n'); }
  process.stdout.write('\r\n> ');
  state('DONE');
  busy = false;
  pump();
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// Read the inbox until a batch with a message of one of these kinds comes,
// answering questions and saying each handover is dealt with on the way
const drain = async (kinds, tries = 12) => {
  let dealt = null;
  for (let n = 0; n < tries; n++) {
    const got = sh('inbox', 'wait', ...(dealt ? [JSON.stringify({ dealt })] : []));
    const v = json(got);
    dealt = null;
    if (!v.mail || !v.mail.length) continue;
    dealt = v.handover;
    for (const m of v.mail) if (m.kind === 'question') sh('answer', m.question, 'src/app.rs');
    const hit = v.mail.find((m) => kinds.includes(m.kind));
    if (hit) { sh('inbox', JSON.stringify({ dealt })); return hit; }
  }
  return null;
};
const act = async (text) => {
  if (role === 'lead' && text.includes('GO')) {
    sh('job_open', 'Fix the parser and get it reviewed');
    sh('task_add', 'Implement the fix in src/app.rs. Done when it builds.');
    sh('assign', 't1', 'shell');   // not an AI: refused
    sh('assign', 't1', 'coder');
    const first = await drain(['report']);
    fs.appendFileSync(said, JSON.stringify({ first }) + '\n');
    sh('task_add', 'Review the fix in src/app.rs.', JSON.stringify({ waits_on: ['t1'] }));
    sh('assign', 't2', 'reviewer');
    return;   // ends its turn: the review comes back as a line in this tab
  }
  if (role === 'lead' && text.includes('New mail for this tab')) {
    const v = json(sh('inbox'));
    fs.appendFileSync(said, JSON.stringify({ pointed: v }) + '\n');
    if (v.handover) sh('inbox', JSON.stringify({ dealt: v.handover }));
    sh('let_go', 'a1');
    sh('let_go', 'a2');
    sh('job_close', 'Fixed and reviewed');
    sh('job_status');
    return;
  }
  if (text.includes('--- t')) {
    await sleep(800);
    if (role === 'coder') sh('ask_lead', 'Which file?');
    await sleep(800);
    sh('report', 'done', role + ' did it.', 'Nothing odd.', 'nothing');
    sh('report', 'done', 'again');
  }
};
`);
const standIn = (role, cli) => {
  const dir = path.join(STUB, role);
  fs.mkdirSync(dir, { recursive: true });
  const cmd = path.join(dir, cli + '.cmd');
  fs.writeFileSync(cmd, `@"${process.execPath}" "${path.join(STUB, 'agent.js')}" ${role} "${LOGS}"\r\n`);
  return [cmd];
};

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe, ...fromCopy);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  desks: [{ name: 'Orch', id: 'orch', folders: [{ cwd: WORK, tabs: [
    { name: 'lead', id: 'lead', command: standIn('lead', 'claude') },
    { name: 'coder', id: 'coder', command: standIn('coder', 'claude') },
    { name: 'reviewer', id: 'reviewer', command: standIn('reviewer', 'codex') },
    { name: 'shell', id: 'shell', command: 'cmd.exe' },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const pages = async () => {
  try { return (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { return []; }
};
const connect = async (target) => {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  return { ws, send, run };
};
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};
const said = (role) => {
  const f = path.join(LOGS, 'said-' + role + '.jsonl');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)) : [];
};
const heard = (role) => {
  const f = path.join(LOGS, 'heard-' + role + '.txt');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '';
};
const call = (role, name) => said(role).filter((s) => s.args && s.args[0] === name);

let board;
try {
  let targets = [];
  await until(async () => (targets = await pages()).length > 0, 'the app\'s page', 30000);
  board = await connect(targets[0]);
  const { run } = board;
  await until(() => run(`["lead","coder","reviewer"].every(id => (S.tabs || []).find(t => t.id === id && t.ai))`),
    'the stand-ins to be read as AIs', 30000);
  const lead = await run(`S.tabs.find(t => t.id === "lead").index`);
  await run(`send({kind:"select", tab:${lead}}); true`);
  await until(() => run(`S.active === ${lead}`), 'the lead in front');
  await sleep(2000);

  console.log('1. the person asks, naming the workers with @');
  await run(`send({kind:"say", tab:${lead}, text:"GO: have <@coder> fix it and <@reviewer> review it"}); true`);
  await until(async () => call('lead', 'assign').length >= 2, 'the lead to assign', 30000);
  const [refused, first] = call('lead', 'assign');
  check(refused.code !== 0 && /tab_run/.test(refused.err), 'a terminal is refused, with the command that fits it: ' + refused.err);
  check(first.code === 0 && /\[shikisha\] next: shikisha inbox wait/.test(first.out), 'the assign answers with what to run next');
  check(call('lead', 'job_open')[0]?.code === 0 && /next: shikisha task_add/.test(call('lead', 'job_open')[0].out), 'job_open says to add a task next');

  console.log('2. the coder is handed its brief, typed line first');
  await until(async () => heard('coder').includes('--- t1 ---'), 'the brief', 30000);
  const h = heard('coder');
  check(h.includes(LEAD_LINE) && h.indexOf(LEAD_LINE) < h.indexOf('\x1b[200~'), 'Claude gets the typed request before the paste');
  check(h.includes('shikisha report done'), 'the brief carries the report command');
  await until(() => run(`(S.jobs || []).length === 1 && S.jobs[0].tasks.length >= 1`), 'the job card', 10000);
  check(true, 'the job is on the board');

  console.log('3. a question reaches the lead and its answer the coder');
  await until(async () => call('coder', 'report').length >= 2, 'the coder to report', 60000);
  const ask = call('coder', 'ask_lead')[0];
  check(ask && ask.code === 0 && ask.out.includes('src/app.rs'), 'ask_lead answered: ' + (ask && ask.out.split('\n').slice(0, 3).join(' ')));
  const [rep, again] = call('coder', 'report');
  check(rep.code === 0 && /reported/.test(rep.out), 'the report is taken');
  check(again.code === 0 && /already reported/.test(again.out), 'a second report is "already"');

  console.log('4. the review comes back as a line in the lead\'s tab');
  await until(async () => heard('reviewer').includes('--- t2 ---'), 'the review brief', 60000);
  const r = heard('reviewer');
  check(!r.includes(LEAD_LINE), 'Codex gets the paste alone');
  await until(async () => heard('lead').includes('New mail for this tab (1)'), 'the line saying there is mail', 60000);
  check(true, 'the lead was told in its tab');
  await until(async () => call('lead', 'job_close').length >= 1, 'the lead to close the job', 60000);
  const close = call('lead', 'job_close')[0];
  check(close.code === 0 && /closed/.test(close.out), 'the job is closed: ' + (close.out || close.err).split('\n')[0]);
  const letGo = call('lead', 'let_go');
  check(letGo.length === 2 && letGo.every((x) => x.code === 0), 'both workers let go');
  await until(() => run(`(S.jobs || []).length === 0`), 'the job card to go', 10000);
  check(true, 'the card is gone once the job is closed');
} catch (e) {
  failures += 1;
  console.error('stopped: ' + e.message);
  for (const role of ['lead', 'coder', 'reviewer']) {
    console.error(`--- ${role} said:`);
    for (const s of said(role)) console.error(JSON.stringify(s).slice(0, 400));
  }
} finally {
  try { board?.ws.close(); } catch {}
  if (!process.argv.includes('--keep')) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
