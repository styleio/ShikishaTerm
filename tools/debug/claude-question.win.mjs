/**
 * Claude Code's own permission prompt, from a real Claude Code: the tab reads
 * as waiting for a person, and the screen it stopped on is written out for the
 * detector's test of what a question is (detect.rs) to be checked against.
 *
 *     cargo build
 *     node tools/debug/claude-question.win.mjs [--keep]
 *
 * Isolated the way confer-real.win.mjs is: a home of its own with the
 * sign-in copied in, deleted at the end. Spends one or two real turns.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const KEEP = process.argv.includes('--keep');
const RUN = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-claude-q-'));
const INSTANCE = path.join(RUN, 'copy');
const APP = path.join(INSTANCE, 'app');
const HOME = path.join(RUN, 'home');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(RUN, 'config.json');
const OUT = path.join(ROOT, 'target', 'claude-question.txt');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8', windowsHide: true });
const launcher = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');
const cleanup = () => {
  ps('-File', launcher, '-At', INSTANCE, '-Stop');
  if (!KEEP) fs.rmSync(RUN, { recursive: true, force: true });
};
process.on('exit', cleanup);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug');
const REAL = os.homedir();
const SIGN_INS = [
  [path.join(REAL, '.claude', '.credentials.json'), path.join(HOME, '.claude', '.credentials.json')],
  [path.join(REAL, '.claude.json'), path.join(HOME, '.claude.json')],
];
for (const [from] of SIGN_INS) if (!fs.existsSync(from)) die(`not signed in: ${from} is missing`);
for (const d of [APP, path.join(HOME, '.claude'), WORK]) fs.mkdirSync(d, { recursive: true });
for (const [from, to] of SIGN_INS) fs.copyFileSync(from, to);
spawnSync('git', ['init', '-q'], { cwd: WORK });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en', keep_terminals: false, external_api: { access: 'user' },
  desks: [{ name: 'Q', id: 'q', folders: [{ cwd: WORK, tabs: [{ name: 'claude-ask', id: 'claude-ask', command: ['claude', '--permission-mode', 'default'] }] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
Object.assign(env, { USERPROFILE: HOME, HOME, DISABLE_AUTOUPDATER: '1', TERM: 'xterm-256color' });
const started = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', launcher,
  '-At', INSTANCE, '-Work', WORK, '-Exe', exe, '-Config', CONFIG, '-Cdp', '0'], { env, encoding: 'utf8', windowsHide: true });
if (started.status !== 0) die('launch failed: ' + started.stdout + started.stderr);
const pid = Number(/^pid=(\d+)/m.exec(started.stdout)?.[1]);
const dataDir = () => [path.join(APP, 'data'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data')].find((d) => fs.existsSync(path.join(d, 'api-token')));
const call = async (method, ...params) => {
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => { buf += d.toString('utf8'); let i; while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); } });
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
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(700); }
  throw new Error('timed out waiting for ' + what);
};
const screen = () => call('tab_screen', 'claude-ask').then((s) => String(s || ''));

let ok = false;
try {
  await until(() => dataDir(), 'the api token', 60000);
  // Up to its prompt: the folder trusted, nothing else answered blindly
  const end = Date.now() + 5 * 60000;
  while (Date.now() < end) {
    const s = await screen();
    // The cursor starts on "No, exit": one down to "Yes, I trust this folder"
    if (/trust this folder|Do you trust/i.test(s)) { await call('send', 'claude-ask', '\x1b[B'); await sleep(400); await call('send', 'claude-ask', '\r'); await sleep(2500); continue; }
    if (/for shortcuts|\? for/i.test(s)) break;
    await sleep(1000);
  }
  await call('send_to_tab', 'claude-ask', 'Run the shell command `echo hi > hello.txt` with your Bash tool, then say done.');
  let s = '';
  await until(async () => /Do you want to/.test(s = await screen()), 'its permission prompt', 4 * 60000)
    .catch(async (e) => { console.log((await screen()).split('\n').filter((l) => l.trim()).slice(-20).join('\n')); throw e; });
  fs.writeFileSync(OUT, s);
  const state = await call('state', 'claude-ask');
  console.log('--- the screen it stopped on (written to ' + OUT + ') ---\n' + s.split('\n').filter((l) => l.trim()).slice(-14).join('\n') + '\n---');
  console.log(`state: ${state}`);
  ok = String(state) === 'QUESTION';
  await call('send', 'claude-ask', '\x1b');
} catch (e) {
  console.error(e.stack || e.message);
}
console.log(ok ? 'PASS the tab reads as waiting for a person' : 'FAIL');
process.exit(ok ? 0 : 1);
