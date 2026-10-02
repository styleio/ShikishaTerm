/**
 * An ask_tab follows the tab it was asked of, not its id: a tab closed while
 * the ask waits, and another opened under the same id, is not the one asked.
 *
 * This checkout's build in a folder of its own, one desk with a stand-in AI
 * tab `helper` (a `claude.cmd` running a script that writes down every byte
 * it is sent, and reads as busy -- "esc to interrupt" -- while a flag file is
 * there). Through the app's own pipe, as the person:
 *
 *   1. `helper` is busy and is asked something: the ask waits, nothing sent
 *   2. `helper` is closed, and a new tab opened under the id `helper`, not
 *      busy -- the moment a waiting ask would go to whatever is called that
 *   3. the ask ends saying the tab is gone; the new tab is sent nothing, and
 *      the first was sent nothing either
 *   4. a fresh ask to `helper` goes to the new tab: the id still reaches the
 *      tab that has it now
 *
 *     cargo build
 *     node tools/debug/ask-tab-reused-id.win.mjs
 *
 * Needs Windows and Node. No account is used and nothing leaves the machine.
 * Nothing of a copy somebody is using is read, written or stopped; the app is
 * stopped on the way out.
 */
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-askreuse');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const STUB = path.join(RUN, 'stub');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const BUSY = path.join(RUN, 'busy');
const HEARD_OLD = path.join(RUN, 'heard-old.txt');
const HEARD_NEW = path.join(RUN, 'heard-new.txt');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const die = (why) => { console.error(why); stopApp(); process.exit(2); };

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, STUB, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });

// The stand-in AI: a prompt, everything it is sent written down, and the
// line a busy Claude Code shows for as long as the flag file is there
fs.writeFileSync(path.join(STUB, 'listen.js'), [
  "const fs = require('fs');",
  'const [out, busy] = process.argv.slice(2);',
  "if (process.stdin.isTTY) process.stdin.setRawMode(true);",
  // Says it started, beside its command
  "fs.writeFileSync(require('path').join(__dirname, process.argv[4] || '', 'started'), '');",
  "process.stdout.write('stand-in ready\\r\\n> ');",
  "process.stdin.on('data', (d) => { fs.appendFileSync(out, d); });",
  "setInterval(() => { process.stdout.write(fs.existsSync(busy) ? '\\r* working (esc to interrupt) ' + Date.now() % 1000 + '   ' : '\\r> ' + ' '.repeat(40) + '\\r> '); }, 500);",
].join('\n'));
const standIn = (name, heard) => {
  const dir = path.join(STUB, name);
  fs.mkdirSync(dir, { recursive: true });
  const cmd = path.join(dir, 'claude.cmd');
  fs.writeFileSync(cmd, `@"${process.execPath}" "${path.join(STUB, 'listen.js')}" "${heard}" "${BUSY}" "${name}" %*\r\n`);
  return [cmd];
};

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  external_api: { access: 'user' },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  keep_terminals: false,
  desks: [{ name: 'Reuse', id: 'reuse', folders: [{ cwd: WORK, tabs: [
    { name: 'helper', id: 'helper', command: standIn('old', HEARD_OLD) },
    { name: 'shell', id: 'shell', command: 'cmd.exe' },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
const child = spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

// The app's own pipe, as the person: each call on a connection of its own,
// so an ask held open does not hold up the rest
const tokenFile = () => [path.join(APP, 'data', 'api-token'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data', 'api-token')].find((p) => fs.existsSync(p));
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
  const hello = await line({ token: fs.readFileSync(tokenFile(), 'utf8').trim() });
  if (!hello.ok) throw new Error('the door refused');
  const a = await line({ id: '1', method, params });
  sock.destroy();
  if (!a.ok) throw new Error(`${method}: ${a.error}`);
  return a.result;
};
const logLines = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/) : []);
const heard = (f) => (fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '');
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return; await sleep(400); }
  throw new Error('timed out waiting for ' + what + '\nlog:\n' + logLines().slice(-20).join('\n'));
};
const stateOf = async (id) => { try { return String(await call('state', id)); } catch (e) { return `(${e.message})`; } };

try {
  await until(() => tokenFile(), 'the api token', 60000);
  fs.writeFileSync(BUSY, '');
  await until(async () => /BUSY/i.test(await stateOf('helper')), 'helper to read as busy', 60000);

  console.log('1. helper is busy and is asked: the ask waits');
  const WORD = 'REUSE' + Math.random().toString(36).slice(2, 8).toUpperCase();
  let answered = null;
  const asked = call('ask_tab', 'helper', 'A word.', `Reply with only ${WORD}.`, 120)
    .then((v) => { answered = { ok: true, v }; }, (e) => { answered = { ok: false, e: String(e.message || e) }; });
  await until(() => logLines().some((l) => l.includes('ask_tab: outside asks helper')), 'the ask to be taken', 20000);
  await sleep(2000);
  check(!heard(HEARD_OLD).includes(WORD), 'nothing is sent to a busy tab');

  console.log('2. helper is taken out of the settings, and a new tab put in under its id, not busy');
  // As the settings page does it: the line taken out, and a new one under
  // the same id -- a new tab, so a uid of its own
  const cfg = JSON.parse(fs.readFileSync(CONFIG, 'utf8').replace(/^﻿/, ''));
  const tabs = cfg.desks[0].folders[0].tabs;
  const old = tabs.find((t) => t.id === 'helper');
  check(typeof old.uid === 'string' && old.uid.length === 36, 'the first helper has its uid written down');
  tabs.splice(tabs.indexOf(old), 1, { name: 'helper', id: 'helper', uid: randomUUID(), command: standIn('new', HEARD_NEW) });
  fs.rmSync(BUSY, { force: true });
  fs.writeFileSync(CONFIG, JSON.stringify(cfg, null, 2));
  await until(() => fs.existsSync(path.join(STUB, 'new', 'started')), 'the new helper to start', 60000);
  await until(async () => /WAIT|DONE|IDLE/i.test(await stateOf('helper')), 'the new helper to be up and free', 60000);

  console.log('3. the ask ends saying the tab is gone; the new tab is sent nothing');
  await until(() => answered !== null, 'the ask to end', 60000);
  console.log('   the ask ended: ' + JSON.stringify(answered).slice(0, 300));
  await sleep(3000);
  check(!heard(HEARD_NEW).includes(WORD), 'the new tab under the id was not sent what was asked of the closed one');
  check(!heard(HEARD_OLD).includes(WORD), 'the closed tab was not sent it either');
  check(!(answered.ok && answered.v && answered.v.state === 'DONE'), 'the new tab\'s screen is not handed back as the answer');
  check(!logLines().some((l) => l.includes('ask_tab: sent to helper')), 'the log says nothing was sent');

  console.log('4. a fresh ask to the id goes to the tab that has it now');
  const WORD2 = 'FRESH' + Math.random().toString(36).slice(2, 8).toUpperCase();
  call('ask_tab', 'helper', 'Another word.', `Reply with only ${WORD2}.`, 30).catch(() => {});
  await until(() => heard(HEARD_NEW).includes(WORD2), 'the new tab to be sent the fresh ask', 30000);
  check(true, 'the new tab is sent a fresh ask by its id');
  await asked;
} catch (e) {
  console.error(String(e.message || e));
  failures += 1;
} finally {
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
