/**
 * This PC's terminals outliving the app (the local-keeper setting), through
 * the running app itself.
 *
 * Starts this checkout's build in a folder of its own, with "keep this PC's
 * terminals" on and one terminal tab that prints its own process id and then
 * waits. Then, the way it happens to a person:
 *
 *   1. the tab's program runs under the resident process, not under the app
 *   2. the app's whole process tree is killed (a crash, or Task Manager's "End
 *      process tree"): the program is still running
 *   3. the app is started again: the tab is back on the same program -- its
 *      screen shows the id it printed before, and no second program was started
 *   4. the app's file is replaced the way an update replaces it (the running
 *      one moved aside to .old) and started again: still the same program
 *   5. quitting and answering "stop every AI": the program and the resident
 *      process end
 *
 *     cargo build
 *     node tools/debug/local-keeper.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder
 * and LOCALAPPDATA (so its resident process has a folder of its own too). The
 * window is closed through tools/debug/lib/quit-app.ps1, which presses the
 * quit question's button; nothing of any other copy is touched.
 */
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-local-keeper');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'local');
const MARK = 'kept-' + Math.random().toString(36).slice(2, 8);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); stopAll(); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const psOut = (cmd) => ps('-Command', cmd).stdout.trim();

/** Every process of this run: the app, its WebView2, the resident process, and the tab's program */
const ours = () => psOut(
  `Get-CimInstance Win32_Process | Where-Object { ($_.CommandLine + '') -like '*sk-local-keeper*' -or ($_.CommandLine + '') -like '*${MARK}*' } | ` +
  `ForEach-Object { '{0} {1} {2}' -f $_.ProcessId, $_.Name, ($_.CommandLine -replace '\\s+', ' ') }`,
).split(/\r?\n/).filter(Boolean);
const stopAll = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.CommandLine + '') -like '*sk-local-keeper*' -or ($_.CommandLine + '') -like '*${MARK}*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const alive = (pid) => psOut(`if (Get-Process -Id ${pid} -ErrorAction SilentlyContinue) { 'yes' } else { 'no' }`) === 'yes';
const appPid = () => {
  const line = ours().find((l) => / SHIKISHA-TERM\.exe /.test(l) && !/--keeper/.test(l) && !/--type=/.test(l));
  return line ? Number(line.split(' ')[0]) : null;
};
const keeperPid = () => {
  const line = ours().find((l) => /--keeper /.test(l));
  return line ? Number(line.split(' ')[0]) : null;
};
/** The program the tab runs: the one PowerShell carrying this run's mark */
const programs = () => ours().filter((l) => /powershell\.exe/i.test(l) && l.includes(MARK) && !l.includes('Get-CimInstance')).map((l) => Number(l.split(' ')[0]));

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopAll();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
// It prints its id, then asks the app something through its own `shikisha`
// every two seconds and says how that went: an AI's report to the app, made
// the same way. After a restart the answer must come from the app there now
const program = `Write-Output ('PID=' + $PID + ' ${MARK}'); $n = 0; while ($true) { $n++; $null = shikisha tab_list 2>&1; Write-Output ('CALL ' + $n + ' ' + $LASTEXITCODE); Start-Sleep -Seconds 2 }`;
/** The last of the program's calls on the screen, and whether it worked */
const lastCall = (screen) => { const all = [...screen.matchAll(/CALL (\d+) (-?\d+)/g)]; const m = all[all.length - 1]; return m ? { n: Number(m[1]), code: Number(m[2]) } : null; };
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en',
  keep_terminals: true,
  resident: false,
  external_api: { access: 'user' },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [{ name: 'held', id: 'held', command: ['powershell.exe', '-NoLogo', '-NoProfile', '-Command', program] }] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
const start = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
const until = async (test, what, ms = 40000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(300); }
  die('timed out waiting for ' + what + '\nthe tab\'s screen: ' + JSON.stringify(lastScreen) + '\nprocesses:\n' + ours().join('\n') + '\nlog:\n' + logTail());
};
const logTail = () => { try { return fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8').split('\n').slice(-25).join('\n'); } catch { return ''; } };
/** The tab's screen as the app last wrote it down: the same text the board and a phone are sent */
let lastScreen = '';
const screenSays = () => call('tab_screen', 'held').then((s) => { lastScreen = String(s || ''); return lastScreen; }).catch((e) => { lastScreen = `(${e.message})`; return ''; });
/** The app's own door (its API pipe), as the person: the key, then one call */
const tokenFile = () => [path.join(APP, 'data', 'api-token'), path.join(LOCAL, 'ShikishaTerm', 'data', 'api-token')].find((p) => fs.existsSync(p));
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
  const hello = await line({ token: fs.readFileSync(tokenFile(), 'utf8').trim() });
  if (!hello.ok) { sock.destroy(); throw new Error('the door refused'); }
  const a = await line({ id: '1', method, params });
  sock.destroy();
  if (!a.ok) throw new Error(`${method}: ${a.error}`);
  return a.result;
};
/** The process that started `pid` */
const parentOf = (pid) => Number(psOut(`(Get-CimInstance Win32_Process -Filter "ProcessId=${pid}").ParentProcessId`));
/** Whether `pid` is `of` or descends from it */
const under = (pid, of) => { for (let p = pid, n = 0; p && n < 10; p = parentOf(p), n += 1) { if (p === of) return true; } return false; };

start();
await until(() => programs().length === 1, 'the tab\'s program to start');
const first = programs()[0];
console.log('1. the program runs under the resident process');
await until(() => keeperPid() !== null, 'the resident process');
const chain = []; for (let p = first, n = 0; p && n < 6; p = parentOf(p), n += 1) chain.push(p);
console.log('   its line of parents: ' + chain.join(' <- '));
check(chain.includes(keeperPid()), `the resident process (${keeperPid()}) is in its line of parents`);
check(!under(first, appPid()), `it does not descend from the app (${appPid()})`);
await until(async () => (await screenSays()).includes(`PID=${first}`), 'the program\'s id on the tab\'s screen');
check(true, `the tab shows PID=${first}`);

console.log('2. the app\'s whole process tree is killed');
const app1 = appPid();
ps('-Command', `& taskkill.exe /PID ${app1} /T /F 2>&1 | Out-Null`);
await until(() => !alive(app1), 'the app to be gone', 15000);
await sleep(1500);
check(alive(first), 'the program is still running');
check(keeperPid() !== null, 'the resident process is still running');

console.log('3. the app is started again');
start();
await until(() => appPid() !== null, 'the app again');
await until(async () => (await screenSays()).includes(`PID=${first}`), 'the same program\'s id back on the tab\'s screen');
check(true, 'the tab\'s screen is back, with the id printed before');
check(programs().length === 1 && programs()[0] === first, `one program, the same one (${programs().join(', ')})`);

console.log('3b. the app lost its note of the terminal (killed again, the note deleted), and is started again');
{
  const app = appPid();
  ps('-Command', `& taskkill.exe /PID ${app} /T /F 2>&1 | Out-Null`);
  await until(() => !alive(app), 'the app to be gone', 15000);
  for (const f of [path.join(APP, 'data', 'far-terminals'), path.join(LOCAL, 'ShikishaTerm', 'data', 'far-terminals')]) fs.rmSync(f, { force: true });
  start();
  await until(() => appPid() !== null, 'the app again');
  await until(async () => (await screenSays()).includes(`PID=${first}`), 'the same program, found without the note');
  await sleep(3000);
  check(programs().length === 1 && programs()[0] === first, `still one program, the same one (${programs().join(', ')})`);
}

console.log('4. the app is replaced the way an update replaces it, and started again');
const app2 = appPid();
ps('-Command', `& taskkill.exe /PID ${app2} /F 2>&1 | Out-Null`);
await until(() => !alive(app2), 'the app to be gone', 15000);
const live = path.join(APP, 'SHIKISHA-TERM.exe');
fs.rmSync(live + '.old', { force: true });
fs.renameSync(live, live + '.old');
fs.copyFileSync(exe, live);
start();
await until(() => appPid() !== null, 'the replaced app');
await until(async () => (await screenSays()).includes(`PID=${first}`), 'the program after the update');
check(programs().length === 1 && programs()[0] === first, 'still the same program after the update');
console.log('   its `shikisha` reaches the app that is running now');
const before = lastCall(lastScreen)?.n ?? 0;
await until(async () => { const c = lastCall(await screenSays()); return c && c.n > before + 1 && c.code === 0; }, 'a call of the program answered by the app after the update', 40000);
check(true, `the program\'s call ${lastCall(lastScreen).n} was answered (exit code 0)`);

console.log('5. quit, answering "stop every AI"');
const keeper = keeperPid();
const quit = ps('-File', path.join(ROOT, 'tools', 'debug', 'lib', 'quit-app.ps1'), '-Root', APP, '-Answer', 'No');
console.log('   ' + quit.stdout.trim().split(/\r?\n/).join('\n   '));
await until(() => !alive(first), 'the program to end', 20000);
check(true, 'the program ended');
await until(() => keeper === null || !alive(keeper), 'the resident process to end', 20000);
check(true, 'the resident process ended');

stopAll();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
