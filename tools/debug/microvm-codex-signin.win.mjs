/**
 * Codex signed in on a MicroVM: its sign-in command in a tab of its own, the
 * address and the code it prints picked out for the person, and the way on
 * through the terminal itself when the step cannot help.
 *
 * A project cloned onto a MicroVM with Codex as its AI, through the running
 * app's own window and a real E2B account, up to the sign-in step. Checked:
 *   - the step shows a tab of its own running Codex's sign-in from another
 *     device (`codex login --device-auth`), in front, mirrored in the step
 *   - the address and the one-time code Codex prints are picked out, each
 *     with a press that copies it
 *   - "Sign in in the terminal" puts the step away and leaves the terminal in
 *     front; signed in there (here: written from outside), the step goes on
 *     by itself, the sign-in tab comes off the desk, and Codex's own tab is
 *     started again to read the sign-in
 *
 * Nobody signs in to anything: the code Codex prints is never entered, and
 * the sign-in is a stand-in file written onto the machine.
 *
 *     cargo build
 *     node tools/debug/microvm-codex-signin.win.mjs
 *
 * Needs Windows, Node, git, and in .private/.env: E2B_API_TOKEN and
 * GITHUB_PAT, as microvm-flow.win.mjs does. Makes one machine on that account
 * for a few minutes, and deletes it on the way out.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-microvm-codex');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const URL = 'https://github.com/octocat/Hello-World.git';
const PROJECT = 'Hello-World';
const CHECKOUT = '/home/user/Hello-World';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const KEY = dotenv.E2B_API_TOKEN;
const PAT = dotenv.GITHUB_PAT;
if (!KEY || !PAT) die('E2B_API_TOKEN and GITHUB_PAT are needed in .private/.env');

const API = 'https://api.e2b.app';
const e2b = async (method, p, body) => {
  const r = await fetch(API + p, { method, headers: { 'X-API-Key': KEY, 'Content-Type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
  const t = await r.text();
  try { return { status: r.status, body: JSON.parse(t) }; } catch { return { status: r.status, body: t }; }
};
const ours = async () => {
  const l = await e2b('GET', '/v2/sandboxes?metadata=' + encodeURIComponent('shikisha=1&project=' + PROJECT) + '&state=running,paused');
  return Array.isArray(l.body) ? l.body : [];
};
const started = new Date();
const cleanUp = async () => {
  for (const s of await ours()) {
    if (new Date(s.startedAt) >= new Date(started.getTime() - 60000)) await e2b('DELETE', '/sandboxes/' + s.sandboxID);
  }
};

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  git_accounts: [{ name: 'check', owners: ['octocat'] }],
  hosts: [{ name: 'vm-check', kind: 'e2b', template: 'base', minutes: 30 }],
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: RUN, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));
fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { 'git/check': PAT, e2b_api_key: KEY } }, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B|CODEX|OPENAI)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};
let boardTarget;
try {
  await until(async () => {
    const p = portOf('shell');
    if (!p) return false;
    boardTarget = (await targetsOf(p)).find((t) => t.type === 'page');
    return !!boardTarget;
  }, 'the window\'s page and its DevTools port', 40000);
} catch (e) { stopApp(); die(e.message); }
const { ws, run, send: cdp } = await connectCdp(boardTarget);
const board = { ws, run, shot: async (label) => {
  const r = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(SHOTS, `microvm-codex-${label}.png`), Buffer.from(r.data, 'base64'));
} };
const saved = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
const desk = () => saved().desks[0];
const project = () => (desk().projects || []).find((p) => p.name === PROJECT) || null;
const folderAt = (cwd) => (desk().folders || []).find((f) => f.cwd === cwd) || null;
const log = () => { const f = path.join(APP, 'logs', 'hooks.log'); return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : ''; };
const on = async (id, cmd) => {
  const { Sandbox } = await import(pathToSdk());
  const box = await Sandbox.connect(id, { apiKey: KEY });
  const r = await box.commands.run(cmd, { timeoutMs: 120000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
  return (r.stdout + r.stderr).trim();
};

try {
  await until(() => board.run('typeof apMicrovm === "function" && !!(S && S.groups)'), 'the board');

  console.log('1. cloned onto a MicroVM, with Codex as its AI');
  await board.run('openAddProject(); true');
  await board.run('apShow("microvm"); true');
  await board.run(`(() => { const i = document.querySelector("#addproj input.apin"); i.value = ${JSON.stringify(URL)}; i.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => board.run('!!(S.add_project && S.add_project.sign_in)'), 'what it signs in as', 30000);
  const aiPick = '[...document.querySelectorAll("#addproj select")].find(s => [...s.options].some(o => o.value === "codex"))';
  await until(() => board.run(`!!${aiPick}`), 'the AI picker with Codex in it', 20000);
  await board.run(`(() => { const s = ${aiPick}; s.value = "codex"; s.dispatchEvent(new Event("change")); return true; })()`);
  check(await board.run(`${aiPick}.value === "codex"`), 'Codex is chosen as the AI to install');
  await board.run('(() => { const r = document.querySelectorAll("#addproj .vmvis input")[0]; r.checked = true; r.dispatchEvent(new Event("change")); })(); true');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  await until(() => board.run('document.getElementById("addproj").hidden'), 'the dialog to close', 30000);
  await until(() => (project()?.homes || []).some((h) => h.sandbox) && !!folderAt(CHECKOUT) && !!folderAt(CHECKOUT).sandbox,
    'the checkout written down as the project\'s', 420000)
    .catch(async (e) => { console.log('    (the board says: ' + JSON.stringify(await board.run('S.making')) + ')'); throw e; });
  const home = project().homes[0];
  check(project().machine_ai === 'codex', 'the project\'s AI is Codex');
  const there = await on(home.sandbox, 'codex --version 2>&1 | head -1');
  check(/codex/i.test(there), 'Codex is installed on the checkout\'s machine: ' + there);

  console.log('2. the sign-in step: its own tab, the address and the code');
  const step = () => board.run('JSON.stringify((S && S.login_step) || null)').then((t) => JSON.parse(t || 'null'));
  await until(async () => ((await step()) || {}).state === 'no', 'the step to say Codex is not signed in', 120000);
  await until(async () => ((await step()) || {}).tab === 'codex sign-in', 'the step to show its sign-in tab', 30000)
    .catch(async (e) => { console.log('    (the step: ' + JSON.stringify(await step()) + ')'); throw e; });
  check(true, 'the step shows a tab of its own for the sign-in');
  const signinTab = () => (folderAt(CHECKOUT)?.tabs || []).find((t) => t.name === 'codex sign-in');
  await until(() => !!signinTab(), 'the sign-in tab on the desk', 20000);
  check(/codex login --device-auth/.test(JSON.stringify(signinTab().command)), 'it runs Codex\'s sign-in from another device: ' + JSON.stringify(signinTab().command));
  await until(() => board.run('(() => { const t = (S.tabs || []).find(x => x.index === S.active); return !!t && t.name === "codex sign-in"; })()'), 'the sign-in tab in front', 30000);
  check(true, 'the sign-in tab is in front');
  const got = await until(async () => { const s = await step(); return /^https:\/\//.test(s.url || '') && !!s.code; }, 'the address and the code', 90000)
    .then(() => step())
    .catch(async () => {
      const text = await board.run('(document.querySelector("#login .lmirror") || {textContent:""}).textContent');
      console.log('    (the terminal says: ' + text.replace(/\s+/g, ' ').trim().slice(-400) + ')');
      return step();
    });
  check(/^https:\/\//.test(got.url || ''), 'the address Codex printed is picked out: ' + (got.url || '(none)'));
  check(/^[A-Z0-9]{4,8}(-[A-Z0-9]{4,8})+$/.test(got.code || ''), 'the one-time code is picked out: ' + (got.code || '(none)'));
  check(await board.run('!document.querySelector("#login .ldevrow").hidden && document.querySelector("#login .ldev").textContent === ' + JSON.stringify(got.code || '-')),
    'the code is shown beside a press that copies it');
  await board.shot('1-step');

  console.log('3. signed in in the terminal instead');
  const terminal = await board.run('(() => { const b = [...document.querySelectorAll("#login .sfoot button")].find(b => b.textContent === T["tui.login.terminal"]); if (b) b.click(); return !!b; })()');
  check(terminal, '"Sign in in the terminal" is offered');
  await until(() => board.run('document.getElementById("login").hidden && !S.login_step'), 'the step put away', 20000);
  check(await board.run('(() => { const t = (S.tabs || []).find(x => x.index === S.active); return !!t && t.name === "codex sign-in"; })()'), 'the terminal is left in front');
  // Signed in from outside: a stand-in of what Codex writes
  await on(home.sandbox, 'mkdir -p ~/.codex && echo \'{"OPENAI_API_KEY":"stand-in"}\' > ~/.codex/auth.json');
  await until(() => !signinTab(), 'the sign-in tab to come off the desk', 60000)
    .catch(() => {});
  check(!signinTab(), 'once signed in, the step goes on by itself and the sign-in tab comes off the desk');
  check(/its tab is started again to read it/.test(log()), 'Codex\'s own tab is started again to read the sign-in');
  await on(home.sandbox, 'rm -f ~/.codex/auth.json');
  await board.shot('2-after');
} catch (e) {
  check(false, e.message);
} finally {
  try { board.ws.close(); } catch {}
  stopApp();
  await cleanUp();
  const left = await ours();
  console.log(left.length ? `  (left on the account: ${left.map((s) => s.sandboxID).join(', ')})` : '  every machine it made is deleted');
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);

// The service's own SDK, where the scratch copy of it is; installed with
// `npm i e2b` into target/e2b-sdk when it is not
function pathToSdk() {
  const dir = path.join(ROOT, 'target', 'e2b-sdk');
  const entry = path.join(dir, 'node_modules', 'e2b', 'dist', 'index.mjs');
  if (!fs.existsSync(entry)) {
    fs.mkdirSync(dir, { recursive: true });
    spawnSync('npm', ['init', '-y'], { cwd: dir, shell: true });
    spawnSync('npm', ['i', 'e2b', '--silent'], { cwd: dir, shell: true });
  }
  return 'file://' + entry.replace(/\\/g, '/');
}
