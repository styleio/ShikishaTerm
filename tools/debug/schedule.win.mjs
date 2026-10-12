/**
 * Hooks named after a time are run by the app's clock: once when the time
 * comes, not again after a restart, and not while automation is stopped.
 *
 * This checkout's build in a folder of its own, with one terminal tab whose
 * automation folder holds `every_1m.lua` and an `at_HHMM.lua` set two
 * minutes ahead of this PC's clock, each writing a line to the log. Checked:
 *   - every_1m runs about a minute after it is first seen, and again a
 *     minute later -- once each time
 *   - the time of day runs once, when it comes
 *   - the times are kept, and the app started again does not run them twice
 *   - with automation stopped, nothing runs
 *
 *     cargo build
 *     node tools/debug/schedule.win.mjs
 *
 * Takes about five minutes. Needs Windows and Node. Nothing of a copy
 * somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-schedule');
const APP = path.join(RUN, 'app');
const AUTO = path.join(RUN, 'auto');
const PORT = 9478;
const TOKEN = 'schedule-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-schedule*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.mkdirSync(AUTO, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// Two minutes ahead, by this PC's clock
const at = new Date(Date.now() + 2 * 60 * 1000 + 30 * 1000);
const hhmm = String(at.getHours()).padStart(2, '0') + String(at.getMinutes()).padStart(2, '0');
fs.writeFileSync(path.join(AUTO, 'every_1m.lua'), 'shikisha.log("TICK every_1m for " .. tab.index)');
fs.writeFileSync(path.join(AUTO, `at_${hhmm}.lua`), 'shikisha.log("TICK at_' + hhmm + ' for " .. tab.index)');

const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en', agent_hooks: answered,
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Clock', id: 'clock', folders: [{ cwd: RUN, tabs: [
    { name: 'shell', id: 'shell', command: ['cmd.exe'], automation: AUTO },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
const start = () => spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
const log = () => { const f = path.join(APP, 'logs', 'hooks.log'); return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : ''; };
const count = (what) => (log().match(new RegExp(`TICK ${what} for 1`, 'g')) || []).length;
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test()) return true; await sleep(2000); }
  throw new Error('timed out waiting for ' + what);
};
let cookies = '';
const board = async () => { const r = await fetch(`${BASE}/?t=${TOKEN}`, { redirect: 'manual' }); cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; '); return /rs=/.test(cookies); };
const shellTab = async () => { const s = await (await fetch(`${BASE}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies } })).json(); return ((s.ui || s).tabs || []).find((t) => t.name === 'shell') || {}; };

try {
  start();
  const began = Date.now();
  await until(async () => log().includes('Sending state'), 'the app', 60000);
  await until(board, 'the board', 30000);
  const shown = await until(async () => /^\d\d:\d\d/.test((await shellTab()).timed || ''), 'the tab to say when', 40000).then(() => shellTab());
  check(true, `the tab says when its automation next runs (⏰ ${shown.timed})`);
  await until(async () => count('every_1m') >= 1, 'every_1m to run once', 120000);
  const first = (Date.now() - began) / 1000;
  check(first >= 55 && first <= 100, `every_1m first ran about a minute after it was first seen (${first.toFixed(0)} s)`);
  await until(async () => count('every_1m') >= 2, 'every_1m to run again', 100000);
  check(count('every_1m') === 2, `...and again a minute later, once (${count('every_1m')})`);
  await until(async () => count(`at_${hhmm}`) >= 1, `at_${hhmm} to run`, 200000);
  check(count(`at_${hhmm}`) === 1, `at_${hhmm} ran when ${hhmm.slice(0, 2)}:${hhmm.slice(2)} came, once`);
  const kept = JSON.parse(fs.readFileSync(path.join(APP, 'data', 'schedule.json'), 'utf8')).last;
  check(Object.keys(kept).some((k) => k.endsWith(`#at_${hhmm}#` + k.split('#')[2])) && Object.keys(kept).length === 2, `the times are kept (${Object.keys(kept).length})`);

  // Started again: the time of day is not run a second time
  stopApp();
  await sleep(1500);
  const before = count(`at_${hhmm}`);
  start();
  await until(async () => (log().match(/Sending state/g) || []).length >= 2, 'the app again', 60000);
  await sleep(35000);
  check(count(`at_${hhmm}`) === before, 'the app started again does not run the time of day twice');

  // Automation stopped: nothing runs
  await until(board, 'the board', 30000);
  await fetch(`${BASE}/api/auto?t=${TOKEN}`, { method: 'POST', headers: { Cookie: cookies }, body: JSON.stringify({ on: false }) });
  await sleep(2000);
  const held = count('every_1m');
  await sleep(80000);
  check(!(await shellTab()).timed, 'with automation stopped, the tab no longer says when');
  check(count('every_1m') === held, `with automation stopped, every_1m does not run (${held} -> ${count('every_1m')})`);
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (!process.env.KEEP) stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
