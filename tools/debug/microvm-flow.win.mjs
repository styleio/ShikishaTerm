/**
 * A project onto a MicroVM, from the add-a-project dialog to its first
 * worktree and back out, through the running app's own window and a real
 * E2B account.
 *
 * What a person does, in order:
 *
 *   1. "Add a project" > "Clone onto a MicroVM": the address, and the MicroVM
 *      chosen from a list whose last line adds one -- the settings' own form,
 *      over the board, saved as it closes. What the machine will sign in to
 *      the git server as is said before anything is made
 *   2. the checkout is made on a machine of its own and written down as the
 *      project's, with a folder of its own on the desk
 *   3. the project goes through its worktree rules, as every project does,
 *      and on to its first worktree, cut on that MicroVM
 *   4. the worktree is a copy of the checkout's machine, on its branch, signed
 *      in to GitHub with the token kept outside the machine
 *   5. deleting the worktree deletes its machine; the checkout's stays
 *
 * Every step is checked against config/config.json and the service itself,
 * not only against the pages.
 *
 *     cargo build
 *     node tools/debug/microvm-flow.win.mjs
 *
 * Needs Windows, Node, git, and in .private/.env: E2B_API_TOKEN (the MicroVM
 * service's key) and GITHUB_PAT (a fine-grained token, handed to the isolated
 * copy as a git account of its own so this PC's own sign-in is never sent).
 * Makes two machines on that account for a minute or two, and deletes every
 * machine it made on the way out. Isolated the way worktree-rules.win.mjs is.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-microvm');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
// A small public repository everybody can clone
const URL = 'https://github.com/octocat/Hello-World.git';
const PROJECT = 'Hello-World';
const CHECKOUT = '/home/user/Hello-World';
const BRANCH = 'check/first';
const WORKTREE = '/home/user/Hello-World-first';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

const dotenv = Object.fromEntries(fs.readFileSync(path.join(ROOT, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const KEY = dotenv.E2B_API_TOKEN;
const PAT = dotenv.GITHUB_PAT;
if (!KEY || !PAT) die('E2B_API_TOKEN and GITHUB_PAT are needed in .private/.env');

// The service, asked directly, to check what the app says it did
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
  language: 'ja',
  remote: { enabled: false },
  confirm_worktree_delete: false,
  git_accounts: [{ name: 'check', owners: ['octocat'] }],
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: RUN, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));
fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { 'git/check': PAT, e2b_api_key: KEY } }, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

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

async function connect(target, name) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(name + ': ' + (r.exceptionDetails.exception?.description || r.exceptionDetails.text));
    return r.result.value;
  };
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `microvm-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, run, shot };
}
const settingsOn = async (pattern) => {
  let t;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    if (!p) return false;
    t = (await targetsOf(p)).find((x) => x.type === 'page' && pattern.test(x.url));
    return !!t;
  }, 'the settings page ' + pattern, 30000);
  return t;
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
const board = await connect(boardTarget, 'the board');
const saved = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
const desk = () => saved().desks[0];
const project = () => (desk().projects || []).find((p) => p.name === PROJECT) || null;
const folderAt = (cwd) => (desk().folders || []).find((f) => f.cwd === cwd) || null;
let cfg = null;

try {
  await until(() => board.run('typeof apMicrovm === "function" && !!(S && S.groups)'), 'the board');

  console.log('1. the address and the MicroVM, and what it signs in as');
  await board.run('openAddProject(); true');
  check(await board.run('!![...document.querySelectorAll("#addproj [data-ap]")].find(b => b.dataset.ap === "microvm")'), '"Clone onto a MicroVM" is one of the other ways in');
  await board.run('apShow("microvm"); true');
  check(await board.run('document.querySelector("#addproj .bpick .nm").textContent') === 'MicroVM がまだありません', 'with none yet, the list says so');
  await board.run(`(() => { const i = document.querySelector("#addproj input.apin"); i.value = ${JSON.stringify(URL)}; i.dispatchEvent(new Event("input")); return true; })()`);
  // The last line of the list adds one: the settings' own form, over the board
  await board.run('document.querySelector("#addproj .bpick").click(); true');
  await until(() => board.run('!!document.querySelector(".aphostadd")'), 'the list with its last line');
  check(await board.run('document.querySelector(".aphostadd").textContent') === '＋ MicroVM を追加', 'the last line adds one');
  await board.run('document.querySelector(".aphostadd").click(); true');
  cfg = await connect(await settingsOn(/section=microvm-add/), 'the settings');
  await until(() => cfg.run('!!document.querySelector(".modal .mfoot .primary")'), 'the MicroVM form');
  check(await cfg.run('[...document.querySelectorAll(".modal input")].map(i => i.value).includes("30")'), 'the minutes are written in, not left to a default');
  await cfg.shot('0-form');
  await cfg.run('document.querySelector(".modal .mfoot .primary").click(); true');
  await until(() => (saved().hosts || []).some((h) => h.kind === 'e2b'), 'the MicroVM in the settings', 20000);
  const vm = saved().hosts.find((h) => h.kind === 'e2b');
  check(vm.minutes === 30 && vm.template === 'base', 'saved with what it says: ' + JSON.stringify(vm));
  await until(() => board.run(`apVm === ${JSON.stringify(vm.name)}`), 'the picker to go on with it chosen', 20000);
  check(true, 'the picker goes on with the new MicroVM chosen: ' + vm.name);
  await until(() => board.run('!!(S.add_project && S.add_project.sign_in)'), 'what it signs in as', 30000);
  const note = await board.run('S.add_project.sign_in');
  check(note.account === 'check' && note.kind === 'fine', 'it signs in as the account for the owner, a fine-grained token: ' + JSON.stringify(note));
  check(await board.run('!document.querySelector("#addproj .bsignin .warn")'), 'a fine-grained token is not warned about');
  await board.shot('1-clone');

  console.log('2. the checkout, on a machine of its own');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  await until(() => (project()?.homes || []).some((h) => h.sandbox), 'the checkout written down as the project\'s', 240000);
  const home = project().homes[0];
  check(home.host === vm.name && home.at === CHECKOUT, 'the checkout is where it says: ' + JSON.stringify(home));
  check(project().git_account === 'check', 'the account it signed in as is the project\'s own');
  check(!!folderAt(CHECKOUT) && folderAt(CHECKOUT).sandbox === home.sandbox && folderAt(CHECKOUT).project === PROJECT,
    'the checkout has a folder of its own on the desk, on its machine');

  console.log('3. through its rules, to its first worktree');
  cfg = await connect(await settingsOn(/section=project-first/), 'the settings');
  await until(() => cfg.run('!!framed && framed.kind === "rules" && !!document.getElementById("rulesgo")'), 'the rules, as a dialog', 30000);
  const far = await cfg.run('document.querySelector("#floatbody").textContent');
  check(far.includes(vm.name + ' でのワークツリーの配置先') && far.includes('元のフォルダの隣（ワークツリーごとに別のマシン）'), 'the rules say where worktrees go on the MicroVM');
  await cfg.shot('2-rules');
  await cfg.run('document.getElementById("rulesgo").click(); true');
  await until(() => board.run('!document.getElementById("branch").hidden'), 'the worktree dialog on the board', 30000);
  await until(() => board.run(`!!(S.branch && S.branch.host === ${JSON.stringify(vm.name)})`), 'the dialog on the MicroVM', 30000);
  await board.run(`branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await board.run(`(() => { const q = document.getElementById("bq"); q.value = ${JSON.stringify(BRANCH)}; q.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => board.run(`!!(S.branch && S.branch.asked === ${JSON.stringify(BRANCH)} && S.branch.folder)`), 'the app\'s answer', 30000);
  check(await board.run('S.branch.folder') === '/home/user/Hello-World-check-first'
    || await board.run('S.branch.folder') === WORKTREE, 'beside the checkout: ' + await board.run('S.branch.folder'));
  check(await board.run('S.branch.here') === false, 'this PC is not offered: the project is not here');
  await until(() => board.run('!!S.branch.sign_in'), 'what it signs in as', 30000);
  check(await board.run('S.branch.sign_in.kind') === 'fine', 'the dialog says what it signs in as');
  await board.shot('3-branch');
  await board.run('document.querySelector("#branch .bgo .go").click(); true');
  const wt = await (async () => {
    let f = null;
    await until(() => { f = (desk().folders || []).find((x) => x.host === vm.name && x.cwd !== CHECKOUT && x.sandbox); return !!f; },
      'the worktree written down, on its own machine', 240000);
    return f;
  })();
  check(wt.sandbox !== home.sandbox && wt.project === PROJECT, 'on a machine of its own, in the project: ' + JSON.stringify(wt));

  console.log('4. the worktree is a copy of the checkout, signed in from outside');
  const listed = await ours();
  const one = (id) => listed.find((s) => s.sandboxID === id);
  check(!!one(home.sandbox) && !!one(wt.sandbox), 'both machines are the service\'s, marked as this app\'s');
  check(one(wt.sandbox)?.metadata?.project === PROJECT, 'marked with the project');
  // Asked inside the worktree's machine
  const conn = await e2b('POST', `/v2/sandboxes/${wt.sandbox}/connect`, { timeout: 120 });
  const inside = async (cmd) => {
    const { Sandbox } = await import(pathToSdk());
    const box = await Sandbox.connect(wt.sandbox, { apiKey: KEY });
    const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
    return (r.stdout + r.stderr).trim();
  };
  check(conn.status < 300, 'the worktree\'s machine answers');
  const branch = await inside(`git -C ${wt.cwd} branch --show-current`);
  check(branch === BRANCH, 'git there is on the branch: ' + branch);
  const login = await inside('curl -s https://api.github.com/user | grep -m1 \'"login"\' || echo none');
  check(login.includes('"login"'), 'it is signed in to GitHub: ' + login);
  const seen = await inside(`(env; cat /proc/*/environ 2>/dev/null | strings; grep -rs '${PAT.slice(-8)}' /etc /home /root /tmp 2>/dev/null) | grep -c '${PAT.slice(-8)}' || true`);
  check(seen.trim() === '0', 'and the token is nowhere inside it: ' + seen);

  console.log('4b. what it serves answers from anywhere, at the address the menu lists');
  await inside(`cd ${wt.cwd} && echo served-from-the-worktree > index.html && (nohup python3 -m http.server 8000 >/dev/null 2>&1 &) ; sleep 1; echo ok`);
  const g = `(S.groups || []).find(x => x.folder === ${JSON.stringify(wt.cwd)})`;
  await until(() => board.run(`!!${g}`), 'the worktree\'s card', 30000);
  check(await board.run(`onMicrovm(${g})`), 'the card knows it is on a MicroVM');
  await board.run(`openFarPorts(${g}, document.querySelector("#tabs") || document.body); true`);
  await until(() => board.run(`!!(S.far_ports && !S.far_ports.busy && S.far_ports.folder === ${JSON.stringify(wt.cwd)})`), 'the addresses', 60000);
  const ports = await board.run('S.far_ports.ports');
  const p8000 = ports.find((p) => p.port === 8000);
  check(!!p8000 && p8000.url === `https://8000-${wt.sandbox}.e2b.app`, 'the port and its public URL are listed: ' + JSON.stringify(ports));
  check(await board.run('!!document.querySelector(".fmenu.farports")'), 'and shown where the menu was');
  await board.shot('3b-urls');
  if (p8000) {
    const got = await fetch(p8000.url).then((r) => r.text()).catch((e) => String(e));
    check(got.trim() === 'served-from-the-worktree', 'the URL answers from anywhere: ' + got.trim().slice(0, 60));
  }
  await board.run('closeFolderMenu(); true');

  console.log('5. deleting the worktree deletes its machine, and only that');
  await board.run(`discardFolder(${g}); true`);
  await until(async () => !(await ours()).some((s) => s.sandboxID === wt.sandbox), 'the worktree\'s machine to go', 60000);
  check(!folderAt(wt.cwd), 'the folder is off the desk');
  check((await ours()).some((s) => s.sandboxID === home.sandbox), 'the checkout\'s machine stays');
  await board.shot('4-after');
} catch (e) {
  check(false, e.message);
} finally {
  try { board.ws.close(); } catch {}
  try { cfg && cfg.ws.close(); } catch {}
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
