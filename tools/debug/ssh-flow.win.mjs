/**
 * A project on this PC worked on a server over SSH as well, through the
 * running app's own window and a real server.
 *
 * What a person does, in order:
 *
 *   1. the worktree dialog lists the server under "Where it runs", saying the
 *      project has no checkout there yet
 *   2. choosing it asks, over the worktree dialog, where the project is
 *      there: a folder already there, or a clone. The folder chosen from the
 *      server's folders is written down as the project's checkout there
 *   3. the worktree dialog, never closed and as it was left, goes on on that
 *      server, and the worktree is made there, beside the checkout as the
 *      server's default placement says
 *  3b. on a second server entry, the clone instead: started from the
 *      project's own remote, written down as the project's checkout there
 *      once the server's git is done, and the dialog waits for it -- and the
 *      list draws the project once, a part for each machine
 *   4. the project's rules page lists where worktrees go on that server, and
 *      "beside the checkout" is a press away
 *
 * Every step is checked against config/config.json and the server itself.
 *
 *     cargo build
 *     node tools/debug/ssh-flow.win.mjs
 *
 * Needs Windows, Node, git, the npm package ssh2 (installed into target/ on
 * the first run), and in the main checkout's .private/.env: SSH_TEST_HOST,
 * SSH_TEST_PORT, SSH_TEST_USER, SSH_TEST_PASSWORD (or SSH_TEST_KEY) and
 * SSH_TEST_REPO -- a git repository on that server kept for this check, with
 * one commit. The branches and worktrees it makes there are removed on the
 * way out. Isolated the way worktree-rules.win.mjs is.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-ssh');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

// The private settings live beside the main checkout, whichever worktree runs this
const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const HOST = dotenv.SSH_TEST_HOST, PORT = Number(dotenv.SSH_TEST_PORT || 22), USER = dotenv.SSH_TEST_USER;
const PASSWORD = dotenv.SSH_TEST_PASSWORD, KEY = dotenv.SSH_TEST_KEY, REPO = (dotenv.SSH_TEST_REPO || '').replace(/\/+$/, '');
if (!HOST || !USER || !REPO || !(PASSWORD || KEY)) die('SSH_TEST_HOST, SSH_TEST_USER, SSH_TEST_REPO and a password or key are needed in .private/.env');
const NAME = REPO.split('/').pop();
const BRANCH = 'check/ssh';
// What is typed into the worktree dialog before a server is chosen, which
// saying where the project is there must leave as it was
const KEPT = 'check/kept';
const TREE = `${REPO}.branches/check-ssh`;
// Where clones made from the clone page go on the server, and what is cloned
const CLONES = `${REPO}.clones`;
const HELLO = "https://github.com/octocat/Hello-World.git";
// A repository nobody can read without signing in
const PRIVATE = "https://github.com/styleio/helloworld.git";

// The server, asked directly
const sdk = path.join(ROOT, 'target', 'ssh2-sdk');
if (!fs.existsSync(path.join(sdk, 'node_modules', 'ssh2'))) {
  fs.mkdirSync(sdk, { recursive: true });
  spawnSync('npm', ['init', '-y'], { cwd: sdk, shell: true });
  spawnSync('npm', ['i', 'ssh2', '--silent'], { cwd: sdk, shell: true });
}
const { Client } = createRequire(path.join(sdk, 'package.json'))('ssh2');
const there = (cmd) => new Promise((resolve) => {
  const c = new Client();
  let out = '';
  c.on('ready', () => c.exec(cmd, (err, st) => {
    if (err) { c.end(); resolve('ERR ' + err.message); return; }
    st.on('data', (d) => { out += d; }).stderr.on('data', (d) => { out += d; });
    st.on('close', () => { c.end(); resolve(out.trim()); });
  })).on('error', (e) => resolve('ERR ' + e.message))
    .connect({ host: HOST, port: PORT, username: USER, password: PASSWORD || undefined,
      privateKey: KEY ? fs.readFileSync(KEY) : undefined, readyTimeout: 20000 });
});
const cleanThere = () => there(`cd ${REPO} && for w in $(git worktree list --porcelain | sed -n 's/^worktree //p' | grep -v "^${REPO}$"); do git worktree remove --force "$w"; done; git worktree prune; git branch -D ${BRANCH} 2>/dev/null; rm -rf ${REPO}.branches ${REPO}-ssh ${REPO}.clones; true`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('the server: ' + await there('git --version'));
await cleanThere();
// The server has no AI of its own: one stands in for Codex, where an
// installer puts it, found by a login shell there (steps 3 and 7). It goes
// again on the way out, with the folder when this made it
// With SSH_FLOW_REAL_AI=1 the server's own Codex, installed and signed in
// there beforehand, is used as it is -- nothing is put in its place, and
// nothing of it is taken away
const REAL_AI = process.env.SSH_FLOW_REAL_AI === '1';
if (REAL_AI && !/Logged in/.test(await there('bash -lc "codex login status" 2>&1'))) die('SSH_FLOW_REAL_AI=1, but Codex on the server is not signed in');
const madeBin = !REAL_AI && (await there('test -d ~/.local/bin && echo had || echo new')) === 'new';
if (!REAL_AI) await there('mkdir -p ~/.local/bin && printf "#!/bin/sh\necho stand-in\n" > ~/.local/bin/codex && chmod +x ~/.local/bin/codex');
console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
// The same project on this PC: a checkout of its own, named as the one over there is
const HERE = path.join(RUN, 'work', NAME);
for (const d of [APP, LOCAL, SHOTS, HERE]) fs.mkdirSync(d, { recursive: true });
const git = (...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd: HERE, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
};
git('init', '-q', '-b', 'main');
fs.writeFileSync(path.join(HERE, 'README.md'), 'here\n');
git('add', '-A');
git('commit', '-q', '-m', 'start');
git('remote', 'add', 'origin', REPO);

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
// A relay on this PC in front of the server, which can be told to pass
// nothing on without closing anything: a router that forgot the connection,
// a PC that slept. New connections through it still pass. A server entry of
// its own reaches the server through it, with its password there from the
// start, as the app reads the secrets once (step 7)
const net = await import('node:net');
const pipes = [];
const relay = net.createServer((near) => {
  const far = net.connect(PORT, HOST);
  const p = { silent: false };
  pipes.push(p);
  near.on('data', (d) => { if (!p.silent) far.write(d); });
  far.on('data', (d) => { if (!p.silent) near.write(d); });
  for (const e of [near, far]) e.on('error', () => {});
  near.on('close', () => far.destroy());
  far.on('close', () => near.destroy());
});
await new Promise((r) => relay.listen(0, '127.0.0.1', r));

fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: false },
  hosts: [{ name: 'srv', at: `ssh://${USER}@${HOST}:${PORT}`, ...(KEY ? { key: KEY } : {}) },
    { name: 'relay', at: `ssh://${USER}@127.0.0.1:${relay.address().port}`, keepalive: 3, ...(KEY ? { key: KEY } : {}) },
    // The same server again, for the clone of step 3b. Not srv2: step 6
    // adds a server of that name from the page
    { name: 'twin', at: `ssh://${USER}@${HOST}:${PORT}`, ...(KEY ? { key: KEY } : {}) }],
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: HERE, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));
fs.writeFileSync(SECRETS, JSON.stringify({ tokens: PASSWORD ? { 'ssh/host/srv/password': PASSWORD, 'ssh/host/relay/password': PASSWORD, 'ssh/host/twin/password': PASSWORD } : {} }, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
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
async function connect(target, name) {
  const {ws, send, run} = await connectCdp(target);
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `ssh-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, run, shot };
}

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
// The isolated app has met no server yet, and asks on the board whether to
// trust each one the first time. Answered as a person would, trusting the key
// the board shows -- only for a first meeting: a key that changed is never
// answered here
const trustFirst = setInterval(() => board.run(`(() => {
  for (const c of (S && S.key_changes) || []) if (!c.before) send({kind:"hostkey", machine:c.machine, fingerprint:c.now, trust:true});
  return true; })()`).catch(() => {}), 1000);
const saved = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
const desk = () => saved().desks[0];
const project = () => (desk().projects || []).find((p) => p.name === NAME) || null;
let cfg = null;

try {
  await until(() => board.run('typeof tellCheckout === "function" && !!(S && S.groups && S.groups.length)'), 'the board');
  const g = `(S.groups || []).find(x => sameFolder(x.folder, ${JSON.stringify(HERE)}))`;
  await until(() => board.run(`!!${g}`), 'the project here');

  console.log('1. the server is listed, with no checkout of the project yet');
  await board.run(`openBranch(${g}); true`);
  await until(() => board.run('!!(S.branch && (S.branch.hosts || []).length)'), 'the dialog\'s answer');
  const offer = await board.run('S.branch.hosts.find(h => h.name === "srv")');
  check(offer && offer.kind === 'ssh' && !offer.at, 'the server is offered, with no checkout there: ' + JSON.stringify(offer));
  check(await board.run('S.branch.here') === true, 'this PC is offered: the project is here');
  check(await board.run(`document.getElementById("bdest").textContent`) === "この PC▾", "the box says this PC and nothing else: " + await board.run(`document.getElementById("bdest").textContent`));
  await board.run('document.getElementById("bdest").click(); true');
  await until(() => board.run('[...document.querySelectorAll(".fmenu .aphost")].some(r => r.textContent.includes("srv"))'), 'the list of places');
  check(await board.run('[...document.querySelectorAll(".fmenu .aphost")].find(r => r.textContent.includes("srv")).textContent.includes("プロジェクトの場所を指定する")'),
    'the row says what choosing it will ask');
  await board.shot('0-dest');
  await board.run(`branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await board.run(`(() => { const q = document.getElementById("bq"); q.value = ${JSON.stringify(KEPT)}; q.dispatchEvent(new Event("input")); return true; })()`);

  console.log('2. choosing it asks where the project is over there, from its folders');
  await board.run('[...document.querySelectorAll(".fmenu .aphost")].find(r => r.textContent.includes("srv")).click(); true');
  await until(() => board.run('!document.getElementById("addproj").hidden && apStep === "locate" && apHost === "srv"'), 'the question where the project is there');
  check(await board.run(`apFor === ${JSON.stringify(NAME)}`), 'asked for this project');
  check(await board.run('!document.getElementById("branch").hidden'), 'asked over the worktree dialog, which stays open under it');
  check(await board.run('[...document.querySelectorAll("#addproj [data-ap]")].map(b => b.dataset.ap).join(",")') === 'folder,clone',
    'two ways: a folder already there, or a clone');
  await board.shot('1-locate');
  await board.run('document.querySelector("#addproj [data-ap=folder]").click(); true');
  await until(() => board.run('apStep === "remote"'), 'the folders of the server');
  const lookThere = () => board.run(`(() => { const i = document.querySelector("#addproj input.apin"); i.value = ${JSON.stringify(REPO)}; i.dispatchEvent(new KeyboardEvent("keydown", {key:"Enter"})); return true; })()`);
  await lookThere();
  // Looked at again once the server's key is trusted: the first look was
  // refused before anything was sent
  let lookedAt = Date.now();
  await until(async () => {
    if (await board.run(`!!(S.remote_list && !S.remote_list.busy && !S.remote_list.error && S.remote_list.at === ${JSON.stringify(REPO)})`)) return true;
    if (await board.run('!!(S.remote_list && S.remote_list.error)') && Date.now() - lookedAt > 3000) { lookedAt = Date.now(); await lookThere(); }
    return false;
  }, 'the listing of the repository over there', 60000)
    .catch(async (e) => { console.log('    (the dialog has: ' + await board.run('JSON.stringify({step: apStep, host: apHost, list: S.remote_list || null, inputs: [...document.querySelectorAll("#addproj input")].map(i => i.value)})') + ')'); await board.shot('x-remote'); throw e; });
  check(await board.run('S.remote_list.git') === true, 'the folder over there is a repository');
  await board.shot('1-remote');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  await until(() => (project()?.homes || []).some((h) => h.host === 'srv'), 'the checkout over there written down as the project\'s', 30000);
  check(project().homes[0].at === REPO, 'the checkout there is the folder chosen: ' + JSON.stringify(project().homes));
  check(project().at && path.resolve(project().at) === path.resolve(HERE), 'and the project keeps its checkout here: ' + project().at);
  const card = (desk().folders || []).find((f) => f.host === 'srv' && f.cwd === REPO);
  check(!!card && card.project === NAME, 'the folder over there is on the desk, in the project');

  console.log('3. back in the dialog, on that server, and the worktree made there');
  await until(() => board.run('!document.getElementById("branch").hidden && branchHost === "srv"'), 'the worktree dialog, back on the server', 30000);
  check(await board.run('document.getElementById("bq").value') === KEPT, 'the dialog is as it was left: the name typed before is still there');
  await board.run(`branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await board.run(`(() => { const q = document.getElementById("bq"); q.value = ${JSON.stringify(BRANCH)}; q.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => board.run(`!!(S.branch && S.branch.asked === ${JSON.stringify(BRANCH)} && S.branch.folder && S.branch.host === "srv")`), 'the app\'s answer', 30000);
  const planned = await board.run('S.branch.folder');
  check(planned === TREE, 'where the server\'s default placement says: ' + planned);
  check(await board.run('S.branch.line').then((l) => l.includes(`git -C ${REPO} worktree add`)), 'cut from the checkout over there');
  // The AI the server has, and whether it is signed in there: every worktree
  // on the server shares that sign-in, so it is said before one is made
  await until(() => board.run(`!!(S.branch.ai_sign_in && S.branch.ai_sign_in.state === ${JSON.stringify(REAL_AI ? 'yes' : 'no')})`), REAL_AI ? 'the server\'s own Codex said to be signed in' : 'the server\'s AI said to be signed out', 60000)
    .catch(async (e) => { console.log('    (the dialog has: ' + await board.run('JSON.stringify(S.branch.ai_sign_in || null)') + ')'); throw e; });
  check(await board.run('S.branch.ai_sign_in.ai') === 'codex', 'the AI asked about is the one the server has, not this PC\'s');
  // What the worktree runs is chosen from the server's AIs too
  await until(() => board.run('/Codex/.test(document.getElementById("bstart").textContent)'), 'the worktree to run the AI the server has', 10000)
    .catch(async (e) => { console.log('    (the picker says: ' + await board.run('document.getElementById("bstart").textContent') + ')'); throw e; });
  check(await board.run('branchStart') === 'codex', 'the worktree runs the AI the server has, not this PC\'s');
  if (REAL_AI) {
    // Signed in for real, by Codex's own sign-in on the server
    await until(() => board.run('/サーバーでログイン済みです/.test(document.querySelector("#branch .baisignin").textContent)'), 'the dialog saying so', 10000);
    check(true, 'the server\'s own Codex, signed in by its own sign-in, is said to be signed in: ' + await board.run('document.querySelector("#branch .baisignin").textContent'));
  } else {
  await until(() => board.run('/サーバーでまだログインしていません/.test(document.querySelector("#branch .baisignin").textContent)'), 'the dialog saying so', 10000);
  check(await board.run('!/元のフォルダのマシン/.test(document.querySelector("#branch .baisignin").textContent)'),
    'said in the words for a server, not for a MicroVM: ' + await board.run('document.querySelector("#branch .baisignin").textContent'));
  // Signed in there -- what Codex writes when it is -- and the dialog, still
  // open, says so in time. Taken away again after
  await there('mkdir -p ~/.codex && echo \'{"stand-in": true}\' > ~/.codex/auth.json');
  try {
    await until(() => board.run('!!(S.branch.ai_sign_in && S.branch.ai_sign_in.state === "yes")'), 'the server\'s AI said to be signed in', 120000)
      .catch(async (e) => { console.log('    (the dialog has: ' + await board.run('JSON.stringify(S.branch.ai_sign_in || null)') + ')'); throw e; });
    await until(() => board.run('/サーバーでログイン済みです/.test(document.querySelector("#branch .baisignin").textContent)'), 'the dialog saying so', 10000);
    check(true, 'signed in on the server, the dialog says so without being closed: ' + await board.run('document.querySelector("#branch .baisignin").textContent'));
  } finally {
    await there('rm -rf ~/.codex');
  }
  }
  await board.shot('2-branch');
  await board.run('document.querySelector("#branch .bgo .go").click(); true');
  await until(() => (desk().folders || []).some((f) => f.host === 'srv' && f.cwd === TREE), 'the worktree written down', 60000);
  const made = await there(`git -C ${TREE} branch --show-current`);
  check(made === BRANCH, 'git over there is on the branch: ' + made);
  check((desk().folders || []).find((f) => f.cwd === TREE).project === NAME, 'the worktree is in the project');

  console.log('3b. on another server entry, the project cloned there from its own remote');
  const firstPages = async () => {
    const p = portOf(path.join('profiles', 'default'));
    return p ? (await targetsOf(p)).filter((t) => t.type === 'page' && /section=project-first/.test(t.url)).length : 0;
  };
  const pagesBefore = await firstPages();
  const projectsBefore = (desk().projects || []).length;
  await board.run(`openBranch(${g}); true`);
  await until(() => board.run('!!(S.branch && (S.branch.hosts || []).some(h => h.name === "twin"))'), 'the dialog\'s answer');
  check(await board.run('S.branch.origin') === REPO, 'the dialog knows where the project is fetched from: ' + await board.run('S.branch.origin'));
  await board.run(`branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await board.run(`(() => { const q = document.getElementById("bq"); q.value = ${JSON.stringify(KEPT)}; q.dispatchEvent(new Event("input")); return true; })()`);
  await board.run('document.getElementById("bdest").click(); true');
  await until(() => board.run('[...document.querySelectorAll(".fmenu .aphost")].some(r => r.textContent.includes("twin"))'), 'the list of places');
  await board.run('[...document.querySelectorAll(".fmenu .aphost")].find(r => r.textContent.includes("twin")).click(); true');
  await until(() => board.run('!document.getElementById("addproj").hidden && apStep === "locate" && apHost === "twin"'), 'the question for the second entry');
  check((await board.run('document.querySelector("#addproj [data-ap=clone]").textContent')).includes(REPO), 'the clone says what it clones from');
  await board.run('document.querySelector("#addproj [data-ap=clone]").click(); true');
  await until(() => board.run('apStep === "sshclone"'), 'the clone page');
  check(await board.run('document.querySelector("#addproj input.apin").value') === REPO, 'the address starts as the project\'s own remote');
  check(await board.run('document.querySelector("#addproj .stitle").textContent') === 'プロジェクトの場所を指定する', 'the page is still the question it was opened for');
  check(await board.run('!document.querySelector("#addproj .aptabs")'), 'no other server and no other way offered on it');
  await board.run(`(() => { const p = document.querySelector("#addproj .aprow input.apin"); p.value = ${JSON.stringify(CLONES)}; p.dispatchEvent(new Event("input")); return true; })()`);
  await board.shot('3b-clone');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  await until(() => board.run('document.getElementById("addproj").hidden'), 'the question closed on the press', 20000);
  check(await board.run('!document.getElementById("branch").hidden && branchHost === "twin"'), 'the worktree dialog, still open, is on the second entry');
  check(await board.run('document.getElementById("bq").value') === KEPT, 'and as it was left');
  const CLONED = `${CLONES}/${NAME}`;
  // While the server's git clones, the dialog says so rather than that the
  // server has no checkout, and nothing can be made yet
  const waiting = await until(() => board.run('/クローンしています/.test(document.querySelector("#branch .berr").textContent)'), 'the dialog saying it waits for the clone', 20000).then(() => true).catch(() => false);
  if (waiting) {
    check(await board.run('document.querySelector("#branch .bgo .go").disabled'), 'while it clones, the dialog says so and the button waits: ' + await board.run('document.querySelector("#branch .berr").textContent'));
    await board.shot('3b-waiting');
  } else {
    console.log('    (the clone was done before the dialog was looked at)');
  }
  await until(() => (project()?.homes || []).some((h) => h.host === 'twin' && h.at === CLONED), 'the clone written down as the project\'s checkout there', 180000);
  check((await there(`git -C ${CLONED} remote get-url origin`)) === REPO, 'cloned on the server from the project\'s remote');
  check((desk().projects || []).length === projectsBefore, 'no second project was made for it');
  check(((desk().folders || []).find((f) => f.host === 'twin' && f.cwd === CLONED) || {}).project === NAME, 'the folder over there is on the desk, in the project');
  // The dialog asks again by itself once the checkout is there
  await until(() => board.run(`!!(S.branch && S.branch.host === "twin" && !S.branch.error && S.branch.folder)`), 'the dialog ready to make a worktree there', 60000)
    .catch(async (e) => { console.log('    (the dialog has: ' + await board.run('JSON.stringify({host: S.branch && S.branch.host, error: S.branch && S.branch.error})') + ')'); throw e; });
  check(await board.run('!document.querySelector("#branch .bgo .go").disabled'), 'and the button can be pressed: ' + await board.run('S.branch.line'));
  check((await board.run('S.branch.line')).includes(`git -C ${CLONED} worktree add`), 'cut from the clone');
  await sleep(1500);
  check(await firstPages() === pagesBefore, 'the worktree rules of a new project are not opened: the project already has them');
  await board.shot('3b-ready');
  await board.run('closeBranch(); true');
  // The list: one heading for the project, a part for each machine, this PC first
  await until(() => board.run('document.querySelectorAll("#tabs .mhead, nav .mhead, .mhead").length >= 3'), 'the parts of the project', 20000)
    .catch(async (e) => { console.log('    (groups: ' + await board.run('JSON.stringify((S.groups || []).map(g => [g.folder, g.host || "", g.whole || ""]))') + ')'); throw e; });
  const parts = await board.run('[...document.querySelectorAll(".mhead .nm")].map(n => n.textContent).join(",")');
  check(parts === 'この PC,srv,twin', 'the project is drawn once, this PC then each server: ' + parts);
  check(await board.run(`[...document.querySelectorAll(".phead .nm")].filter(n => n.textContent === ${JSON.stringify(NAME)}).length`) === 1, 'under one heading');
  await board.shot('3b-list');

  console.log('4. the rules page says where worktrees go on the server');
  await board.run(`openSettings("project-rules", true, ${JSON.stringify(HERE)}); true`);
  let cfgTarget;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    if (!p) return false;
    cfgTarget = (await targetsOf(p)).find((t) => t.type === 'page' && /section=project-rules/.test(t.url));
    return !!cfgTarget;
  }, 'the settings on the project\'s rules', 30000);
  cfg = await connect(cfgTarget, 'the settings');
  await until(() => cfg.run('!!document.querySelector("[data-rules=\\"place@srv\\"]")'), 'the server\'s line', 30000);
  const line = await cfg.run('document.querySelector("[data-rules=\\"place@srv\\"]").closest(".row").textContent');
  check(line.includes('srv でのワークツリーの配置先') && line.includes('{origin_folder}.branches') && line.includes(REPO), 'the line says where, as written, and from which checkout: ' + line);
  check(await cfg.run(`(() => { const rows = [...document.querySelectorAll("#project-rules .rulesrow")].map(r => r.id || r.textContent.slice(0, 12)); return rows.indexOf("project-place") < rows.findIndex(t => t.startsWith("srv")); })()`), "this PC's place comes before the server's");
  await cfg.run('document.querySelector("[data-rules=\\"place@srv\\"]").click(); true');
  await until(() => cfg.run('!!document.querySelector(".rulesedit .placequick")'), 'the place opened for changing');
  await cfg.run('[...document.querySelectorAll(".rulesedit .placequick button")].find(b => b.textContent === "元のフォルダの隣").click(); true');
  await cfg.shot('3-rules');
  await cfg.run('save(); true');
  await until(() => project().homes[0].placement === '{origin_folder}/..', 'the placement written for the server', 20000);
  check(true, 'beside the checkout is a press away, and saved as written');
  try { cfg.ws.close(); } catch {}
  cfg = null;
  // The next worktree on the server goes where the rule now says
  await board.run(`openBranch(${g}); branchHost = "srv"; drawBranch(); askBranch(); true`);
  await board.run(`branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await board.run(`(() => { const q = document.getElementById("bq"); q.value = "check/next"; q.dispatchEvent(new Event("input")); return true; })()`);
  // Asked again once the saved rules are read in, as the dialog does by itself
  await until(() => board.run(`!!(S.branch && S.branch.asked === "check/next" && S.branch.host === "srv" && S.branch.folder === ${JSON.stringify(REPO + "-check-next")})`), "the answer with the new rule", 30000).catch(() => {});
  check(await board.run('S.branch.folder') === `${REPO}-check-next`, 'the next goes beside the checkout, named for it: ' + await board.run('S.branch.folder'));
  await board.run('closeBranch(); true');

  console.log('4b. a worktree on the server is deleted there, by git there');
  // The red entry is on the worktree's menu, and not on the project's own folder
  const menuOf = (folder) => board.run(`(() => { const g = (S.groups || []).find(x => x.folder === ${JSON.stringify(folder)}); if (!g) return "no group"; folderMenu({currentTarget: document.body, preventDefault(){}}, g); const said = [...document.querySelectorAll(".fmenu .warn")].map(w => w.textContent).join(","); closeFolderMenu(); return said; })()`);
  check(await menuOf(TREE) === '完全削除', 'the worktree on the server can be deleted from its menu');
  check(await menuOf(REPO) === '', 'the project\'s own folder there cannot');
  // Every line the board says while waiting, so a wait that ends in nothing
  // says what was said instead
  const flashSaid = (re, what) => {
    const heard = [];
    return until(() => board.run('S.flash || ""').then((t) => { if (t && heard[heard.length - 1] !== t) heard.push(t); return re.test(t); }), what, 60000)
      .then(() => board.run('S.flash'))
      .catch((e) => { console.log('    (the board said: ' + JSON.stringify(heard) + ')'); throw e; });
  };
  const discard = (folder) => board.run(`send({kind:"folderdiscard", folder:${JSON.stringify(folder)}, unasked:false}); true`);
  // Something not committed there: refused, said why, and nothing is closed
  await there(`echo draft > ${TREE}/draft.txt`);
  await discard(TREE);
  const refused = await flashSaid(/削除していません/, 'the refusal');
  check(/コミットしていないものが 1 件/.test(refused) && !/プッシュしてから/.test(refused), 'what is not committed stops it, and pushing is not asked for: ' + refused);
  check((desk().folders || []).some((f) => f.cwd === TREE) && (await there(`test -e ${TREE}/draft.txt && echo there`)) === 'there', 'the folder stays, on the list and on the server');
  // The project's own folder there is refused the same way
  await discard(REPO);
  check(/ワークツリーではありません/.test(await flashSaid(/削除していません.*ワークツリーではありません/, 'the refusal of the project\'s folder')), 'the project\'s own folder is never removed');
  // Committed, and not pushed: removed, and the branch stays on the server
  await there(`cd ${TREE} && git add draft.txt && git -c user.name=check -c user.email=check@example.invalid commit -qm draft`);
  // Asked for on the settings, from the folder's own page: the settings call
  // it a worktree, and hand deleting it to the board, which deletes it on the
  // server as its own menu does
  await board.run(`openSettings(null, true, ${JSON.stringify(TREE)}); true`);
  let folderPage;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    if (!p) return false;
    folderPage = (await targetsOf(p)).find((t) => t.type === 'page' && /folder=/.test(t.url));
    return !!folderPage;
  }, 'the settings on the worktree\x27s page', 30000);
  const fp = await connect(folderPage, 'the folder\x27s settings');
  try {
    await until(() => fp.run('[...document.querySelectorAll("button.danger")].some(b => b.textContent === "フォルダごと削除")'), 'the delete button on the worktree\x27s page', 30000)
      .catch(async (e) => { console.log('    (the page says: ' + (await fp.run('document.body.innerText')).replace(/\s+/g, ' ').slice(0, 300) + ')'); throw e; });
    check(true, 'the settings offer to delete a worktree on a server');
    await fp.run('[...document.querySelectorAll("button.danger")].find(b => b.textContent === "フォルダごと削除").click(); true');
    await until(() => fp.run('!!document.querySelector("dialog.confirm-box button.danger")'), 'the question before it goes', 10000);
    await fp.run('document.querySelector("dialog.confirm-box button.danger").click(); true').catch(() => true);
  } finally {
    try { fp.ws.close(); } catch {}
  }
  // Said once it is gone, and not covered by the settings being read again
  // -- deleting it wrote them
  const saidGone = until(() => board.run('S.flash || ""').then((t) => /check-ssh を削除しました/.test(t)), 'the board saying it is gone', 90000);
  await until(() => !(desk().folders || []).some((f) => f.cwd === TREE), 'the folder off the list', 60000);
  await until(async () => (await there(`test -e ${TREE} && echo there || echo gone`)) === 'gone', 'the folder gone from the server', 60000);
  check(true, 'the worktree is removed on the server');
  check(!(await there(`git -C ${REPO} worktree list --porcelain`)).includes(TREE), 'git there no longer lists it');
  check((await there(`git -C ${REPO} log -1 --format=%s ${BRANCH}`)) === 'draft', 'its branch stays in the repository there, with the commit not pushed');
  await saidGone;
  await sleep(3000);
  check(/check-ssh を削除しました/.test(await board.run('S.flash || ""')), 'the board says it is gone, and a moment later still does: ' + await board.run('S.flash || ""'));

  console.log('5. a project cloned onto the server, from a page of its own');
  // The page: the address, the server chosen as a MicroVM is, where on it
  // typed or walked to. The server's own git clones; nothing is installed
  await there(`mkdir -p ${CLONES}/plain && touch ${CLONES}/plain/file`);
  const openClone = async (url) => {
    await board.run(`openAddProject(); apShow("sshclone"); true`);
    await until(() => board.run('!!document.querySelector("#addproj .bpick")'), 'the clone page', 10000);
    // The address is the first field; the folder is the one beside the walker
    await board.run(`(() => { const u = document.querySelector("#addproj input.apin"), p = document.querySelector("#addproj .aprow input.apin");
      u.value = ${JSON.stringify(url)}; u.dispatchEvent(new Event("input"));
      p.value = ${JSON.stringify(CLONES)}; p.dispatchEvent(new Event("input")); return true; })()`);
  };
  // With no server yet, the picker says so, and nothing else
  const had = saved();
  fs.writeFileSync(CONFIG, JSON.stringify({ ...had, hosts: [] }, null, 2));
  await until(() => board.run('!(S.hosts || []).length'), 'the settings read in with no server', 20000);
  await board.run(`apHost = ""; openAddProject(); apShow("sshclone"); true`);
  await until(() => board.run('!!document.querySelector("#addproj .bpick")'), 'the clone page', 10000);
  check((await board.run('document.querySelector("#addproj .bpick").textContent')) === 'SSH ホストがまだありません▾',
    'with no server the picker says so and nothing else: ' + await board.run('document.querySelector("#addproj .bpick").textContent'));
  await board.run('closeAddProject(); true');
  fs.writeFileSync(CONFIG, JSON.stringify(had, null, 2));
  await until(() => board.run('(S.hosts || []).some(h => h.name === "srv")'), 'the server back in the settings', 20000);
  // One page for a server: the server above, and the two things to do there
  // as tabs. Adding a folder asks for no address and no account
  await board.run(`openAddProject(); apShow("sshclone"); true`);
  await until(() => board.run('!!document.querySelector("#addproj .aptabs")'), 'the server page', 10000);
  check(await board.run('[...document.querySelectorAll("#addproj .aptabs button")].map(b => b.textContent).join(",")') === 'クローンする,サーバーのフォルダを追加',
    'cloning and adding a folder are two tabs under one server');
  await board.run('document.querySelectorAll("#addproj .aptabs button")[1].click(); true');
  await until(() => board.run('(document.querySelectorAll("#addproj .aprrow") || []).length > 0'), 'the server\'s folders on the add tab', 30000);
  const addTab = await board.run('document.querySelector("#addproj .sbody").textContent');
  check(!/Git の URL/.test(addTab) && !/GitHub アカウント/.test(addTab) && /srv/.test(addTab),
    'the add tab asks for the server and a folder, and for no address or account');
  await board.shot('5-sshadd');
  await board.run('closeAddProject(); true');
  await openClone(HELLO);
  check(await board.run('apHost') === 'srv', 'the server is chosen, as a MicroVM is');
  check(await board.run('[...document.querySelectorAll("#addproj .aphostadd")].length === 0'), 'the list is closed until it is pressed');
  // The walker: the same one the "open a folder there" page walks with
  await board.run(`document.querySelector('#addproj button[title="${'サーバーのフォルダをたどる'}"]').click(); true`);
  await until(() => board.run('(document.querySelectorAll("#addproj .aprrow") || []).length > 0'), 'the server\'s folders, walked', 30000);
  check(await board.run('[...document.querySelectorAll("#addproj .aprrow .nm")].some(n => n.textContent === "plain")'), 'the folders there are listed, to walk into');
  check(!/null/.test(await board.run('document.querySelector("#addproj .aprhere").textContent')), 'the folder being looked at is said, and nothing else');
  check((await board.run('document.querySelector("#addproj .shint.mono").textContent')).includes(`${CLONES}/Hello-World`), 'it says where the clone goes');
  await board.shot('5-sshclone');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  // The dialog closes and the clone stands as a row on the board, as a
  // MicroVM's does, saying what it is doing
  await until(() => board.run('document.getElementById("addproj").hidden'), 'the dialog closed on the press', 20000);
  const row = () => board.run('JSON.stringify((S.making || []).find(m => m.name === "Hello-World") || null)').then((t) => JSON.parse(t || 'null'));
  await until(async () => ((await row()) || {}).stage === 'ssh_cloning', 'a row for the clone on the board', 20000);
  check(/サーバーにリポジトリをクローンしています/.test(await board.run('[...document.querySelectorAll(".making")].map(r => r.textContent).join(" | ")')),
    'the row says it is cloning onto the server');
  const cloned = () => (desk().folders || []).find((f) => f.host === 'srv' && f.cwd === `${CLONES}/Hello-World`);
  await until(() => !!cloned(), 'the clone, added as a folder on the server', 180000);
  check(/octocat\/Hello-World/.test(await there(`git -C ${CLONES}/Hello-World remote get-url origin`)), 'cloned on the server, by the server\'s git');
  await until(async () => !(await row()), 'the row gone once it is on the desk', 20000);
  // On through its rules, as a MicroVM's clone goes
  let rules = null;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    if (!p) return false;
    rules = (await targetsOf(p)).find((t) => t.type === 'page' && /section=project-first/.test(t.url) && t.url.includes(encodeURIComponent(`${CLONES}/Hello-World`).replace(/%2F/g, '/')) || (t.type === 'page' && /section=project-first/.test(t.url)));
    return !!rules;
  }, 'the project\'s worktree creation rules', 30000).catch(() => {});
  check(!!rules, 'the project goes on to its worktree creation rules, as a MicroVM\'s does');
  if (rules) { const c = await connect(rules, 'the rules'); await c.run('closeSettings(); true').catch(() => {}); try { c.ws.close(); } catch {} }
  // The same again: the checkout there is taken in, not cloned over
  await there(`echo mine > ${CLONES}/Hello-World/mine.txt`);
  await openClone(HELLO.replace('.git', ''));
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  await until(async () => !(await row()) && (await board.run('document.getElementById("addproj").hidden')), 'the second clone to finish', 60000);
  check((await there(`cat ${CLONES}/Hello-World/mine.txt`)) === 'mine', 'a checkout of the same repository already there is taken in as it is');
  // Another repository of that name, and a folder that is no repository: said, and nothing touched
  for (const [url, what, word] of [['https://github.com/someone-else/Hello-World.git', 'another repository', '別のリポジトリ'], ['https://github.com/octocat/plain.git', 'a folder that is not one', 'git のリポジトリではありません']]) {
    await openClone(url);
    await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
    // Said on the row, as a MicroVM's failure is, with Try again and Close
    const name = url.split('/').pop().replace('.git', '');
    const failedRow = () => board.run(`JSON.stringify((S.making || []).find(m => m.name === ${JSON.stringify(name)} && m.stage === "failed") || null)`).then((t) => JSON.parse(t || 'null'));
    await until(async () => !!(await failedRow()), 'the refusal on the row', 60000);
    const f = await failedRow();
    check(f.error.includes(word), `${what} there is said on the row, not cloned over: ` + f.error);
    await board.run(`send({kind:"making", id:${f.id}, act:"dismiss"}); true`);
    await until(async () => !(await failedRow()), 'the failed row put away', 20000);
  }
  check((await there(`ls ${CLONES}/plain`)) === 'file', 'and what was there is left as it was');

  console.log('5b. a private repository, and a server whose git cannot sign in to it');
  // The step opens: the commands drafted for this server, to copy -- nothing
  // runs from the page -- and a terminal there, in the clone's folder
  await openClone(PRIVATE);
  // The account is optional and starts empty -- the server's git as it
  // is. The server holds no GitHub account, so nothing is suggested; styleio
  // is typed for this project
  const acctIn = 'document.querySelector("#addproj input.apin[list=apacctlist]")';
  check(await board.run(acctIn + ".value") === "" && /GitHub アカウント（任意）/.test(await board.run('document.querySelector("#addproj .sbody").textContent')),
    'the account is an optional field, empty to begin with');
  await until(() => board.run("apLive && apLive.accountsAsk === 0"), 'the server to say which accounts it holds', 30000);
  check(await board.run('document.querySelectorAll("#apacctlist option").length') === 0,
    'with no account on the server, nothing is suggested');
  await board.run('(() => { const i = ' + acctIn + '; i.value = "bad name"; i.dispatchEvent(new Event("input")); return true; })()');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  check(/アカウント名を、英数字とハイフンで/.test(await board.run('document.querySelector("#addproj .apfoot").textContent')),
    'a name GitHub cannot have is stopped before the clone');
  await board.run('(() => { const i = ' + acctIn + '; i.value = "styleio"; i.dispatchEvent(new Event("input")); return true; })()');
  check(await board.run('document.querySelector("#addproj .apwhy").hidden'),
    'the reason goes as soon as the name is one GitHub can have');
  await board.shot('5b-account');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  const step = () => board.run('JSON.stringify((S && S.login_step) || null)').then((t) => JSON.parse(t || 'null'));
  await until(async () => ((await step()) || {}).kind === 'git', 'the sign-in step for the server\x27s git', 90000);
  const st = await step();
  check(st.account === 'styleio' && /styleio としてサインイン/.test(await board.run('document.getElementById("login").textContent')), 'the step names the account to sign in as: ' + st.account);
  check(st.host === 'srv' && st.folder === CLONES && st.url === PRIVATE.replace('https://', 'https://styleio@'), 'the step is for this server, its folder and this repository: ' + JSON.stringify({ host: st.host, folder: st.folder }));
  check(st.commands.length === 3 && /apt install gh/.test(st.commands[0]) && st.commands[1].startsWith('gh auth login') && st.commands[2] === 'gh auth setup-git',
    'the commands are drafted for an Ubuntu server with no gh: ' + st.commands.map((c) => c.slice(0, 30)).join(' | '));
  await until(() => board.run('!document.getElementById("login").hidden'), 'the step on the board', 20000);
  // Copied, or typed into the terminal without the Enter that runs it: the
  // person reads it there, and presses Enter themselves
  check(await board.run('(() => { const rows = [...document.querySelectorAll("#login .lcmd")]; return rows.length === 3 && rows.every(r => r.querySelectorAll("button").length === 2); })()'),
    'each command has a copy button and one that types it, and nothing that runs it');
  await until(() => board.run('/\$\s*$/.test((document.querySelector("#login .lmirror") || {textContent:""}).textContent.trim() + " ") || /shikisha-test\.clones/.test((document.querySelector("#login .lmirror") || {textContent:""}).textContent)'), 'the server\x27s terminal in the step', 60000);
  check(/shikisha-test.clones/.test(await board.run('document.querySelector("#login .lmirror").textContent')), 'the terminal stands in the clone\x27s folder on the server');
  if (!/shikisha-test.clones/.test(await board.run('document.querySelector("#login .lmirror").textContent'))) {
    console.log('    (the board: ' + await board.run('JSON.stringify({groups:(S.groups||[]).map((g,i)=>i+":"+g.folder), tabs:(S.tabs||[]).map(t=>t.index+":"+t.name+"@"+t.group+"/"+t.kind+"/"+t.state)})') + ')');
    console.log('    (the settings: ' + JSON.stringify((desk().folders||[]).filter((f)=>f.host).map((f)=>({cwd:f.cwd,host:f.host,tabs:f.tabs}))) + ')');
  }
  await until(async () => ((await step()) || {}).state === 'no', 'the step to say the repository cannot be read yet', 60000);
  await board.shot('5b-git-signin');
  // Standing in for gh auth login, which a person does in a browser: a
  // sign-in given to the server's git for a moment, taken away after
  const PAT = dotenv.GITHUB_HELLO_WORLD_PAT;
  check(!!PAT, 'a token for the private repository is in .private/.env (GITHUB_HELLO_WORLD_PAT)');
  // As gh does: a token for the account git names, and nothing for any other
  await there(`git config --global credential.https://github.com.helper '!f() { [ "$1" = get ] || exit 0; ok=; while read l && [ -n "$l" ]; do [ "$l" = username=styleio ] && ok=1; done; [ -n "$ok" ] && echo username=styleio && echo password=${PAT}; true; }; f'`);
  try {
    await until(async () => ((await step()) || {}).state === 'yes', 'the step to see the sign-in', 60000);
    check(true, 'the sign-in is seen while the step is open');
    await board.run('document.getElementById("loginnext").click(); true');
    await until(() => board.run('document.getElementById("login").hidden'), 'the step closed', 20000);
    await until(() => !!(desk().folders || []).find((f) => f.host === 'srv' && f.cwd === `${CLONES}/helloworld`), 'the private clone, made after the sign-in', 180000);
    check(true, '"Clone again" clones it now');
    check((await there(`git -C ${CLONES}/helloworld remote get-url origin`)) === 'https://styleio@github.com/styleio/helloworld.git',
      'the project signs in as its own account: its address names styleio, and no token is in it');
    check(!(await there(`GIT_TERMINAL_PROMPT=0 git ls-remote https://someone-else@github.com/styleio/helloworld.git >/dev/null 2>&1 && echo read || echo refused`)).includes('read'),
      'the same repository asked as another account is not read with styleio\x27s sign-in');
    check(!(desk().folders || []).some((f) => f.host === 'srv' && f.cwd === CLONES), 'and the terminal put there for the step is off the desk');

    console.log('5c. a project of another GitHub account, on the same server');
    // The second account is read from .private/.env and never written here
    const U2 = dotenv.GITHUB_HELLO_WORLD_USER2, PAT2 = dotenv.GITHUB_HELLO_WORLD_PAT2;
    check(!!U2 && !!PAT2, 'a second account and its token are in .private/.env (GITHUB_HELLO_WORLD_USER2, GITHUB_HELLO_WORLD_PAT2)');
    const SECOND = `https://github.com/${U2}/helloworld.git`;
    const SECOND_AT = `${CLONES}/second`;
    // As gh does with two accounts signed in: each one's token for its own
    // name, and nothing for a name it does not hold
    await there(`mkdir -p ${SECOND_AT} && git config --global --replace-all credential.https://github.com.helper '!f() { [ "$1" = get ] || exit 0; u=; while read l && [ -n "$l" ]; do case "$l" in username=*) u=\${l#username=};; esac; done; case "$u" in styleio) echo username=styleio; echo password=${PAT};; ${U2}) echo username=${U2}; echo password=${PAT2};; esac; true; }; f'`);
    await openClone(SECOND);
    await board.run(`(() => { const p = document.querySelector("#addproj .aprow input.apin"); p.value = ${JSON.stringify(SECOND_AT)}; p.dispatchEvent(new Event("input"));
      const a = ${acctIn}; a.value = ${JSON.stringify(U2)}; a.dispatchEvent(new Event("input")); return true; })()`);
    await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
    await until(() => !!(desk().folders || []).find((f) => f.host === 'srv' && f.cwd === `${SECOND_AT}/helloworld`), 'the second account\x27s clone', 180000)
      .catch(async (e) => { console.log('    (the board has: ' + await board.run('JSON.stringify({flash: S.flash, making: S.making, step: S.login_step && {state: S.login_step.state, account: S.login_step.account}})') + ')'); throw e; });
    check((await there(`git -C ${SECOND_AT}/helloworld remote get-url origin`)) === `https://${U2}@github.com/${U2}/helloworld.git`,
      'the second project signs in as its own account, named in its address');
    check((await there(`GIT_TERMINAL_PROMPT=0 git -C ${CLONES}/helloworld fetch -q && GIT_TERMINAL_PROMPT=0 git -C ${SECOND_AT}/helloworld fetch -q && echo both`)) === 'both',
      'both projects fetch on the one server, each as its own account');
    check(!(await there(`GIT_TERMINAL_PROMPT=0 git ls-remote https://styleio@github.com/${U2}/helloworld.git >/dev/null 2>&1 && echo read || echo refused`)).includes('read'),
      'the second account\x27s repository is not read as the first account');
  } finally {
    await there('git config --global --unset-all credential.https://github.com.helper; true');
  }

  console.log('6. a server added from the page, signing in the way it does');
  // From the clone page's own list, as a MicroVM is added from its list: the
  // form, the way it signs in -- a password when the server takes one --
  // and back on the clone page with it chosen, its folders walked with it
  await board.run(`openAddProject(); apShow("sshclone"); true`);
  await until(() => board.run('!!document.querySelector("#addproj .bpick")'), 'the clone page', 10000);
  await board.run('document.querySelectorAll("#addproj .bpick")[0].click(); true');
  await until(() => board.run('!!document.querySelector(".aphostadd")'), 'the list with its last line', 10000);
  await board.run('document.querySelector(".aphostadd").click(); true');
  await until(() => board.run('!!document.querySelector("#addproj select.apin")'), 'the host form', 10000);
  const byPassword = !!PASSWORD;
  await board.run(`(() => {
    const inputs = [...document.querySelectorAll("#addproj input.apin")];
    const set = (i, v) => { i.value = v; i.dispatchEvent(new Event("input")); };
    set(inputs[0], "srv2"); set(inputs[1], ${JSON.stringify(HOST)}); set(inputs[2], ${JSON.stringify(USER)}); set(inputs[3], ${JSON.stringify(String(PORT))});
    const auth = document.querySelector("#addproj select.apin");
    auth.value = ${JSON.stringify(byPassword ? 'password' : 'key')}; auth.dispatchEvent(new Event("change"));
    set(inputs[${byPassword ? 5 : 4}], ${JSON.stringify(byPassword ? PASSWORD : KEY || '')});
    return true; })()`);
  // What is on screen, not what is marked: a field is shown when it takes room
  const shownField = (word) => board.run(`[...document.querySelectorAll("#addproj .sfield")].some(f => f.offsetHeight > 0 && f.querySelector("label") && f.querySelector("label").textContent === ${JSON.stringify(word)} && f.querySelector("input"))`);
  check(await shownField(byPassword ? 'パスワード' : '鍵ファイル') && !(await shownField(byPassword ? '鍵ファイル' : 'パスワード')),
    `the ${byPassword ? 'password' : 'key file'} field is the one shown, and the other is not`);
  await board.shot('6-host-form');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  await until(() => board.run('apHost === "srv2" && !!document.querySelector("#addproj .aprow")'), 'back on the clone page with the new server chosen', 30000);
  const written = (saved().hosts || []).find((h) => h.name === 'srv2');
  check(!!written && !JSON.stringify(written).includes(PASSWORD || '\u0000never'), 'the server is in the settings, and no password is written there: ' + JSON.stringify(written));
  if (byPassword) {
    check(fs.readFileSync(SECRETS, 'utf8').includes('ssh/host/srv2/password'), 'the password is in the secret store, under the name the connection reads');
  }
  await board.run(`document.querySelector('#addproj button[title="サーバーのフォルダをたどる"]').click(); true`);
  await until(() => board.run('(document.querySelectorAll("#addproj .aprrow") || []).length > 0'), 'the new server\'s folders, reached with what the form was given', 30000);
  check(true, 'the new server is reached with what the form was given');
  check(written.keepalive === 30, 'its connection check is written down as a value, not left to a default nobody sees: ' + written.keepalive);
  await board.run('closeAddProject(); true');

  console.log('7. a terminal whose connection goes silent is opened again once the server answers');
  // A terminal on the server through the relay (made before the app, above)
  {
    const withRelay = saved();
    // A second tab whose command is a line of the shell's own: typed there as
    // it was written, its $((..)), && and quotes are the shell's (so SHK-BOTH
    // is on the screen only when the quotes were read, not typed as text)
    withRelay.desks[0].folders.push({ cwd: `/home/${USER}`, host: 'relay', tabs: [{ name: 'far', id: 'far', command: 'bash' },
      { name: 'line', id: 'line', command: 'echo SHK-$((1+1)) && echo "SHK-B"\'OTH\'' }] });
    // A button that hands a request to Claude, as this PC would start it
    withRelay.quick_commands = { items: [{ id: 'ask', label: 'ask', kind: 'ai', body: 'hello', ai: 'claude' }] };
    fs.writeFileSync(CONFIG, JSON.stringify(withRelay, null, 2));
    const farTab = () => board.run('JSON.stringify((S.tabs || []).find(t => t.name === "far") || null)').then((t) => JSON.parse(t || 'null'));
    await until(async () => !!(await farTab()), 'the terminal on the server, through the relay', 60000);
    await board.run(`send({kind:"select", tab: ${(await farTab()).index}}); true`);
    const screen = () => board.run('document.getElementById("screen").textContent');
    const typed = async (line) => {
      await board.run(`send({kind:"key", text:${JSON.stringify(line)}}); true`);
      await board.run('send({kind:"key", named:"enter"}); true');
    };
    await until(async () => /\$\s*$/.test((await screen()).trimEnd()) || /\$/.test(await screen()), 'the server\'s prompt', 60000);
    await typed('echo SHK$((6*7))');
    await until(async () => /SHK42/.test(await screen()), 'the terminal answering', 30000);
    check(true, 'the terminal on the server answers through the relay');
    const lineTab = await board.run('JSON.stringify((S.tabs || []).find(t => t.name === "line") || null)').then((t) => JSON.parse(t || 'null'));
    check(!!lineTab, 'the tab whose command is a line of the shell\x27s is on the board');
    if (lineTab) {
      await board.run(`send({kind:"select", tab: ${lineTab.index}}); true`);
      await until(async () => /SHK-2/.test(await screen()) && /SHK-BOTH/.test(await screen()), 'the line run by the server\x27s shell as written', 60000)
        .catch(async (e) => { console.log('    (the terminal says: ' + (await screen()).replace(/\s+/g, ' ').trim().slice(-200) + ')'); throw e; });
      check(true, 'a tab\x27s own command reaches the server\x27s shell as it was written: $((1+1)) is 2, && runs the second, and its quotes are read');
      await board.run(`send({kind:"select", tab: ${(await farTab()).index}}); true`);
    }
    // A button that hands work to an AI goes to the one the server has, not
    // this PC's
    const dests = () => board.run('JSON.stringify(Object.values(S.quick_to || {}).filter(d => d.how === "open").map(d => d.name))');
    await until(async () => /Codex/.test(await dests()), 'a button handing work to the AI the server has', 60000)
      .catch(async (e) => { console.log('    (the buttons say: ' + await dests() + ')'); throw e; });
    check(!/Claude/.test(await dests()), 'a button hands the work in a server folder to the AI the server has: ' + await dests());
    // Silent from here: nothing is closed, nothing more arrives
    for (const p of pipes) p.silent = true;
    const saidBack = until(() => board.run('S.flash || ""').then((t) => /接続が切れたので、開き直しました/.test(t)), 'the tab said to be opened again', 120000);
    await until(async () => /サーバーとの接続が切れました/.test(await screen()), 'the terminal to say its connection went', 90000)
      .catch(async (e) => { console.log('    (the terminal says: ' + (await screen()).replace(/\s+/g, ' ').trim().slice(-200) + ')'); throw e; });
    check(true, 'the connection going silent is found by the checks nobody answered, and said on the screen');
    await saidBack;
    check(true, 'the tab is opened again once the server answers');
    await until(async () => (await farTab())?.state !== 'EXIT', 'the tab running again', 30000);
    await typed('echo SHK$((7*7))');
    await until(async () => /SHK49/.test(await screen()), 'the new terminal answering', 30000);
    check(true, 'and it answers what is typed into it');
    // A shell ended by its person is ended: said so, and not opened again
    await typed('exit');
    await until(async () => (await farTab())?.state === 'EXIT', 'the tab ended by exit', 30000);
    await sleep(8000);
    check((await farTab())?.state === 'EXIT' && !/接続が切れました/.test((await screen()).split('SHK49').pop()), 'a shell ended with exit ends its tab, and is not opened again');
  }

  console.log('8. what a server listens on opens here, carried over SSH');
  // A development server started there, listening on the server's own
  // loopback only: nothing of it is reachable from outside
  const PORT_THERE = 18765;
  await there(`mkdir -p ~/shk-serve && echo served-from-the-server > ~/shk-serve/index.html && (nohup python3 -m http.server ${PORT_THERE} --bind 127.0.0.1 --directory ~/shk-serve >/dev/null 2>&1 &) ; sleep 1; echo ok`);
  try {
    const rg = `(S.groups || []).find(x => x.folder === ${JSON.stringify(REPO)})`;
    check(await board.run(`(() => { folderMenu({currentTarget: document.body, preventDefault(){}}, ${rg}); const has = [...document.querySelectorAll(".fmenu div")].some(d => d.textContent === "サーバーのポートを開く"); closeFolderMenu(); return has; })()`),
      'a folder on a server offers its ports on its menu');
    await board.run(`showPorts(${rg}); true`);
    const portsSaid = () => board.run('JSON.stringify(S.far_ports || null)').then((t) => JSON.parse(t || 'null'));
    await until(async () => { const v = await portsSaid(); return !!v && !v.busy && v.server && v.ports.some((p) => p.port === PORT_THERE); }, 'the server\x27s port, listed', 60000)
      .catch(async (e) => { console.log('    (the list: ' + JSON.stringify(await portsSaid()) + ')'); throw e; });
    check(true, 'the port the server listens on is listed, not yet carried here');
    await board.run(`send({kind:"farpage", folder:${JSON.stringify(REPO)}, port:${PORT_THERE}}); true`);
    await until(async () => !!((await portsSaid()).ports.find((p) => p.port === PORT_THERE) || {}).url, 'the port carried here', 30000);
    const url = (await portsSaid()).ports.find((p) => p.port === PORT_THERE).url;
    check(/^http:\/\/127\.0\.0\.1:\d+\/$/.test(url), 'carried to this PC\x27s own loopback: ' + url);
    const got = await fetch(url).then((r) => r.text()).catch((e) => String(e));
    check(got.trim() === 'served-from-the-server', 'what the server serves is read here: ' + got.trim().slice(0, 60));
    // A tab of the folder, beside its other tabs, written down as the port
    // over there -- the port here is chosen afresh on every start
    await until(() => board.run(`(S.tabs || []).some(t => t.name === ":${PORT_THERE}" && t.group === (S.groups || []).indexOf(${rg}))`), 'a browser tab on it, in the folder', 30000)
      .catch(async (e) => { console.log('    (the tabs: ' + await board.run('JSON.stringify((S.tabs || []).map(t => [t.name, t.group]))') + ')'); throw e; });
    check(true, 'and opened in a browser tab of the folder, beside its other tabs');
    const written = (desk().folders || []).filter((f) => f.cwd === REPO && f.host);
    check(written.some((f) => (f.tabs || []).some((t) => t.command === `browser far://${PORT_THERE}`)),
      'written down in the folder as the port over there: ' + JSON.stringify(written.map((f) => f.tabs)));
    await board.run('closeFolderMenu(); true');
  } finally {
    await there(`pkill -f "http.server ${PORT_THERE}"; rm -rf ~/shk-serve; true`);
  }

  console.log('9. the same path on two machines is two folders');
  // srv and srv2 (added in 6) are two entries for one server: to the app two
  // machines, each with a folder at the same path
  const SAME = `/home/${USER}/shk-same`;
  await there(`mkdir -p ${SAME}`);
  try {
    const both = saved();
    both.desks[0].folders.push(
      { cwd: SAME, host: 'srv', tabs: [{ name: 'same-a', id: 'same-a', command: 'bash' }] },
      // A git panel too: a row with no terminal of its own, which stands
      // under its folder by the machine the folder is on
      { cwd: SAME, host: 'srv2', tabs: [{ name: 'same-b', id: 'same-b', command: 'bash' }, { name: 'same-git', id: 'same-git', command: 'git' }] });
    fs.writeFileSync(CONFIG, JSON.stringify(both, null, 2));
    const sameGroups = () => board.run(`JSON.stringify((S.groups || []).map((g, i) => ({i, key: g.key, host: g.host, folder: g.folder})).filter(g => g.folder === ${JSON.stringify(SAME)}))`).then((t) => JSON.parse(t));
    await until(async () => (await sameGroups()).length === 2, 'two folders of one path on the board', 60000)
      .catch(async (e) => { console.log('    (the board has: ' + JSON.stringify(await sameGroups()) + ')'); throw e; });
    const gs = await sameGroups();
    check(new Set(gs.map((g) => g.key)).size === 2 && gs.some((g) => g.host === 'srv') && gs.some((g) => g.host === 'srv2'),
      'each is its own folder, on its own machine: ' + gs.map((g) => g.host).join(', '));
    const groupOf = (name) => board.run(`((S.tabs || []).find(t => t.name === ${JSON.stringify(name)}) || {}).group`);
    await until(async () => (await groupOf('same-a')) != null && (await groupOf('same-b')) != null, 'both tabs on the board', 60000);
    const [ga, gb] = [await groupOf('same-a'), await groupOf('same-b')];
    check(ga !== gb && gs.find((g) => g.i === ga).host === 'srv' && gs.find((g) => g.i === gb).host === 'srv2',
      'each tab stands under its own machine\x27s folder');
    await until(async () => (await groupOf('same-git')) != null, 'the git panel on the board', 60000);
    check(await groupOf('same-git') === gb, 'the git panel stands under srv2\x27s folder, not srv\x27s of the same path');
    // Put out of sight: srv's folder goes, and srv2's of the same path stays
    const key1 = gs.find((g) => g.host === 'srv').key;
    const shown = (name) => board.run(`(S.tabs || []).some(t => t.name === ${JSON.stringify(name)})`);
    await board.run(`send({kind:"folderhide", folder:${JSON.stringify(key1)}, hide:true}); true`);
    await until(async () => !(await shown('same-a')), 'srv\x27s folder out of sight', 30000);
    check(await shown('same-b') && await shown('same-git'), 'putting srv\x27s folder away leaves srv2\x27s of the same path drawn');
    await board.run('send({kind:"folderhide", folder:"", hide:false}); true');
    await until(() => shown('same-a'), 'srv\x27s folder back', 30000);
    // Taking srv2's off the list leaves srv's
    const key2 = gs.find((g) => g.host === 'srv2').key;
    // Its settings open on it, not on the other machine's folder of that path
    await board.run(`openSettings(null, true, ${JSON.stringify(key2)}); true`);
    let samePage;
    await until(async () => {
      const p = portOf(path.join('profiles', 'default'));
      if (!p) return false;
      samePage = (await targetsOf(p)).find((t) => t.type === 'page' && /folder=%01srv2%01/.test(t.url));
      return !!samePage;
    }, 'the settings on srv2\x27s folder', 30000);
    const sp = await connect(samePage, 'srv2\x27s folder\x27s settings');
    try {
      await until(() => sp.run('typeof sel === "object" && sel.grp != null && !!desks[sel.desk]'), 'the folder chosen on the page', 30000);
      check(await sp.run('(desks[sel.desk].folders[sel.grp] || {}).host') === 'srv2', 'the settings open on srv2\x27s folder, not srv\x27s of the same path');
    } finally {
      try { sp.ws.close(); } catch {}
    }
    await board.run('send({kind:"closesettings"}); true');
    // Once its shell has settled: a folder with a tab at work is not taken off
    const stateOf = (name) => board.run(`((S.tabs || []).find(t => t.name === ${JSON.stringify(name)}) || {}).state || ""`);
    await until(async () => !/BUSY|QUESTION/.test(await stateOf('same-b')), 'srv2\x27s shell settled', 60000);
    await board.run(`send({kind:"folderclose", folder:${JSON.stringify(key2)}}); true`);
    await until(() => !(desk().folders || []).some((f) => f.cwd === SAME && f.host === 'srv2'), 'srv2\x27s folder off the list', 60000)
      .catch(async (e) => { console.log('    (the board says: ' + await board.run('S.flash || ""') + '; same-b is ' + await stateOf('same-b') + ')'); throw e; });
    check((desk().folders || []).some((f) => f.cwd === SAME && f.host === 'srv'), 'and srv\x27s folder of the same path stays');
    await until(async () => (await sameGroups()).length === 1, 'one folder of that path left on the board', 60000);
    check((await sameGroups())[0].host === 'srv', 'the one left is srv\x27s');
  } finally {
    await there(`rm -rf ${SAME}`);
  }
} catch (e) {
  check(false, e.message);
} finally {
  clearInterval(trustFirst);
  relay.close();
  try { board.ws.close(); } catch {}
  try { cfg && cfg.ws.close(); } catch {}
  stopApp();
  await cleanThere();
  if (!REAL_AI) await there('rm -f ~/.local/bin/codex' + (madeBin ? '; rmdir ~/.local/bin 2>/dev/null; true' : ''));
  console.log('  what it made on the server is removed: ' + await there(`cd ${REPO} && git worktree list | wc -l && git branch --list 'check/*' | wc -l`).then((s) => s.replace(/\s+/g, ' ')));
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);
