/**
 * This PC's resident process of the version before an update, replaced by
 * one of this version once it holds nothing -- through the running app.
 *
 * A real published version (kept by update-trial.win.mjs) is started in a
 * folder of its own with one terminal tab, which its resident process holds.
 * Then, the way it happens to a person:
 *
 *   1. the app is updated the way an update does it (the running file moved
 *      aside to .old, this checkout's build put in its place) and started
 *      again: the tab is back on the same program, under the old resident
 *      process, which runs on
 *   2. a second tab is opened while the old one still holds the first: it
 *      opens there too -- a resident process is never replaced under a
 *      terminal it holds
 *   3. both programs end: the old resident process holds nothing any more
 *   4. a third tab is opened: the old resident process is ended, one of this
 *      version is started, and the tab's program runs under it
 *
 *     cargo build
 *     node tools/debug/keeper-replace.win.mjs [--before=0.26.0]
 *
 * Needs Windows and Node, and the older version fetched once by
 * `node tools/debug/update-trial.win.mjs --back=<version>`. Isolated the way a
 * new-user run is: its own folder and LOCALAPPDATA. Nothing of any other copy
 * is touched.
 */
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { stateFolder } from './lib/copy.mjs';

const arg = (name, def) => (process.argv.find((a) => a.startsWith(`--${name}=`)) || '').split('=')[1] || def;
const BEFORE = arg('before', '0.26.0');
const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-keeper-replace');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'local');
const CACHE = path.join(os.tmpdir(), 'sk-update-trial-cache', BEFORE);
const CONFIG = path.join(APP, 'config', 'config.json');
const MARK = 'swap-' + Math.random().toString(36).slice(2, 8);
const held = ['SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe'];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const psOut = (cmd) => ps('-Command', cmd).stdout.trim();
const ours = () => psOut(
  `Get-CimInstance Win32_Process | Where-Object { ($_.CommandLine + '') -like '*sk-keeper-replace*' -or ($_.CommandLine + '') -like '*${MARK}*' } | ` +
  `ForEach-Object { '{0} {1} {2}' -f $_.ProcessId, $_.Name, ($_.CommandLine -replace '\\s+', ' ') }`,
).split(/\r?\n/).filter(Boolean);
const stopAll = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.CommandLine + '') -like '*sk-keeper-replace*' -or ($_.CommandLine + '') -like '*${MARK}*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const alive = (pid) => !!pid && psOut(`if (Get-Process -Id ${pid} -ErrorAction SilentlyContinue) { 'yes' } else { 'no' }`) === 'yes';
const appPid = () => {
  const line = ours().find((l) => / SHIKISHA-TERM\.exe /.test(l) && !/--keeper/.test(l) && !/--type=/.test(l) && !/ --cli /.test(l));
  return line ? Number(line.split(' ')[0]) : null;
};
const keepers = () => ours().filter((l) => /--keeper /.test(l)).map((l) => Number(l.split(' ')[0]));
const programsOf = (word) => ours().filter((l) => /powershell\.exe/i.test(l) && l.includes(MARK) && l.includes(word) && !l.includes('Get-CimInstance')).map((l) => Number(l.split(' ')[0]));
const exeOf = (pid) => psOut(`(Get-CimInstance Win32_Process -Filter "ProcessId=${pid}").ExecutablePath`);
const parentOf = (pid) => Number(psOut(`(Get-CimInstance Win32_Process -Filter "ProcessId=${pid}").ParentProcessId`));
const under = (pid, of) => { for (let p = pid, n = 0; p && n < 10; p = parentOf(p), n += 1) { if (p === of) return true; } return false; };
const logText = () => { try { return fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8'); } catch { return ''; } };
const die = (why) => { console.error(why + '\nprocesses:\n' + ours().join('\n') + '\nlog:\n' + logText().split('\n').slice(-30).join('\n')); stopAll(); process.exit(2); };
const until = async (test, what, ms = 60000) => {
  for (const end = Date.now() + ms; Date.now() < end; await sleep(400)) { try { if (await test()) return; } catch {} }
  die('timed out waiting for ' + what);
};

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (!fs.existsSync(path.join(CACHE, 'SHIKISHA-TERM.exe'))) {
  die(`no ${BEFORE} kept at ${CACHE} -- run tools/debug/update-trial.win.mjs --back=${BEFORE} once, which fetches it`);
}

stopAll();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
// The older version in the folder, as it was before the update; this
// version's files kept aside for the update to put in place
const NEW = path.join(RUN, 'new');
fs.mkdirSync(NEW, { recursive: true });
for (const name of held) {
  if (fs.existsSync(path.join(APP, name))) fs.copyFileSync(path.join(APP, name), path.join(NEW, name));
  if (fs.existsSync(path.join(CACHE, name))) fs.copyFileSync(path.join(CACHE, name), path.join(APP, name));
}
const program = (word) => ['powershell.exe', '-NoLogo', '-NoProfile', '-Command', `Write-Output ('PID=' + $PID + ' ${MARK} ${word}'); while ($true) { Start-Sleep -Seconds 1 }`];
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  resident: false,
  update: { auto: false },
  external_api: { access: 'user' },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [{ name: 'first', id: 'first', command: program('first-word') }] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
const start = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
/** The app's own door (its API pipe), as the person: the key, then one call */
const call = async (method, ...params) => {
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${appPid()}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); }
  });
  const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
  const hello = await line({ token: fs.readFileSync(path.join(stateFolder(APP, LOCAL), 'api-token'), 'utf8').trim() });
  if (!hello.ok) { sock.destroy(); throw new Error('the door refused'); }
  const a = await line({ id: '1', method, params });
  sock.destroy();
  if (!a.ok) throw new Error(`${method}: ${a.error}`);
  return a.result;
};
const openTab = (name, word) => call('open_tab', { name, folder: WORK, command: program(word) });

console.log(`0. ${BEFORE} holds a terminal in its resident process`);
start();
await until(() => programsOf('first-word').length === 1 && keepers().length === 1, `the tab's program under ${BEFORE}'s resident process`);
const first = programsOf('first-word')[0];
const old = keepers()[0];
check(under(first, old), `the program (${first}) runs under ${BEFORE}'s resident process (${old})`);

console.log('1. updated the way an update does it, and started again');
{
  const app = appPid();
  ps('-Command', `& taskkill.exe /PID ${app} /T /F 2>&1 | Out-Null`);
  await until(() => !alive(app), 'the older app to be gone', 15000);
  for (const name of held) {
    const at = path.join(APP, name);
    if (!fs.existsSync(at)) continue;
    fs.rmSync(at + '.old', { force: true });
    fs.renameSync(at, at + '.old');
  }
  for (const name of held) if (fs.existsSync(path.join(NEW, name))) fs.copyFileSync(path.join(NEW, name), path.join(APP, name));
  start();
  await until(() => appPid() !== null && fs.existsSync(path.join(stateFolder(APP, LOCAL), 'api-token')), 'the updated app');
  await until(async () => String(await call('tab_screen', 'first')).includes(`PID=${first}`), 'the first tab back on its program');
  check(alive(old) && keepers().length === 1, 'the old resident process runs on, and is the only one');
  check(programsOf('first-word').length === 1 && alive(first), 'the same program, and only it');
}

console.log('2. a second tab while the old one still holds the first');
{
  await openTab('second', 'second-word');
  await until(() => programsOf('second-word').length === 1, 'the second tab\'s program');
  const second = programsOf('second-word')[0];
  check(under(second, old), 'it opens on the old resident process, which holds a running terminal');
  check(alive(first), 'the first program is untouched');
  check(logText().includes('still holds terminals; the new one is opened on it'), 'the log says why it was kept');
}

console.log('3. both programs end');
{
  for (const pid of [...programsOf('first-word'), ...programsOf('second-word')]) ps('-Command', `Stop-Process -Id ${pid} -Force -ErrorAction SilentlyContinue`);
  const saved = () => { try { return JSON.parse(fs.readFileSync(path.join(stateFolder(APP, LOCAL), 'far-terminals'), 'utf8')).terms.length; } catch { return 0; } };
  await until(() => saved() === 0, 'the app to take both ends', 30000);
  check(alive(old), 'the old resident process is still there, holding nothing');
}

console.log('4. a third tab: the old resident process is replaced first');
{
  const opened = await openTab('third', 'third-word');
  await until(() => programsOf('third-word').length === 1, 'the third tab\'s program', 60000);
  const third = programsOf('third-word')[0];
  await until(() => !alive(old), 'the old resident process to go', 15000);
  check(true, 'the old resident process is gone');
  const now = keepers();
  check(now.length === 1 && now[0] !== old, `one resident process, a new one (${now.join(', ')})`);
  check(now.length === 1 && under(third, now[0]), 'the third tab\'s program runs under it');
  check(now.length === 1 && !/\.old$/i.test(exeOf(now[0])), `it is this version's file (${now.length === 1 ? exeOf(now[0]) : ''})`);
  check(/replaced by one of/.test(logText()), 'the log says it was replaced');
  await until(async () => String(await call('tab_screen', opened.id)).includes(`PID=${third}`), 'the third tab showing its program');
  check(true, 'the third tab shows its program');
}

stopAll();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
