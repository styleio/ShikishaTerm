/**
 * After a run that did not close properly, the AI tabs come back one at a
 * time; after two in a row that stopped soon after starting, they wait for
 * the person.
 *
 * This checkout's build in a folder of its own, with three tabs the app
 * takes for AI tabs (Claude Code's profile, running only a shell, so no AI
 * is started and nothing is spent) and its board on this machine only.
 * The mark of a run that died is left where a crash leaves it, and the app
 * is started:
 *   --after=one (the default): the first AI tab starts at once and the
 *     others are held, each starting about five seconds after the one before
 *   --after=two: every AI tab waits, still waits a while later, and starts
 *     when "Start now" is pressed for it
 *
 *     cargo build
 *     node tools/debug/paced-start.win.mjs [--after=one|two]
 *
 * Needs Windows and Node. Nothing of a copy somebody is using is read,
 * written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const AFTER = (process.argv.find((a) => a.startsWith('--after=')) || '--after=one').slice(8);
const RUN = path.join(os.tmpdir(), 'sk-paced-start');
const APP = path.join(RUN, 'app');
const PORT = 9470;
const TOKEN = 'paced-start-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-paced-start*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

if (!['one', 'two'].includes(AFTER)) die('--after is one or two');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.mkdirSync(path.join(APP, 'data'), { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
const ai = (id) => ({ name: id, id, command: ['cmd.exe', '/k', 'echo', id], profile: 'Claude Code' });
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en', agent_hooks: answered,
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Paced', id: 'paced', folders: [{ cwd: RUN, tabs: [ai('ai-a'), ai('ai-b'), ai('ai-c')] }] }],
}, null, 2));
// What a run that died leaves: its mark, of a process long gone, and for
// the second case one short run already counted before it
fs.writeFileSync(path.join(APP, 'data', 'running.json'), JSON.stringify({ pid: 4000000, version: '0.27.0', started: 1, settled: false }));
fs.writeFileSync(path.join(APP, 'data', 'short-runs'), AFTER === 'two' ? '1' : '0');

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
const started = Date.now();

let cookies = '';
const board = async () => {
  const r = await fetch(`${BASE}/?t=${TOKEN}`, { redirect: 'manual', signal: AbortSignal.timeout(10000) });
  cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
};
const state = async () => (await fetch(`${BASE}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) })).json();
const intent = (o) => fetch(`${BASE}/api/intent?t=${TOKEN}`, { method: 'POST', headers: { Cookie: cookies }, body: JSON.stringify(o), signal: AbortSignal.timeout(10000) });
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};
const tabs = async () => {
  const s = await state();
  const list = (s.ui || s).tabs || [];
  return Object.fromEntries(['ai-a', 'ai-b', 'ai-c'].map((n) => [n, list.find((t) => t.name === n)]));
};
const waiting = (t) => t && t.hold && t.hold.kind === 'wait';
const secs = () => ((Date.now() - started) / 1000).toFixed(1);

try {
  await until(async () => { await board(); return /(^|; )rs=/.test(cookies); }, 'the board', 60000);
  const first = await until(async () => { const t = await tabs(); return t['ai-a'] && t; }, 'the tabs', 30000);
  const log = fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8');
  if (AFTER === 'one') {
    check(/AI tabs come back OneByOne/.test(log), 'the start says the AI tabs come back one at a time');
    check(!waiting(first['ai-a']), `the first AI tab starts at once (${secs()} s)`);
    check(waiting(first['ai-b']) && waiting(first['ai-c']), 'the others wait their turn');
    check(/one at a time/.test(first['ai-b']?.hold?.say || ''), `the waiting tab says why: "${first['ai-b']?.hold?.say}"`);
    await until(async () => !waiting((await tabs())['ai-b']), 'the second to start', 15000);
    const second = Number(secs());
    const t = await tabs();
    check(waiting(t['ai-c']), `the second started (${second} s) while the third still waits`);
    await until(async () => !waiting((await tabs())['ai-c']), 'the third to start', 15000);
    const third = Number(secs());
    check(third - second >= 3.5, `the third started a gap after the second (${second} s, ${third} s)`);
  } else {
    check(/AI tabs come back Asked/.test(log), 'the start says the AI tabs wait for the person');
    check(['ai-a', 'ai-b', 'ai-c'].every((n) => waiting(first[n])), 'every AI tab waits');
    await sleep(12000);
    const later = await tabs();
    check(['ai-a', 'ai-b', 'ai-c'].every((n) => waiting(later[n])), 'twelve seconds later they still wait');
    await intent({ kind: 'select', tab: later['ai-b'].index });
    await sleep(500);
    await intent({ kind: 'restart' });
    await until(async () => !waiting((await tabs())['ai-b']), 'the pressed one to start', 15000);
    const after = await tabs();
    check(!waiting(after['ai-b']), '"Start now" starts the tab it was pressed for');
    check(waiting(after['ai-a']) && waiting(after['ai-c']), 'the others go on waiting');
  }
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
