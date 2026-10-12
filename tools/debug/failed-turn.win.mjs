/**
 * A turn the model will not take, with a real Codex: the tab reads as failed,
 * an ask of it is answered FAILED with what Codex said went wrong, and asking
 * again sends the words again rather than waiting on a tab taken for busy.
 * Beside it, a Codex that answers is asked the same way, so the change to
 * where a reply ends is seen to leave an ordinary answer whole.
 *
 *     cargo build
 *     node tools/debug/failed-turn.win.mjs [--keep]
 *
 * The failure is made by naming a model that does not exist (`-m`): the turn
 * goes to the server and comes back an error, which Codex files in its record
 * the way it files "the model is at capacity" -- a completion that carries the
 * error and no answer.
 *
 * Then the road a real review takes: an AI asks with the `shikisha` command,
 * which hands its wait over after 100 s, and the turn asked fails after that.
 * The tab asked talks to a model server this script runs, which holds the
 * first request past the handover and then refuses it; the AI that asked is
 * a real Codex, and the failure has to reach it through its inbox. Spends a
 * few real turns of the Codex account.
 *
 * Needs Windows, Node 22, and `codex` signed in on this machine. Isolated the
 * way confer-real.win.mjs is: a home folder of its own (USERPROFILE, HOME,
 * CODEX_HOME) with the sign-in copied in, deleted at the end (--keep leaves it).
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const KEEP = process.argv.includes('--keep');
const RUN = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-failed-'));
const INSTANCE = path.join(RUN, 'copy');
const APP = path.join(INSTANCE, 'app');
const HOME = path.join(RUN, 'home');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(RUN, 'config.json');
const TOKEN = 'failed-turn-check-token-0123456789';
const BAD_MODEL = 'shikisha-no-such-model';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8', windowsHide: true });
const launcher = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');
let cleaned = false;
const cleanup = () => {
  if (KEEP || cleaned) return;
  ps('-File', launcher, '-At', INSTANCE, '-Stop');
  if (fs.existsSync(HOME)) {
    if (fs.realpathSync(HOME) !== path.join(fs.realpathSync(RUN), 'home')) throw new Error('the test home no longer belongs to this run');
    fs.rmSync(HOME, { recursive: true, force: true });
  }
  cleaned = true;
};
process.on('exit', cleanup);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
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
if (spawnSync('where.exe', ['codex'], { encoding: 'utf8' }).status !== 0) die('codex is not on PATH');
const SIGN_IN = [path.join(os.homedir(), '.codex', 'auth.json'), path.join(HOME, '.codex', 'auth.json')];
if (!fs.existsSync(SIGN_IN[0])) die(`not signed in: ${SIGN_IN[0]} is missing`);

// ── The model server the slow tab talks to ─────────────────────────────────
// The first request is held past the `shikisha` command's own wait (100 s)
// and then refused; any after it are refused at once. A refusal Codex does
// not try again (400), so the turn ends in the error
const HOLD_MS = 110_000;
let modelAsked = 0;
const modelServer = http.createServer((req, res) => {
  req.resume();
  const refuse = () => {
    if (res.writableEnded) return;
    res.writeHead(400, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { type: 'invalid_request_error', message: 'The test model is at capacity.' } }));
  };
  if (req.method === 'POST' && modelAsked++ === 0) setTimeout(refuse, HOLD_MS);
  else refuse();
});
await new Promise((r) => modelServer.listen(0, '127.0.0.1', r));
const modelPort = modelServer.address().port;

// ── The copy, its home and its tabs ────────────────────────────────────────
const F = { bad: path.join(WORK, 'bad'), good: path.join(WORK, 'good'), slow: path.join(WORK, 'slow') };
for (const d of [APP, path.join(HOME, '.codex'), ...Object.values(F), path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
fs.copyFileSync(...SIGN_IN);
// Never an update, and the sandbox that needs no administrator (as confer-real).
// The test server as a provider of its own, used only by the tab that names it
fs.writeFileSync(path.join(HOME, '.codex', 'config.toml'),
  'windows_wsl_setup_acknowledged = true\ncheck_for_update_on_startup = false\n\n[windows]\nsandbox = "unelevated"\n\n' +
  `[model_providers.held]\nname = "held"\nbase_url = "http://127.0.0.1:${modelPort}/v1"\nwire_api = "responses"\n`);
// The AI that asks needs the skill that says how
const skillText = spawnSync(exe, ['--cli', 'skill'], { encoding: 'utf8', windowsHide: true }).stdout;
if (!skillText.includes('name: shikisha')) die('the app did not print its skill:\n' + skillText);
for (const dir of Object.values(F)) {
  const at = path.join(dir, '.agents', 'skills', 'shikisha');
  fs.mkdirSync(at, { recursive: true });
  fs.writeFileSync(path.join(at, 'SKILL.md'), skillText);
  spawnSync('git', ['init', '-q'], { cwd: dir });
}
const codex = ['codex', '--dangerously-bypass-approvals-and-sandbox'];
const TABS = {
  'codex-bad': { cwd: F.bad, command: [...codex, '-m', BAD_MODEL] },
  'codex-good': { cwd: F.good, command: codex },
  'codex-slow': { cwd: F.slow, command: [...codex, '-c', 'model_provider="held"', '-m', 'held-model'] },
};
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  keep_terminals: false,
  remote: { sticky_token: true, fixed_token: TOKEN },
  external_api: { access: 'user' },
  agent_hooks: { 'Codex CLI': 'on' },
  desks: [{ name: 'Failed', id: 'failed', folders: Object.entries(TABS).map(([id, t]) => ({ cwd: t.cwd, tabs: [{ name: id, id, command: t.command }] })) }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX_HOME)/i.test(k)));
Object.assign(env, { USERPROFILE: HOME, HOME, CODEX_HOME: path.join(HOME, '.codex'), TERM: 'xterm-256color' });
const started = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', launcher,
  '-At', INSTANCE, '-Work', WORK, '-Exe', exe, '-Config', CONFIG, '-Cdp', '0'], { env, encoding: 'utf8', windowsHide: true });
if (started.status !== 0) die('launch failed: ' + started.stdout + started.stderr);
const pid = Number(/^pid=(\d+)/m.exec(started.stdout)?.[1]);
const port = Number(/board=http:\/\/127\.0\.0\.1:(\d+)/.exec(started.stdout)?.[1]);
if (!pid || !port) die('the launcher did not identify its copy');
console.log('isolated copy: ' + RUN);

// ── The door: the pipe, as the person, a connection a call ─────────────────
const dataDir = () => [path.join(APP, 'data'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data')]
  .find((d) => fs.existsSync(path.join(d, 'api-token')));
const call = async (method, ...params) => {
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
  const hello = await line({ token: fs.readFileSync(path.join(dataDir(), 'api-token'), 'utf8').trim() });
  if (!hello.ok) throw new Error('the door refused');
  const a = await line({ id: '1', method, params });
  sock.destroy();
  if (!a.ok) throw new Error(`${method}: ${a.error}`);
  return a.result;
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};
const screen = (id) => call('tab_screen', id).then((s) => String(s || ''));
const state = (id) => call('state', id).then(String);
const hooksLog = () => {
  const f = path.join(APP, 'logs', 'hooks.log');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8').split(/\r?\n/) : [];
};

// The hooks, agreed to on the board the way a person agrees (as confer-real)
const agreeToHooks = async () => {
  let cookies = '';
  const base = `http://127.0.0.1:${port}`;
  const board = async () => {
    const r = await fetch(`${base}/?t=${TOKEN}`, { redirect: 'manual', signal: AbortSignal.timeout(10000) });
    cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
  };
  const ui = async () => (await fetch(`${base}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) })).json();
  await until(async () => { await board(); return /(^|; )rs=/.test(cookies); }, 'the board', 60000);
  await until(async () => !!(await ui()).ui?.hook_ask, 'the question about the hooks', 60000);
  const asked = (await ui()).ui.hook_ask;
  await fetch(`${base}/api/intent?t=${TOKEN}`, { method: 'POST', headers: { Cookie: cookies },
    body: JSON.stringify({ kind: 'agenthooks', answer: 'on', seq: asked.seq }), signal: AbortSignal.timeout(10000) });
  const hooks = path.join(HOME, '.codex', 'hooks.json');
  await until(() => fs.existsSync(hooks) && fs.readFileSync(hooks, 'utf8').includes('--hook'), 'the hooks to be written', 60000);
  for (const id of Object.keys(TABS)) await call('restart', id, 'fresh');
  await sleep(3000);
};

// Each Codex up to its prompt; a question this does not know fails the run
const bringUp = async (id) => {
  const end = Date.now() + 10 * 60000;
  while (Date.now() < end) {
    const s = await screen(id);
    if (/Setting up sandbox|Input disabled until setup/i.test(s)) { await sleep(3000); continue; }
    if (/Couldn't set up your sandbox/i.test(s) && /non-admin sandbox/i.test(s)) {
      await call('send', id, '2'); await sleep(300); await call('send', id, '\r'); await sleep(3000); continue;
    }
    if (/Do you trust|trust the files|allow Codex to work|Trust this folder\?/i.test(s)) { await call('send', id, '\r'); await sleep(2500); continue; }
    if (/Update available|Update now/i.test(s) && /Skip/i.test(s)) { await call('send', id, '\x1b'); await sleep(2000); continue; }
    if (/Ask Codex to do anything|YOLO mode|context left|Ctrl\+J newline|\/ commands|full access/i.test(s)) return;
    await sleep(1000);
  }
  console.log(`--- ${id} did not come up; its screen ---\n${await screen(id)}\n---`);
  throw new Error(`${id} did not come up`);
};

const checks = [];
const check = (ok, what, seen) => {
  checks.push(ok);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${what}${ok || seen === undefined ? '' : `\n     saw: ${typeof seen === 'string' ? seen : JSON.stringify(seen)}`}`);
};

try {
  await until(() => dataDir(), 'the api token', 60000);
  console.log(`the copy is up (pid ${pid}); agreeing to the hooks`);
  await agreeToHooks();
  for (const id of Object.keys(TABS)) { await bringUp(id); console.log(`  ${id} is up`); }
  await sleep(3000);

  console.log(`1. a Codex given a model that does not exist (${BAD_MODEL}) is asked`);
  const logFrom = hooksLog().length;
  let t0 = Date.now();
  const first = await call('ask_tab', 'codex-bad', 'A quick check.', 'Reply with only the word READY.', { timeout_ms: 300000 });
  console.log(`   answered in ${Math.round((Date.now() - t0) / 1000)}s: ${JSON.stringify({ state: first.state, error: first.error, note: first.note })}`);
  check(first.state === 'FAILED', 'the ask is answered FAILED, not left waiting', first);
  check(typeof first.error === 'string' && first.error.length > 0, 'what Codex said went wrong comes with it', first);
  check((await state('codex-bad')) === 'FAILED', 'the tab itself reads as failed', await state('codex-bad'));
  check(hooksLog().slice(logFrom).some((l) => /State tab\d+ \w+->FAILED \[Codex CLI\]/.test(l)), 'the log shows the tab turning FAILED');

  console.log('2. asked again: the words go in again');
  const logAgain = hooksLog().length;
  t0 = Date.now();
  const second = await call('ask_tab', 'codex-bad', 'Once more.', 'Reply with only the word READY.', { timeout_ms: 300000 });
  console.log(`   answered in ${Math.round((Date.now() - t0) / 1000)}s: ${JSON.stringify({ state: second.state, error: second.error })}`);
  check(hooksLog().slice(logAgain).some((l) => l.includes('ask_tab: sent to codex-bad')), 'the second ask was sent, not held as busy');
  check(second.state === 'FAILED', 'and is answered FAILED for its own turn', second);

  console.log('3. a Codex that answers is still answered whole');
  const word = 'PINEAPPLE' + Math.random().toString(36).slice(2, 6).toUpperCase();
  const good = await call('ask_tab', 'codex-good', 'A quick check.', `Reply with only the word ${word}.`, { timeout_ms: 300000 });
  console.log(`   ${JSON.stringify({ state: good.state, reply: good.reply, source: good.source })}`);
  check(good.state === 'DONE' && String(good.reply || '').includes(word), 'an answer is DONE with its words', good);

  console.log(`4. an AI asks with the shikisha command, lets go after 100 s, and the turn asked fails after that (${HOLD_MS / 1000}s)`);
  const logLetGo = hooksLog().length;
  await call('send_to_tab', 'codex-good',
    'Use the shikisha command to ask <@codex-slow> to reply with only the word READY. ' +
    'If the command says it is still working, end your turn. When you are told it has answered, ' +
    'read it with shikisha inbox and reply to me with what came back, as it is.');
  const fromLog = (re) => hooksLog().slice(logLetGo).some((l) => re.test(l));
  await until(() => fromLog(/ask_tab: codex-good asks codex-slow/), 'codex-good to ask', 4 * 60000);
  await until(() => fromLog(/ask_tab: codex-slow -> PENDING/), 'the command to let go', 4 * 60000);
  check(true, 'the asking AI let go of the line (PENDING)');
  await until(() => fromLog(/ask_tab: codex-slow's answer \(FAILED\) is in codex-good's inbox/), 'the failure in the inbox', 4 * 60000);
  check(true, 'the failure that came after it went into the asker\'s inbox');
  check((await state('codex-slow')) === 'FAILED', 'the tab asked reads as failed', await state('codex-slow'));
  // The asker is told, reads its inbox, and says what came back
  const told = await until(async () => /at capacity|ended in an error|NOT DONE/i.test(await screen('codex-good'))
    && ['DONE', 'WAIT', 'BACKGROUND'].includes(await state('codex-good')), 'the asker to say what came back', 5 * 60000).catch(() => false);
  check(told, 'the asker was told and read the failure', (await screen('codex-good')).split('\n').slice(-25).join('\n'));
} catch (e) {
  console.error(e.stack || e.message);
  checks.push(false);
} finally {
  const ok = checks.length > 0 && checks.every(Boolean);
  console.log(`\n${ok ? 'all passed' : 'FAILED'} (${checks.filter(Boolean).length}/${checks.length})`);
  modelServer.close();
  cleanup();
  process.exit(ok ? 0 : 1);
}
