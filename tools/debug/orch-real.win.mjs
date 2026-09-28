/**
 * A job handed out between AI tabs, with the AIs themselves: does a real lead
 * see "fix, review, fix again, until the review finds nothing" through?
 *
 * Three tabs of this checkout's build in one repository: a lead (Claude Code),
 * a coder (Claude Code) and a reviewer (Codex). The person's request goes to
 * the lead through the app's own door, as a person's words, naming the other
 * two with @. Nothing tells the lead which commands to use beyond the skill,
 * which sits in the folder as a project skill.
 *
 *     cargo build
 *     node tools/debug/orch-real.win.mjs [--trials=2] [--kind=once|loop] [--exe=<path>] [--keep]
 *
 * A trial passes when: tests the AIs never see pass, the lead handed the work
 * out with dispatch (the fix and at least one review), every assignment was
 * reported, the job was closed, and the lead ended with the mark it was asked
 * for. Results go to target/orch-real/results.json.
 *
 * Needs Windows, Node, git, and `claude` and `codex` signed in. Spends real
 * turns of both accounts: a handful of trials needs no asking, a hundred does.
 * Isolated: its own folder and LOCALAPPDATA; the person's own skills and
 * settings are not touched.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const OUT = path.join(ROOT, 'target', 'orch-real');
const arg = (name, dflt) => {
  const a = process.argv.find((x) => x.startsWith(`--${name}=`));
  return a ? a.slice(name.length + 3) : dflt;
};
const TRIALS = Number(arg('trials', 2));
// once: the fix is asked for whole, and one review usually finds nothing.
// loop: the first fix is asked for one function only, so the review finds the
// rest and the job has to go round again -- fix, review, fix, review
const KIND = arg('kind', 'once');
const RUN = path.join(os.tmpdir(), 'sk-orch-real');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'repo');
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
const git = (...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd: REPO, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
  return r.stdout;
};
const nonce = () => Math.random().toString(36).slice(2, 8).toUpperCase();

// --exe=<path> runs another build instead of this checkout's: the one
// installed, say, to try exactly what people will run
const exe = (process.argv.find((a) => a.startsWith('--exe=')) || '').slice(6) || path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
for (const cli of ['claude', 'codex']) {
  if (spawnSync('where.exe', [cli], { encoding: 'utf8' }).status !== 0) die(`${cli} is not on PATH`);
}

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, REPO, path.join(RUN, 'localappdata'), OUT]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// Three bugs, and tests kept outside the folder the AIs work in
const CALC = [
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
].join('\n');
fs.writeFileSync(HIDDEN, [
  "const assert = require('assert');",
  `const { sum, average, discounted } = require(${JSON.stringify(path.join(REPO, 'calc.js'))});`,
  'assert.strictEqual(sum([1, 2, 3]), 6);',
  'assert.strictEqual(average([]), 0);',
  'assert.strictEqual(average([2, 4]), 3);',
  'assert.strictEqual(discounted(200, 10), 180);',
  "console.log('ok');",
].join('\n'));
fs.writeFileSync(path.join(REPO, 'calc.js'), CALC);
const skillText = spawnSync(appExe, ['--cli', 'skill'], { encoding: 'utf8' }).stdout;
if (!skillText.includes('name: shikisha')) die('the app did not print its skill');
for (const where of ['.claude', '.agents']) {
  const at = path.join(REPO, where, 'skills', 'shikisha');
  fs.mkdirSync(at, { recursive: true });
  fs.writeFileSync(path.join(at, 'SKILL.md'), skillText);
}
fs.writeFileSync(path.join(REPO, '.gitignore'), '.claude/\n.agents/\n');
git('init', '-q');
git('add', '.');
git('commit', '-q', '-m', 'start');

const claudeCmd = ['claude', '--dangerously-skip-permissions'];
const codexCmd = ['codex', '--dangerously-bypass-approvals-and-sandbox'];
const TABS = { lead: claudeCmd, coder: claudeCmd, reviewer: codexCmd };
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  external_api: { access: 'user' },
  desks: [{ name: 'ORCH', id: 'orch', folders: [{ cwd: REPO, tabs: Object.entries(TABS).map(([id, command]) => ({ name: id, id, command })) }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

let door;
const openDoor = async () => {
  const end = Date.now() + 60000;
  const file = path.join(APP, 'data', 'api-token');
  while (!fs.existsSync(file) && Date.now() < end) await sleep(500);
  if (!fs.existsSync(file)) die('no api-token appeared');
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
const ready = {
  claude: /bypass permissions|for shortcuts|\? for/i,
  codex: /Ask Codex to do anything|YOLO mode|send\s+⏎|⏎ send|context left|Ctrl\+J newline|esc to interrupt|\/ commands|full access/i,
};
const bringUp = async (id) => {
  const kind = id === 'reviewer' ? 'codex' : 'claude';
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
  throw new Error(`${id} did not come up:\n${await screen(id)}`);
};
const logLines = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/) : []);

const results = [];
const trial = async (n) => {
  git('checkout', '-q', '--', '.');
  git('clean', '-qfd');
  const mark = `JOB${nonce()}`;
  const said = new RegExp(`${mark}-\\d`);
  const first = KIND === 'loop'
    ? `Have <@coder> fix only the bug in sum() in calc.js in this folder (nothing else yet), and have <@reviewer> review the whole of calc.js. `
    : `Have <@coder> fix the bugs in calc.js in this folder, and have <@reviewer> review the fix. `;
  const ask = first +
    `If the review finds anything, have <@coder> fix it and <@reviewer> review again, until the review finds nothing. ` +
    `See the whole job through. When it is done, reply to me with only ${mark}- followed by how many reviews there were, like ${mark}-N.`;
  const from = logLines().length;
  const t0 = Date.now();
  await door('send_to_tab', 'lead', ask);
  let quietSince = null;
  let ended = 'TIMEOUT';
  while (Date.now() - t0 < 40 * 60000) {
    const st = await state('lead').catch(() => '?');
    if (['QUESTION', 'EXIT', 'FAILED', 'LIMIT'].includes(st)) { ended = st; break; }
    const out = await screen('lead');
    const log = logLines().slice(from);
    const open = log.some((l) => /orchestration: t\d+ added/.test(l)) && !log.some((l) => /orchestration: r\d+ closed/.test(l));
    if (said.test(out) && ['DONE', 'WAIT'].includes(st)) { ended = 'DONE'; break; }
    // Quiet with the job open is a lead waiting for the line that says mail came
    if (['DONE', 'WAIT'].includes(st) && !open && log.length) { quietSince ??= Date.now(); if (Date.now() - quietSince > 180000) { ended = 'QUIET'; break; } }
    else quietSince = null;
    await sleep(3000);
  }
  const out = await screen('lead');
  const log = logLines().slice(from);
  const events = log.filter((l) => l.includes('orchestration:'));
  const dispatched = events.filter((l) => / -> /.test(l)).length;
  const reported = events.filter((l) => /reported/.test(l)).length;
  const closed = events.some((l) => /closed/.test(l));
  const reviews = events.filter((l) => / -> reviewer/.test(l)).length;
  const tests = spawnSync('node', [HIDDEN], { encoding: 'utf8' });
  const fixed = tests.status === 0;
  const pass = fixed && dispatched >= 2 && reviews >= (KIND === 'loop' ? 2 : 1) && reported >= dispatched && closed && said.test(out);
  const why = pass ? '' : [ended !== 'DONE' && `lead ended ${ended}`, !fixed && 'hidden tests fail',
    dispatched < 2 && `only ${dispatched} dispatch(es)`, reviews < (KIND === 'loop' ? 2 : 1) && `only ${reviews} review(s)`,
    reported < dispatched && `${dispatched - reported} unreported`, !closed && 'job not closed', !said.test(out) && 'no final mark']
    .filter(Boolean).join('; ');
  const r = { kind: KIND, n, pass, why, ms: Date.now() - t0, dispatched, reviews, reported, closed, events };
  results.push(r);
  fs.writeFileSync(path.join(OUT, `results-${KIND}.json`), JSON.stringify(results, null, 2));
  fs.writeFileSync(path.join(OUT, `lead-${KIND}-${n}.txt`), out);
  console.log(`${pass ? 'PASS' : 'FAIL'} #${n} ${Math.round(r.ms / 1000)}s dispatched=${dispatched} reviews=${reviews} reported=${reported} closed=${closed}` + (pass ? '' : ` -- ${why}`));
  for (const e of events) console.log('    ' + e.replace(/^.*orchestration: /, ''));
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
  for (let n = 1; n <= TRIALS; n++) await trial(n);
} catch (e) {
  console.error('stopped: ' + e.message);
} finally {
  const ok = results.filter((r) => r.pass).length;
  console.log(`\n${ok}/${results.length} passed`);
  if (!process.argv.includes('--keep')) stopApp();
}
