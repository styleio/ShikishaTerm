/**
 * An AI started by hand inside a shell tab is noticed, and let go of again.
 *
 * The case is the ordinary one: somebody opens a terminal, types `claude`, and
 * the tab goes on calling itself a shell -- no mark, no state, and the
 * emergency stop with nothing to press. What decides it is not the tab's
 * command but what is running under it, so nothing short of a real process in
 * a real tab proves this works: the job has to list it, Windows has to hand
 * over its command line, and the profile has to be picked up while the tab is
 * already running.
 *
 *     cargo build
 *     node tools/debug/guest-ai.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder,
 * its own LOCALAPPDATA, its own home, and a stub CLI on its PATH -- so no
 * account is used and nothing leaves the machine. The stub is a copy of Node
 * named `claude.exe`, because the name of the program on disk is exactly what
 * this is about. Nothing of a copy somebody is using is read, written or
 * stopped. The app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9347;
const RUN = path.join(os.tmpdir(), 'sk-guest-ai');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work', 'shop');
const HOME = path.join(RUN, 'home');
const STUB = path.join(RUN, 'stub');
const CONFIG = path.join(APP, 'config', 'config.json');

/** How long the stand-in AI stays up before ending on its own. Long enough to
 *  be noticed, short enough that the letting go can be watched in one run */
const ALIVE_MS = 25000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
const node = process.execPath;

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, HOME, STUB, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });

// The AI, as far as this check needs one: a program called `claude` that sits
// there. A copy of Node rather than a script, because a script is run by
// something else and the name on the process would be that something else --
// which is the harder half of this and is covered by the tests in `guest.rs`
fs.copyFileSync(node, path.join(STUB, 'claude.exe'));
fs.writeFileSync(path.join(STUB, 'wait.js'),
  `setTimeout(() => process.exit(0), ${ALIVE_MS});\n`);

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
// A shell tab, and nothing about it says AI. What it goes on to run is the
// same thing typing the name at its prompt would run
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  desks: [{
    name: 'Check', id: 'check',
    folders: [{
      cwd: WORK,
      tabs: [{
        name: 'shell', id: 'shell',
        // Named in full rather than as `claude`: a tab's environment is built
        // from the registry, not from this run's, so a folder put on PATH here
        // is not on the tab's -- and a bare name would find whatever is really
        // installed on the machine, which is a real AI on somebody's account
        command: ['powershell.exe', '-NoLogo', '-NoProfile', '-NoExit', '-Command',
          `& "${path.join(STUB, 'claude.exe')}" "${path.join(STUB, 'wait.js')}"`],
      }],
    }],
  }],
}, null, 2));

const starter = path.join(RUN, 'start.ps1');
fs.writeFileSync(starter, [
  '$ErrorActionPreference = "Stop"',
  `$env:LOCALAPPDATA = "${path.join(RUN, 'localappdata')}"`,
  `$env:USERPROFILE = "${HOME}"`,
  `$env:PATH = "${STUB};" + $env:PATH`,
  `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=${PORT}"`,
  'foreach ($e in @(Get-ChildItem env: | Where-Object { $_.Name -match "^(CLAUDE|ANTHROPIC)" })) { Remove-Item ("env:" + $e.Name) }',
  `Start-Process -FilePath "${path.join(APP, 'SHIKISHA-TERM.exe')}" -WorkingDirectory "${APP}"`,
].join('\r\n') + '\r\n');
const started = ps('-File', starter);
if (started.status !== 0) die('the copy did not start:\n' + started.stdout + started.stderr);

let targets;
for (let i = 0; i < 240 && !targets; i++) {
  try {
    const found = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
    targets = found.filter((t) => t.type === 'page');
  } catch { /* not up yet */ }
  if (targets && !targets.length) targets = null;
  if (!targets) await sleep(500);
}
if (!targets) { stopApp(); die('the app\'s page never opened its DevTools port'); }
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r, { once: true }));
let id = 0;
const waiting = new Map();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
});
const call = (method, params = {}) => new Promise((res, rej) => {
  const n = ++id;
  waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
  ws.send(JSON.stringify({ id: n, method, params }));
});
const run = async (expression) => {
  const r = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
};
const until = async (test, what, ms = 40000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};
const tab = () => run('JSON.stringify((S && S.tabs && S.tabs[0]) || null)').then((t) => JSON.parse(t || 'null'));

try {
  console.log('1. the tab starts as what it was opened as');
  await until(async () => !!(await tab()), 'the tab to appear');
  const first = await tab();
  check(first.profile === 'GENERIC', `it is read as a shell at first: ${first.profile}`);

  console.log('2. the AI running inside it is noticed');
  await until(async () => (await tab()).profile === 'Claude Code', 'the AI to be noticed');
  const seen = await tab();
  check(seen.profile === 'Claude Code', `the screen is read as the AI's: ${seen.profile}`);
  check(seen.ai === 'claude', `the tab wears the AI's mark: ${seen.ai}`);
  check(seen.name === 'shell', 'the name the person gave the tab is left alone');
  check(await run('!!document.querySelector("#strip .stab .aim.ai-claude")'),
    'the mark is drawn on the tab');

  console.log('3. and the notice says what this tab will not do afterwards');
  const said = await run('document.querySelector("#toast") && document.querySelector("#toast").textContent');
  check(/not restored/.test(said || ''), `the notice was shown: ${JSON.stringify(said)}`);

  console.log('4. when the AI ends, the tab is a shell again');
  await until(async () => (await tab()).profile === 'GENERIC', 'the AI to be let go of', 40000);
  const after = await tab();
  check(after.profile === 'GENERIC', `it is read as a shell again: ${after.profile}`);
  check(!after.ai, `the mark is gone: ${after.ai}`);
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  try { ws.close(); } catch { /* already gone */ }
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all good');
process.exit(failures ? 1 : 0);
