/**
 * This PC's terminals outliving the app (the local-keeper setting), through
 * the running app itself.
 *
 * Starts this checkout's build in a folder of its own with the setting left
 * unset -- it is on unless turned off -- and two terminal tabs: one that
 * prints its own process id and then waits, and one that, after a quiet
 * while, leaves a small server listening on a port of its own behind its
 * prompt. Then, the way it happens to a person:
 *
 *   1. the tabs' programs run under the resident process, not under the app,
 *      with nothing set (the default is on)
 *   1b. the board says once that terminals now keep running (the card), and
 *      the server's tab reads as at work behind its prompt, with its port
 *   2. the app's whole process tree is killed (a crash, or Task Manager's "End
 *      process tree"): the programs are still running
 *   3. the app is started again: the tabs are back on the same programs -- the
 *      screen shows the id printed before, no second program was started --
 *      and the server's tab still reads as at work, with its port
 *   3b. the app lost its note of the terminals (killed again, the note
 *      deleted), and is started again: still the same programs
 *   3c. the card is put away with its button: it is gone, and stays gone
 *   4. the app's file is replaced the way an update replaces it (the running
 *      one moved aside to .old) and started again: still the same program, and
 *      its `shikisha` reaches the app running now
 *   5. quitting and answering "Yes" (leave them running): the app goes, the
 *      programs stay, and the next start comes back to them
 *   6. the setting turned off: a new tab runs inside the app; the app killed
 *      and started again comes back to the kept programs, starting no second
 *      one; the settings say how many are still running, and their "Stop
 *      them" ends the programs and the resident process
 *   8. (run before 7) the resident process frozen so it cannot answer:
 *      quitting and answering "No" does not quit -- the question comes back
 *      saying the terminals could not be stopped; "Cancel" keeps the app,
 *      and quitting again and answering "Yes" leaves the program running
 *   7. the setting back to unset, a fresh start holds the tab again; quitting
 *      and answering "No" (stop everything): the program and the resident
 *      process end
 *
 *     cargo build
 *     node tools/debug/local-keeper.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder
 * and LOCALAPPDATA (so its resident process has a folder of its own too). The
 * board and the settings are read over the copy's own DevTools ports. The
 * window is closed through tools/debug/lib/quit-app.ps1, which presses the
 * quit question's button. Pictures land in target/shots. Nothing of any other
 * copy is touched.
 */
import {connectCdp} from './chrome.mjs';
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
const SHOTS = path.join(ROOT, 'target', 'shots');
const CONFIG = path.join(APP, 'config', 'config.json');
const MARK = 'kept-' + Math.random().toString(36).slice(2, 8);
const L = JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', 'en.json'), 'utf8'));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); stopAll(); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const psOut = (cmd) => ps('-Command', cmd).stdout.trim();

/** Every process of this run: the app, its WebView2, the resident process, and the tabs' programs */
const ours = () => psOut(
  `Get-CimInstance Win32_Process | Where-Object { ($_.CommandLine + '') -like '*sk-local-keeper*' -or ($_.CommandLine + '') -like '*${MARK}*' } | ` +
  `ForEach-Object { '{0} {1} {2}' -f $_.ProcessId, $_.Name, ($_.CommandLine -replace '\\s+', ' ') }`,
).split(/\r?\n/).filter(Boolean);
const stopAll = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.CommandLine + '') -like '*sk-local-keeper*' -or ($_.CommandLine + '') -like '*${MARK}*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const alive = (pid) => psOut(`if (Get-Process -Id ${pid} -ErrorAction SilentlyContinue) { 'yes' } else { 'no' }`) === 'yes';
// Not a tab's `shikisha` call (--cli), which is the same program
const appPid = () => {
  const line = ours().find((l) => / SHIKISHA-TERM\.exe /.test(l) && !/--keeper/.test(l) && !/--type=/.test(l) && !/ --cli /.test(l));
  return line ? Number(line.split(' ')[0]) : null;
};
const keeperPid = () => {
  const line = ours().find((l) => /--keeper /.test(l));
  return line ? Number(line.split(' ')[0]) : null;
};
/** The programs the tabs run, by tab: PowerShell carrying this run's mark and the tab's word */
const programsOf = (word) => ours().filter((l) => /powershell\.exe/i.test(l) && l.includes(MARK) && l.includes(word) && !l.includes('Get-CimInstance')).map((l) => Number(l.split(' ')[0]));
const programs = () => programsOf('held-word');
/** The server the second tab leaves running: node, on this run's script */
const servers = () => ours().filter((l) => /node\.exe/i.test(l) && l.includes('listen-' + MARK)).map((l) => Number(l.split(' ')[0]));

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});

console.log('starting this checkout\'s build, isolated');
stopAll();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
// It prints its id, then asks the app something through its own `shikisha`
// every two seconds and says how that went: an AI's report to the app, made
// the same way. After a restart the answer must come from the app there now
const program = `Write-Output ('PID=' + $PID + ' ${MARK} held-word'); $n = 0; while ($true) { $n++; $null = shikisha tab_list 2>&1; Write-Output ('CALL ' + $n + ' ' + $LASTEXITCODE); Start-Sleep -Seconds 2 }`;
// The second tab: quiet long enough for the app to learn what its program
// is on its own, then a server left running behind it, then quiet again --
// work behind the prompt, the way a dev server started by an AI is
const PORT = await freePort();
const LISTEN = path.join(WORK, `listen-${MARK}.js`);
fs.writeFileSync(LISTEN, `require('net').createServer(() => {}).listen(${PORT}, '127.0.0.1');\n`);
const serving = `Write-Output ('BG=' + $PID + ' ${MARK} bg-word'); Start-Sleep -Seconds 20; Start-Process -FilePath node -ArgumentList '${LISTEN.replace(/'/g, "''")}' -NoNewWindow; Write-Output 'SERVING'; while ($true) { Start-Sleep -Seconds 60 }`;
/** The last of the program's calls on the screen, and whether it worked */
const lastCall = (screen) => { const all = [...screen.matchAll(/CALL (\d+) (-?\d+)/g)]; const m = all[all.length - 1]; return m ? { n: Number(m[1]), code: Number(m[2]) } : null; };
const baseConfig = {
  language: 'en',
  resident: false,
  external_api: { access: 'user' },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [
    { name: 'held', id: 'held', command: ['powershell.exe', '-NoLogo', '-NoProfile', '-Command', program] },
    { name: 'bg', id: 'bg', command: ['powershell.exe', '-NoLogo', '-NoProfile', '-Command', serving] },
  ] }] }],
};
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify(baseConfig, null, 2));
/** The settings as the app has them now, changed by `change` and written back */
const editConfig = (change) => {
  const c = JSON.parse(fs.readFileSync(CONFIG, 'utf8').replace(/^﻿/, ''));
  change(c);
  fs.writeFileSync(CONFIG, JSON.stringify(c, null, 2));
};

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const portFile = (envName) => path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
const start = () => {
  // A port written by the copy before is not this one's
  for (const e of ['shell', path.join('profiles', 'default')]) fs.rmSync(portFile(e), { force: true });
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
};
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
const stateFile = (name) => [path.join(APP, 'data', name), path.join(LOCAL, 'ShikishaTerm', 'data', name)].find((p) => fs.existsSync(p));
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
  const hello = await line({ token: fs.readFileSync(stateFile('api-token'), 'utf8').trim() });
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

// ── A page over the copy's own DevTools port ──────────────────────────────
const portOf = (envName) => {
  const f = portFile(envName);
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
async function connect(target, label) {
  const {ws, send, run} = await connectCdp(target);
  const shot = async (name) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `local-keeper-${label}-${name}.png`), Buffer.from(r.data, 'base64'));
  };
  // A real press where an element is: the mouse, not a click() in the page
  const press = async (selectorJs) => {
    const b = await run(`(() => { const e = ${selectorJs}; if (!e) return null; e.scrollIntoView({block: "nearest"}); const r = e.getBoundingClientRect(); return {x: r.left + r.width / 2, y: r.top + r.height / 2}; })()`);
    if (!b) throw new Error('nothing to press: ' + selectorJs);
    for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
      await send('Input.dispatchMouseEvent', { type, x: b.x, y: b.y, button: 'left', clickCount: 1 });
    }
  };
  return { close: () => ws.close(), run, shot, press };
}
/** The board of the copy running now */
async function board() {
  let t;
  await until(async () => { const p = portOf('shell'); return p && (t = (await targetsOf(p)).find((x) => x.type === 'page')); }, 'the board\'s DevTools port', 60000);
  const b = await connect(t, 'board');
  await until(() => b.run('typeof S !== "undefined" && !!S && !!S.tabs && S.tabs.length >= 2'), 'the board and its tabs', 40000);
  return b;
}
/** What the board says of a tab: its state and the ports under it */
const tabSays = (b, id) => b.run(`(() => { const t = S.tabs.find(t => t.id === ${JSON.stringify(id)}); return t ? { state: t.state, ports: (t.place || {}).ports || [] } : null; })()`);
const workingBehind = async (b) => { const t = await tabSays(b, 'bg'); return !!t && t.state === 'BACKGROUND' && t.ports.includes(PORT); };

start();
await until(() => programs().length === 1 && programsOf('bg-word').length === 1, 'the tabs\' programs to start');
const first = programs()[0];
const bgFirst = programsOf('bg-word')[0];
console.log('1. with nothing set, the programs run under the resident process');
await until(() => keeperPid() !== null, 'the resident process');
const chain = []; for (let p = first, n = 0; p && n < 6; p = parentOf(p), n += 1) chain.push(p);
console.log('   its line of parents: ' + chain.join(' <- '));
check(!('keep_terminals' in JSON.parse(fs.readFileSync(CONFIG, 'utf8'))), 'the settings say nothing about it');
check(chain.includes(keeperPid()), `the resident process (${keeperPid()}) is in its line of parents`);
check(!under(first, appPid()), `it does not descend from the app (${appPid()})`);
check(under(bgFirst, keeperPid()), 'the second tab\'s program is the resident process\'s too');
await until(async () => (await screenSays()).includes(`PID=${first}`), 'the program\'s id on the tab\'s screen');
check(true, `the tab shows PID=${first}`);

console.log('1b. the board: the card, and the server\'s tab at work behind its prompt');
let b = await board();
await until(() => b.run('!!S.keep_notice && !!document.querySelector(".keepnote")'), 'the card that says terminals keep running', 30000);
check(await b.run(`document.querySelector(".keepnote .tt").textContent === ${JSON.stringify(L['tui.keep.title'])}`), 'the card says what changed');
await until(() => servers().length === 1, 'the server to start', 40000);
await until(() => workingBehind(b), 'the server\'s tab at work behind its prompt, with its port', 40000);
check(true, `the server's tab is BACKGROUND with :${PORT} under it`);
await b.shot('1-card-and-background');
b.close();

console.log('2. the app\'s whole process tree is killed');
const app1 = appPid();
ps('-Command', `& taskkill.exe /PID ${app1} /T /F 2>&1 | Out-Null`);
await until(() => !alive(app1), 'the app to be gone', 15000);
await sleep(1500);
check(alive(first), 'the program is still running');
check(servers().length === 1, 'the server is still running');
check(keeperPid() !== null, 'the resident process is still running');

console.log('3. the app is started again');
start();
await until(() => appPid() !== null, 'the app again');
await until(async () => (await screenSays()).includes(`PID=${first}`), 'the same program\'s id back on the tab\'s screen');
check(true, 'the tab\'s screen is back, with the id printed before');
check(programs().length === 1 && programs()[0] === first, `one program, the same one (${programs().join(', ')})`);
b = await board();
await until(() => workingBehind(b), 'the server\'s tab still at work behind its prompt after the restart', 40000);
check(true, `after the restart the server's tab is still BACKGROUND with :${PORT}`);
check(servers().length === 1, 'still one server');
check(await b.run('!!S.keep_notice'), 'the card is still up: it was not put away yet');
await b.shot('3-after-restart');
b.close();

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

console.log('3c. the card is put away with its button');
b = await board();
await until(() => b.run('!!document.querySelector(".keepnote button.go")'), 'the card\'s button');
await b.press('document.querySelector(".keepnote button.go")');
await until(() => b.run('!S.keep_notice && !document.querySelector(".keepnote")'), 'the card to go');
check(true, 'the card is gone');
await until(() => !!stateFile('keep-told'), 'it to be written down as told', 10000);
check(true, 'it is written down as told');
b.close();

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
b = await board();
await sleep(6000);
check(!(await b.run('!!S.keep_notice')), 'the card stays put away after a start');
b.close();

console.log('5. quit, answering "Yes": the programs go on, and the next start comes back to them');
{
  const app = appPid();
  const quit = ps('-File', path.join(ROOT, 'tools', 'debug', 'lib', 'quit-app.ps1'), '-Root', APP, '-Answer', 'Yes');
  console.log('   ' + quit.stdout.trim().split(/\r?\n/).join('\n   '));
  check(quit.stdout.includes('2'), 'the question says how many of this PC\'s terminals go on');
  await until(() => !alive(app), 'the app to quit', 20000);
  await sleep(1500);
  check(alive(first) && servers().length === 1, 'the programs are still running');
  start();
  await until(() => appPid() !== null, 'the app again');
  await until(async () => (await screenSays()).includes(`PID=${first}`), 'the same program after quitting');
  check(programs().length === 1 && programs()[0] === first, 'the same program, and only it');
}

console.log('6. the setting turned off');
editConfig((c) => { c.keep_terminals = false; });
await sleep(2500);
{
  // A tab opened now runs inside the app
  const at = await call('open_tab', { name: 'fresh', folder: WORK, command: ['powershell.exe', '-NoLogo', '-NoProfile', '-Command', `Write-Output '${MARK} fresh-word'; while ($true) { Start-Sleep -Seconds 60 }`] });
  console.log('   opened: ' + JSON.stringify(at));
  await until(() => programsOf('fresh-word').length === 1, 'the new tab\'s program');
  const fresh = programsOf('fresh-word')[0];
  check(under(fresh, appPid()), 'a tab opened with the setting off runs inside the app');
  check(!under(fresh, keeperPid()), 'and not under the resident process');
  // The kept ones are gone back to, not started twice. Its first line has
  // scrolled away by now: its count of calls going on from where it was is
  // what says the screen is the same program's
  const was = lastCall(await screenSays())?.n ?? 0;
  const app = appPid();
  ps('-Command', `& taskkill.exe /PID ${app} /T /F 2>&1 | Out-Null`);
  await until(() => !alive(app), 'the app to be gone', 15000);
  start();
  await until(() => appPid() !== null, 'the app again');
  await until(async () => (lastCall(await screenSays())?.n ?? 0) > was, 'the kept program, with the setting off');
  await sleep(4000);
  check(programs().length === 1 && programs()[0] === first, `the kept program is gone back to, not started again (${programs().join(', ')})`);
  check(servers().length === 1 && programsOf('bg-word').length === 1, 'the kept server too');
  // The settings: how many still run, and the press that ends them
  b = await board();
  await b.run('openSettings("basic")');
  let cfgTarget;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    return p && (cfgTarget = (await targetsOf(p)).find((t) => t.type === 'page' && /section=basic/.test(t.url)));
  }, 'the settings page', 40000);
  const s = await connect(cfgTarget, 'settings');
  await until(() => s.run('!!document.querySelector(".keepheld") && !document.querySelector(".keepheld").hidden'), 'the line of terminals still running', 30000);
  const said = await s.run('document.querySelector(".keepheld").textContent');
  console.log('   the settings say: ' + said);
  check(said.includes('2'), 'the settings say two are still running');
  check(!(await s.run('document.querySelector("input[type=checkbox]") && [...document.querySelectorAll("label.check")].find(l => l.textContent.includes(' + JSON.stringify(L['settings.keep_terminals.label']) + ')).querySelector("input").checked')), 'the setting reads as off');
  check(await s.run(`![...document.querySelectorAll(".hint")].some(h => h.textContent === ${JSON.stringify(L['settings.keep_terminals.store'])})`), 'no Store line in a copy that is not from the Store');
  await s.run('document.querySelector(".keepheld").scrollIntoView({block: "center"})');
  await s.shot('6-still-running');
  await s.press('document.querySelector(".keepheld button")');
  await until(() => s.run('!!document.querySelector("dialog button.danger")'), 'the question before stopping');
  await s.shot('6-ask');
  await s.press('document.querySelector("dialog button.danger")');
  const keeper = keeperPid();
  await until(() => !alive(first) && servers().length === 0, 'the kept programs to end', 20000);
  check(true, 'the kept programs ended');
  await until(() => keeper === null || !alive(keeper), 'the resident process to end', 20000);
  check(true, 'the resident process ended');
  await until(() => s.run('document.querySelector(".keepheld").hidden'), 'the line to go', 15000);
  check(true, 'the line is gone');
  await sleep(4000);
  check(keeperPid() === null, 'no resident process was started again to look for the stopped terminals');
  b = await board();
  await until(() => b.run('["held", "bg"].every(id => (S.tabs.find(t => t.id === id) || {}).state === "EXIT")'), 'the stopped tabs to read as ended', 20000);
  check(true, 'the stopped tabs read as ended, not as waiting for a line');
  b.close();
  check(alive(programsOf('fresh-word')[0]), 'the tab inside the app is untouched');
  s.close();
  b.close();
}

/** Freeze or thaw every thread of `pid`: a process that cannot answer */
const freeze = (pid, on) => ps('-Command',
  `Add-Type -Name Nt -Namespace Freeze -MemberDefinition '[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h); [DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr h);'; ` +
  `$p = Get-Process -Id ${pid}; [void][Freeze.Nt]::${on ? 'NtSuspendProcess' : 'NtResumeProcess'}($p.Handle)`);
/** Quit through the window, answering `answer`; what the question said */
const quitApp = (answer, ...more) => {
  const quit = ps('-File', path.join(ROOT, 'tools', 'debug', 'lib', 'quit-app.ps1'), '-Root', APP, '-Answer', answer, ...more);
  console.log('   ' + quit.stdout.trim().split(/\r?\n/).join('\n   '));
  return quit.stdout;
};

console.log('8. the resident process cannot answer: "No" does not quit, and says so');
{
  const app = appPid();
  ps('-Command', `& taskkill.exe /PID ${app} /T /F 2>&1 | Out-Null`);
  await until(() => !alive(app), 'the app to be gone', 15000);
  for (const pid of programsOf('fresh-word')) ps('-Command', `Stop-Process -Id ${pid} -Force -ErrorAction SilentlyContinue`);
  editConfig((c) => { delete c.keep_terminals; });
  start();
  await until(() => programs().length === 1 && keeperPid() !== null, 'the tab held again');
  const held = programs()[0];
  const keeper = keeperPid();
  const running = appPid();
  await sleep(3000);
  freeze(keeper, true);
  quitApp('No');
  const failed = L['msg.quit.stop_failed'].split('{why}')[0];
  const again = quitApp('Cancel', '-Asked', '-Seconds', '30');
  check(again.includes(failed), 'the question comes back saying the terminals could not be stopped');
  await sleep(1500);
  check(appPid() === running, 'the app is still there after "Cancel"');
  check(alive(held), 'the program still runs');
  quitApp('Yes');
  await until(() => appPid() === null, 'the app to quit', 20000);
  check(alive(held), 'quit leaving them: the program still runs');
  freeze(keeper, false);
  stopAll();
  await sleep(1500);
}

console.log('7. the setting unset again: held again, and "No" stops everything');
{
  start();
  await until(() => programs().length === 1 && keeperPid() !== null, 'the tab held again');
  const again = programs()[0];
  check(under(again, keeperPid()), 'the new program runs under a new resident process');
  const keeper = keeperPid();
  const quit = ps('-File', path.join(ROOT, 'tools', 'debug', 'lib', 'quit-app.ps1'), '-Root', APP, '-Answer', 'No');
  console.log('   ' + quit.stdout.trim().split(/\r?\n/).join('\n   '));
  await until(() => !alive(again), 'the program to end', 20000);
  check(true, 'the program ended');
  await until(() => keeper === null || !alive(keeper), 'the resident process to end', 20000);
  check(true, 'the resident process ended');
}

stopAll();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
