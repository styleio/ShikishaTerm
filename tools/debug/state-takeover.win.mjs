/**
 * The first start of a copy whose records are still in the `data` folder
 * beside it takes them across to this PC's own state folder -- copied, the old
 * folder left for another PC that shares it -- and a second start leaves what
 * this PC wrote since alone.
 *
 *     node tools/debug/state-takeover.win.mjs
 *
 * Needs Windows, Node and a debug build. Isolated: its own folder, its own
 * LOCALAPPDATA, started from its own folder; nothing of a copy somebody is
 * using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { stateFolder } from './lib/copy.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-state-takeover');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const STATE = stateFolder(APP, LOCAL);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8' });
const stop = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
let failed = 0;
const check = (ok, what) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`); if (!ok) failed++; };
const until = async (test, what, ms = 40000) => {
  for (const end = Date.now() + ms; Date.now() < end; await sleep(250)) if (test()) return;
  throw new Error(`timed out waiting for ${what}`);
};
const read = (p) => { try { return fs.readFileSync(p, 'utf8'); } catch { return null; } };

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) throw new Error(`no build at ${exe}`);
stop();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) throw new Error('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en',
  external_api: { access: 'user' },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: RUN, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));
// What an older version left beside the program: records, a folder of them,
// what stays with the program, and the marks of a run that are not carried
const DATA = path.join(APP, 'data');
fs.mkdirSync(path.join(DATA, 'ai-accounts', 'one'), { recursive: true });
fs.writeFileSync(path.join(DATA, 'remote-token'), 'carried-token-0123456789abcdef');
fs.writeFileSync(path.join(DATA, 'known-hosts.json'), '{"example:22":"ssh-ed25519 AAAA"}');
fs.writeFileSync(path.join(DATA, 'ai-accounts', 'one', 'marker'), 'one');
fs.writeFileSync(path.join(DATA, 'running.json'), JSON.stringify({ pid: 4000000, version: '0.1.0', started: 1, settled: true }));

const start = () => {
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
};

try {
  console.log('1. the first start takes the records across');
  start();
  await until(() => fs.existsSync(path.join(STATE, 'api-token')), 'the key in the state folder');
  check(read(path.join(STATE, 'program-folder.txt'))?.trim().toLowerCase() === APP.toLowerCase(), 'the state folder says whose it is');
  check(read(path.join(STATE, 'remote-token')) === 'carried-token-0123456789abcdef', 'the phone key came across');
  check(read(path.join(STATE, 'known-hosts.json'))?.includes('example:22'), 'the servers met came across');
  check(read(path.join(STATE, 'ai-accounts', 'one', 'marker')) === 'one', 'a folder of records came across whole');
  check(!read(path.join(STATE, 'running.json'))?.includes('4000000'), 'the old run\'s mark was not carried');
  check(read(path.join(DATA, 'remote-token')) === 'carried-token-0123456789abcdef', 'the old folder is left for another PC');
  check(!fs.existsSync(path.join(DATA, 'api-token')), 'this run wrote its key into the state folder only');
  const log = read(path.join(APP, 'logs', 'hooks.log')) || '';
  check(/State: kept in .* from now on/.test(log), 'the log says where the records are kept');

  console.log('2. a second start leaves what this PC wrote since alone');
  stop();
  await sleep(1500);
  fs.writeFileSync(path.join(STATE, 'remote-token'), 'written-on-this-pc-0123456789');
  fs.rmSync(path.join(STATE, 'api-token'), { force: true });
  start();
  await until(() => fs.existsSync(path.join(STATE, 'api-token')), 'the key again');
  check(read(path.join(STATE, 'remote-token')) === 'written-on-this-pc-0123456789', 'nothing was copied over it again');
  check(fs.readdirSync(path.dirname(STATE)).length === 1, 'one state folder for the copy, nothing half-made beside it');
} finally {
  stop();
}
console.log(failed ? `${failed} FAILED` : 'all passed');
process.exit(failed ? 1 : 0);
