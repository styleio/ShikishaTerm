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
// What the checkout's machine is prepared with, besides its AI
const SETUP = 'sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq php-cli';
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

// The private settings live beside the main checkout, whichever worktree runs this
const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
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
  // A press the way a hand makes one: the page's own handlers, and the
  // rows it builds again under the pointer, see it the way they see a person
  const press = async (x, y, button = 'left') => {
    for (const type of ['mousePressed', 'mouseReleased']) await send('Input.dispatchMouseEvent', { type, x, y, button, clickCount: 1 });
  };
  return { ws, run, shot, press };
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
  // The account is chosen on screen: the one for the address's owner first
  // and chosen, this PC's git last
  const acctPick = '[...document.querySelectorAll("#addproj select")].find(s => [...s.options].some(o => o.value === "@pc"))';
  check(await board.run(`!!${acctPick} && ${acctPick}.value === "check" && [...${acctPick}.options].map(o => o.value).join(",") === "check,@pc"`),
    'the account is chosen on screen, the one for the owner first: ' + await board.run(`${acctPick} ? [...${acctPick}.options].map(o => o.value).join(",") : "no picker"`));
  check(await board.run('!document.querySelector("#addproj .bsignin .warn")'), 'a fine-grained token is not warned about');
  // The AI its machine is given, chosen here and written in from the start
  const aiPick = '[...document.querySelectorAll("#addproj select")].find(s => [...s.options].some(o => o.value === "claude"))';
  check(await board.run(`!!${aiPick} && ${aiPick}.value === "claude"`), 'the AI to install is chosen from the start: Claude Code');
  if (!(await board.run(`!!${aiPick}`))) console.log('    (the board has: ' + JSON.stringify(await board.run('({ais: S.machine_ais, assistant: S.assistant, selects: [...document.querySelectorAll("#addproj select")].map(s => s.outerHTML.slice(0, 200))})')) + ')');
  // Who can open what it serves is chosen before it is made, never left to
  // a default: pressed without a choice, it is asked for; what private gives
  // up is said only once private is chosen
  check(await board.run('!!document.querySelector("#addproj .vmvis") && ![...document.querySelectorAll("#addproj .vmvis input")].some(i => i.checked)'),
    'who can open it is asked, with nothing chosen');
  check(await board.run('document.querySelector("#addproj .vmvis .vmloses").hidden'), 'what private gives up is not said before it is chosen');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  check(await board.run('!document.getElementById("addproj").hidden && !(S.making || []).some(m => m.name === "Hello-World")'),
    'pressed without choosing, nothing is made');
  await board.run('(() => { const r = document.querySelectorAll("#addproj .vmvis input")[1]; r.checked = true; r.dispatchEvent(new Event("change")); })(); true');
  check(await board.run('!document.querySelector("#addproj .vmvis .vmloses").hidden'), 'chosen private, what it gives up is said');
  // This run's machines are unlisted: 4b opens what one serves from anywhere
  await board.run('(() => { const r = document.querySelectorAll("#addproj .vmvis input")[0]; r.checked = true; r.dispatchEvent(new Event("change")); })(); true');
  check(await board.run('document.querySelector("#addproj .vmvis .vmloses").hidden'), 'chosen unlisted, it is not said');
  await board.shot('1-clone');

  console.log('2. the checkout, on a machine of its own, as a row on the board');
  await board.run('document.querySelector("#addproj .apfoot .go").click(); true');
  // The dialog closes, and a row says how far the machine has got
  await until(() => board.run('document.getElementById("addproj").hidden'), 'the dialog to close', 30000);
  const rowStage = () => board.run('((S.making || []).find(m => m.name === "Hello-World" && /^vm_/.test(m.stage)) || {}).stage || ""');
  await until(async () => /^vm_/.test(await rowStage()), 'a row for the machine', 30000);
  check(true, 'the dialog closed and the board has a row for it: ' + await rowStage());
  const rowText = await board.run('[...document.querySelectorAll(".making")].map(r => r.textContent).join(" | ")');
  check(/MicroVM/.test(rowText), 'the row says what it is doing: ' + rowText.slice(0, 80));
  await board.shot('1b-row');
  await until(async () => (await rowStage()) === 'vm_preparing', 'the row to say the AI is being installed', 240000);
  check(true, 'the row says the AI and the machine setup are being installed');
  // Written in a few strokes -- the home, the account, the AI, the folder --
  // and read only once all of them are there, not between two of them
  await until(() => (project()?.homes || []).some((h) => h.sandbox) && project().git_account && project().machine_ai
      && !!folderAt(CHECKOUT) && !!folderAt(CHECKOUT).sandbox, 'the checkout written down as the project\'s', 240000)
    .catch(async (e) => { console.log('    (the board says: ' + JSON.stringify(await board.run('S.making')) + ')'); throw e; });
  const home = project().homes[0];
  check(home.host === vm.name && home.at === CHECKOUT, 'the checkout is where it says: ' + JSON.stringify(home));
  check(home.private === false && !project().microvm_private, 'the machine is written down as unlisted, as chosen, on the checkout and not the project');
  check(project().git_account === 'check', 'the account it signed in as is the project\'s own');
  check(!!folderAt(CHECKOUT) && folderAt(CHECKOUT).sandbox === home.sandbox && folderAt(CHECKOUT).project === PROJECT,
    'the checkout has a folder of its own on the desk, on its machine');
  check(project().machine_ai === 'claude' && home.prepared === 'ai: claude\n', 'the AI is the project\'s, and the machine is written down as having it');
  // Asked inside a machine, by id
  const on = async (id, cmd) => {
    const { Sandbox } = await import(pathToSdk());
    const box = await Sandbox.connect(id, { apiKey: KEY });
    const r = await box.commands.run(cmd, { timeoutMs: 120000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
    return (r.stdout + r.stderr).trim();
  };
  const claudeThere = await on(home.sandbox, 'claude --version 2>&1 | head -1');
  check(/Claude Code/.test(claudeThere), 'Claude Code is installed on the checkout\'s machine: ' + claudeThere);

  console.log('2c. the sign-in step, before anything is copied from the checkout');
  // The machine says its Claude is not signed in, so the step is shown: the
  // checkout's own Claude tab in front and mirrored inside, the ask in so
  // many words, and a way on either way
  const step = () => board.run('JSON.stringify((S && S.login_step) || null)').then((t) => JSON.parse(t || 'null'));
  await until(async () => ((await step()) || {}).state === 'no', 'the step to say Claude is not signed in on the checkout', 90000);
  await until(() => board.run('!document.getElementById("login").hidden'), 'the sign-in step on the board', 30000);
  check((await step()).name === 'Claude Code' && (await step()).folder === CHECKOUT, 'the step is about the checkout\'s Claude: ' + JSON.stringify(await step()));
  await until(() => board.run('(() => { const t = (S.tabs || []).find(x => x.index === S.active); return !!t && t.name === "claude" && (S.groups || [])[t.group] && (S.groups || [])[t.group].folder === ' + JSON.stringify(CHECKOUT) + '; })()'),
    'the checkout\'s Claude tab in front', 30000);
  await until(() => board.run('/Claude/.test((document.querySelector("#login .lmirror") || {textContent:""}).textContent)'), 'the checkout\'s Claude mirrored in the step', 90000);
  check(await board.run('document.querySelector("#login .lstrong").textContent.includes("ログインしてください")'), 'the ask is said, and said first');
  check(await board.run('!document.getElementById("loginnext").disabled'), 'and the way on is open without it');
  // Claude's own sign-in, walked to the address it prints: keys sent to the
  // terminal in the step, and the address picked up beside it for the help
  const screenText = () => board.run('(document.querySelector("#login .lmirror") || {textContent:""}).textContent');
  for (let i = 0; i < 4 && !/^https:\/\//.test(((await step()) || {}).url || ''); i++) {
    await board.run('send({kind:"key", named:"enter"}); true');
    await sleep(5000);
    console.log('    (after Enter ' + (i + 1) + ': ' + (await screenText()).replace(/\s+/g, ' ').trim().slice(-200) + ')');
  }
  // The whole of it: the address is longer than a row, and ends at the
  // blank before "Paste code here"
  await until(async () => /^https:\/\/claude\.(ai|com)\/.*[?&]state=[A-Za-z0-9_-]+$/.test(((await step()) || {}).url || ''), 'the sign-in address Claude printed, picked up whole for the help', 30000)
    .catch(async (e) => {
      const s = await step();
      const text = (await screenText());
      fs.writeFileSync(path.join(SHOTS, 'microvm-2c-screen.txt'), text);
      console.log('    (the step says url=' + JSON.stringify((s || {}).url || '') + '; the terminal text has https at ' + text.indexOf('https') + ', ' + text.length + ' chars, saved beside the shots)');
      throw e;
    });
  check(await board.run('!document.querySelector("#login .lurl").hidden && /^https:\\/\\/claude\\.(ai|com)\\//.test(document.querySelector("#login .lurl").getAttribute("href"))'),
    'the address is beside the terminal, to copy or open: ' + (await step()).url.slice(0, 60) + '…');
  check(/Paste code here/i.test(await screenText()), 'Claude asks for the code in the terminal');
  // A code the length and shape of a real one, so the send is judged as a
  // real one is: not by the text landing in the terminal, but by Claude
  // taking it as sent -- which it says by refusing it
  const fakeCode = 'x'.repeat(64) + '#' + 'y'.repeat(32);
  await board.run(`(() => { const c = document.getElementById("logincode"); c.value = ${JSON.stringify(fakeCode)}; document.getElementById("logincodesend").click(); return true; })()`);
  await until(async () => /OAuth error|invalid code/i.test(await screenText()), 'Claude to take the code as sent (and refuse it)', 45000)
    .catch(async (e) => { console.log('    (the terminal says: ' + (await screenText()).replace(/\s+/g, ' ').trim().slice(-300) + ')'); throw e; });
  check(true, 'the help sends the code and the Enter that sends it');
  await board.shot('2c-login');
  // Signed in from outside, as a key given by the machine setup would be:
  // the credentials, and Claude's own word that its first run is over --
  // what the app asks before it calls Claude signed in there
  const signInFromOutside = () => on(home.sandbox, [
    `mkdir -p ~/.claude && echo '{}' > ~/.claude/.credentials.json;`,
    `f=~/.claude.json;`,
    `if [ ! -s "$f" ]; then echo '{"hasCompletedOnboarding": true}' > "$f";`,
    `elif grep -q hasCompletedOnboarding "$f"; then sed -i 's/"hasCompletedOnboarding" *: *false/"hasCompletedOnboarding": true/' "$f";`,
    `else sed -i '0,/{/s//{"hasCompletedOnboarding": true,/' "$f"; fi`,
  ].join(' '));
  await signInFromOutside();
  await until(async () => ((await step()) || {}).state === 'yes', 'the sign-in to be seen while the step is open', 40000);
  check(await board.run('document.querySelector("#login .lstate").textContent.includes("ログインを確認しました")'), 'the step says the sign-in was seen');
  await on(home.sandbox, 'rm -f ~/.claude/.credentials.json');
  await board.run('document.getElementById("loginnext").click(); true');
  await until(() => board.run('document.getElementById("login").hidden'), 'the step put away', 30000);

  console.log('3. through its rules, to its first worktree');
  cfg = await connect(await settingsOn(/section=project-first/), 'the settings');
  await until(() => cfg.run('!!framed && framed.kind === "rules" && !!document.getElementById("rulesgo")'), 'the rules, as a dialog', 30000);
  const far = await cfg.run('document.querySelector("#floatbody").textContent');
  check(far.includes(vm.name + ' でのワークツリーの配置先') && far.includes('元のフォルダの隣（ワークツリーごとに別のマシン）'), 'the rules say where worktrees go on the MicroVM');
  check(far.includes('MicroVM のセットアップ') && far.includes('Claude Code'), 'the rules say what its MicroVM is prepared with');
  // The machine setup, written the way the AI would have proposed it
  await cfg.run('document.querySelector("[data-rules=microvm]").click(); true');
  await until(() => cfg.run('!!document.querySelector("#floatbody .rulesedit textarea")'), 'the machine setup opened for writing');
  await cfg.run(`(() => { const t = document.querySelector("#floatbody .rulesedit textarea"); t.value = ${JSON.stringify(SETUP)}; t.dispatchEvent(new Event("input")); t.dispatchEvent(new Event("change")); return true; })()`);
  await until(() => cfg.run('document.querySelector("#floatbody").textContent.includes("まだ入っていません")'), 'the line to say the machine does not have it yet');
  check(true, 'written, the line says the checkout\'s machine does not have it yet');
  // One press, named for what it does: the page has no "run on the MicroVM"
  // of its own beside the way on
  await until(() => cfg.run('document.getElementById("rulesgo").textContent === "保存して MicroVM に入れて次へ"'), 'the way on to say it sets up the MicroVM');
  check(!(await cfg.run('document.querySelector("#floatbody").textContent')).includes('保存して MicroVM で実行'),
    'no second button: the way on is the one press that sets up the MicroVM');
  await cfg.shot('2-rules');
  await cfg.run('document.getElementById("rulesgo").click(); true');
  // "Next" hands the checkout's machine to the app to prepare, closes, and
  // the board shows a row under the project until it is
  await until(() => board.run('!S.settings_open'), 'the settings to close', 30000);
  const prepStage = () => board.run(`((S.making || []).find(m => m.folder === ${JSON.stringify(CHECKOUT)} && m.name === ${JSON.stringify(vm.name)}) || {}).stage || ""`);
  await until(async () => (await prepStage()) === 'vm_preparing', 'a row for the machine being prepared', 30000);
  check(await board.run(`(S.groups || []).some(g => g.family && sameFolder(g.folder, ${JSON.stringify(CHECKOUT)}))`), 'the row stands under the project');
  await board.shot('2b-preparing');
  await until(() => (project()?.homes || [])[0]?.prepared === `ai: claude\n${SETUP}\n`, 'the checkout prepared as written', 600000)
    .catch(async (e) => { console.log('    (the board says: ' + JSON.stringify(await board.run('S.making')) + ')'); throw e; });
  const phpThere = await on(home.sandbox, 'php --version 2>&1 | head -1');
  check(/^PHP \d/.test(phpThere), 'the machine setup ran on the checkout\'s machine: ' + phpThere);
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
  // Whether the checkout's AI is signed in, asked of the checkout's machine:
  // not yet, said with the checkout's tab one press away; then, once a
  // sign-in is there, seen without closing the dialog
  const aiNote = () => board.run('JSON.stringify((S.branch && S.branch.ai_sign_in) || null)').then((t) => JSON.parse(t || 'null'));
  await until(async () => ((await aiNote()) || {}).state === 'no', 'the dialog to say Claude is not signed in on the checkout', 90000);
  check((await aiNote()).name === 'Claude Code' && (await aiNote()).checkout === CHECKOUT, 'not signed in yet, said for the checkout: ' + JSON.stringify(await aiNote()));
  check(await board.run('!!document.querySelector("#branch .baisignin button")'), 'the checkout\'s tab is one press away');
  await board.shot('3-branch');
  await signInFromOutside();
  await until(async () => ((await aiNote()) || {}).state === 'yes', 'the sign-in to be seen while the dialog stays open', 120000);
  check(true, 'signed in on the checkout, the dialog says so without being closed');
  await on(home.sandbox, 'rm -f ~/.claude/.credentials.json');
  await board.run('document.querySelector("#branch .bgo .go").click(); true');
  const wt = await (async () => {
    let f = null;
    await until(() => { f = (desk().folders || []).find((x) => x.host === vm.name && x.cwd !== CHECKOUT && x.sandbox); return !!f; },
      'the worktree written down, on its own machine', 240000)
      .catch(async (e) => {
        console.log('    (the board has: ' + await board.run('JSON.stringify({flash: S.flash, making: S.making, branch: S.branch && {asked: S.branch.asked, folder: S.branch.folder, error: S.branch.error, in_use: S.branch.in_use}})') + ')');
        throw e;
      });
    return f;
  })();
  check(wt.sandbox !== home.sandbox && wt.project === PROJECT, 'on a machine of its own, in the project: ' + JSON.stringify(wt));
  // The worktree opens on the AI its machine was given, the way a worktree
  // here opens on what its original runs: one tab, Claude's, and Claude
  // running in it (its first screen asks, so the tab reads as a question)
  check((wt.tabs || []).length === 1 && wt.tabs[0].command === 'claude', 'the worktree opens on the AI: ' + JSON.stringify(wt.tabs));
  const aiTab = () => board.run(`JSON.stringify((S.tabs || []).filter(t => t.name === "claude").slice(-1)[0] || null)`).then((t) => JSON.parse(t || 'null'));
  await until(async () => !!(await aiTab()), 'the worktree\'s AI tab on the board', 30000);
  await until(async () => ['QUESTION', 'BUSY'].includes(((await aiTab()) || {}).state), 'Claude to be running in it', 120000);
  check((await aiTab()).profile === 'Claude Code', 'the tab is read as Claude\'s: ' + JSON.stringify(await aiTab()));

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
  const has = await inside('claude --version 2>&1 | head -1; php --version 2>&1 | head -1');
  check(/Claude Code/.test(has) && /PHP \d/.test(has), 'the worktree has what its checkout was prepared with: ' + has.replace(/\n/g, ' | '));

  console.log('4a. the machine paused under the terminal wakes when it is typed into');
  const appLog = () => fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8');
  const awakeCount = () => (appLog().match(new RegExp('e2b: ' + wt.sandbox + ' is awake', 'g')) || []).length;
  // A terminal on a MicroVM opens when its tab is first in front: until then
  // there is no link to end, and nothing of the machine is started. So the
  // worktree's Claude is put in front first, and its terminal opened
  const aiFirst = await aiTab();
  await board.run(`send({kind:"select", tab: ${aiFirst.index}}); true`);
  await until(() => board.run(`S.active === ${aiFirst.index}`), 'the worktree\'s Claude tab in front to open it', 30000);
  await until(async () => awakeCount() > 0, 'the worktree\'s terminal to open as its tab is shown', 90000);
  await until(async () => /claude/i.test(await board.run('document.getElementById("screen").textContent')),
    'Claude to start in the worktree\'s terminal', 90000);
  // Settled before the machine is paused, as one is after its minutes: a
  // Claude paused while it is still starting times its first call to its
  // servers out when it wakes, and ends -- which is Claude, not the pause
  let lastScreen = '', sameSince = Date.now();
  await until(async () => {
    const now = await board.run('document.getElementById("screen").textContent');
    if (now !== lastScreen) { lastScreen = now; sameSince = Date.now(); }
    return Date.now() - sameSince > 8000;
  }, 'Claude to settle on a screen', 120000);
  // Which Claude is running there, to know it is the same one afterwards
  const claudePid = async () => (await on(wt.sandbox, 'pgrep -x claude | head -n 1')).trim();
  const pidBefore = await claudePid();
  check(/^\d+$/.test(pidBefore), 'Claude is running on the worktree\'s machine: ' + pidBefore);
  const openedTimes = awakeCount();
  // Paused from outside, as it is after its minutes; the terminal says so.
  // A key typed into it wakes the machine, and Claude in it goes on
  const paused = await e2b('POST', `/sandboxes/${wt.sandbox}/pause`);
  check(paused.status < 300, 'the worktree\'s machine is paused from outside');
  await until(async () => ((await ours()).find((s) => s.sandboxID === wt.sandbox) || {}).state === 'paused', 'the service to say it is paused', 60000);
  const aiNow = await aiTab();
  await board.run(`send({kind:"select", tab: ${aiNow.index}}); true`);
  // In front before anything is typed: a key sent on the heels of the
  // selection would land in the tab that was in front a moment ago
  await until(() => board.run(`S.active === ${aiNow.index}`), 'the worktree\'s Claude tab in front', 30000);
  const screen = () => board.run('document.getElementById("screen").textContent');
  await until(async () => /接続が切れました/.test(await screen()), 'the terminal to say its link ended', 60000);
  // A key typed into the paused machine's terminal wakes the machine (the
  // app says so in its log). One that answers none of Claude's questions --
  // Enter chose whatever its screen had in front, "No, exit" included, and
  // Claude ended by being answered, not by the pause
  await board.run('send({kind:"key", named:"right"}); true');
  // Once more than when it was first opened: this one is the key's
  await until(async () => awakeCount() > openedTimes, 'the app to wake the machine for the key', 90000)
    .catch((e) => { console.log('    (the app says: ' + appLog().split(/\r?\n/).filter((l) => /e2b/.test(l)).slice(-4).join(' | ') + ')'); throw e; });
  check(!/could not wake/.test(appLog()), 'the same shell is taken up again, not refused');
  // The same Claude, still running, and its screen drawn again over the
  // notice: nothing was started over
  await until(async () => (await claudePid()) === pidBefore, 'Claude, the same process, still running after the pause', 90000)
    .catch(async (e) => { console.log('    (claude there now: ' + JSON.stringify(await claudePid()) + '; the terminal says: ' + (await screen()).replace(/\s+/g, ' ').trim().slice(-300) + ')'); throw e; });
  await until(async () => !/\$\s*$/.test((await screen()).trimEnd()) && /MicroVM を再開しました/.test(await screen()), 'Claude\'s screen, not a shell, after the notice', 60000)
    .catch(async (e) => { console.log('    (the terminal says: ' + (await screen()).replace(/\s+/g, ' ').trim().slice(-300) + ')'); throw e; });
  check(true, 'Claude in the terminal went on after the pause, as it was');
  await until(async () => ((await ours()).find((s) => s.sandboxID === wt.sandbox) || {}).state === 'running', 'the service to say it is running again', 60000);

  console.log('4d. a file attached from the input bar lands on the machine, in the folder there');
  // A one-pixel picture, attached the way the bar attaches: the path handed
  // back is the machine's, and the file is there under it
  const pixel = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';
  const attached = await board.run(`attachViaIpc("pixel.png", ${JSON.stringify(pixel)})`);
  check(attached && attached.ok && attached.path && attached.path.startsWith(wt.cwd + '/.SHIKISHA/tmp/') && attached.path.endsWith('.png'),
    'the path is the machine\'s own: ' + JSON.stringify(attached));
  const there = await inside(`test -f ${JSON.stringify(attached.path)} && wc -c < ${JSON.stringify(attached.path)}; cat ${wt.cwd}/.SHIKISHA/.gitignore`);
  check(/^\s*70\s*\*\s*$/.test(there.replace(/\r/g, '')), 'the file is on the machine, beside an ignore of its own: ' + JSON.stringify(there));

  console.log('4c. the git panel reports on the worktree, with git run over there');
  // Asked the way the column beside the tab asks -- the tab named, the act,
  // its arguments -- and answered from the machine's own git in that folder
  const ai = await aiTab();
  const gitSend = (act, args) => board.run(`send({kind:"git", panel: ${JSON.stringify(ai.id)}, act: ${JSON.stringify(act)}, args: ${JSON.stringify(args || {})}}); true`);
  await gitSend('branch');
  await until(() => board.run('JSON.stringify(G.branch)').then((b) => (JSON.parse(b || 'null') || {}).name === BRANCH), 'the panel to say the branch, read over there', 60000);
  check(true, 'the panel reports on the worktree\'s machine: ' + await board.run('JSON.stringify(G.branch)'));
  // Made with no base named, it grows from the server's default, and
  // bringing its latest in fetches the branch that default is
  const cut = await board.run('JSON.stringify(G.branch)').then((b) => JSON.parse(b || 'null') || {});
  check((cut.catch_up || [])[0] === 'git fetch origin +refs/heads/master:refs/remotes/origin/master',
    'catching up fetches the branch the server\'s default is, not one called HEAD: ' + JSON.stringify({base: cut.base, catch_up: cut.catch_up}));
  await inside(`cd ${wt.cwd} && echo "from the panel" > panel.txt`);
  await gitSend('status');
  await until(() => board.run('(G.rows || []).some(r => r.path === "panel.txt")'), 'the new file in the panel\'s list', 60000);
  await gitSend('stage', { paths: ['panel.txt'] });
  await gitSend('status');
  await until(() => board.run('(G.rows || []).some(r => r.path === "panel.txt" && r.staged)'), 'staged from the panel', 60000);
  await gitSend('commit', { text: 'panel: committed over there' });
  await until(async () => (await inside(`git -C ${wt.cwd} log -1 --format=%s`)) === 'panel: committed over there', 'the commit, made on the machine', 60000);
  const author = await inside(`git -C ${wt.cwd} log -1 --format='%an <%ae>'`);
  check(/@users\.noreply\.github\.com>$/.test(author) && !/x-access-token/.test(author), 'by the account, as GitHub knows it: ' + author);

  console.log('4e. a branch the server has is offered a pull request, with where it goes');
  // The repository here cannot be pushed to, so the machine is told the
  // server has the branch as it is, and follows it: what a push leaves
  // behind. Put back afterwards, so step 5 still finds the commit unpushed
  await inside(`cd ${wt.cwd} && git update-ref refs/remotes/origin/${BRANCH} HEAD && git branch -q -u origin/${BRANCH}`);
  const prSaid = () => board.run(`JSON.stringify({github: gitOnGithub(), place: (gitTab() || {}).place || null, branch: G.branch, prs: G.prs, why: G.prsWhy, shown: gitPrFormShown(), pr: I.pr && {from: I.pr.from, project: I.pr.project, head: I.pr.head, bases: I.pr.bases, base: I.pr.base}})`);
  await gitSend('branch');
  await until(() => board.run('!!(G.branch && G.branch.upstream)'), 'the panel to see the branch follow the server', 60000)
    .catch(async (e) => { console.log('    (the panel has: ' + await prSaid() + ')'); throw e; });
  await until(async () => {
    await board.run('gitAskPrs(); gitPrFormFit(); true');
    return board.run('!!(I.pr && I.pr.from === "git" && Array.isArray(I.pr.bases))');
  }, 'the pull request form and where it can go', 90000)
    .catch(async (e) => { console.log('    (the panel has: ' + await prSaid() + ')'); throw e; });
  const bases = await board.run('I.pr.bases');
  check(bases.includes('master') && !bases.includes(BRANCH), 'where it goes is offered, from what the machine knows the server has: ' + JSON.stringify(bases));
  await inside(`cd ${wt.cwd} && git branch -q --unset-upstream && git update-ref -d refs/remotes/origin/${BRANCH}`);

  console.log('4f. a worktree asked for from a worktree: grown from the checkout\'s machine, and one already made is in use');
  // A new worktree is a copy of the checkout's machine, whichever folder the
  // dialog opens from: what it offers to grow from is what that machine has,
  // and not the branch only this worktree's machine has
  const wg = `(S.groups || []).find(x => x.folder === ${JSON.stringify(wt.cwd)})`;
  await board.run(`openBranch(${wg}); true`);
  await until(() => board.run('!!(S.branch && Array.isArray(S.branch.bases) && S.branch.bases.includes("origin/master"))'), 'what it can grow from', 60000)
    .catch(async (e) => { console.log('    (the dialog has: ' + await board.run('JSON.stringify(S.branch && S.branch.bases)') + ')'); throw e; });
  const wbases = await board.run('S.branch.bases');
  check(!wbases.includes(BRANCH), 'grown from what the checkout\'s machine has, not from the branch only this worktree has: ' + JSON.stringify(wbases));
  // The same branch again: git on a new copy cannot see it, the board can
  await board.run(`branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await board.run(`(() => { const q = document.getElementById("bq"); q.value = ${JSON.stringify(BRANCH)}; q.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => board.run(`!!(S.branch && S.branch.in_use)`), 'the branch to be said in use', 60000)
    .catch(async (e) => { console.log('    (the dialog has: ' + await board.run('JSON.stringify({asked: S.branch && S.branch.asked, folder: S.branch && S.branch.folder, error: S.branch && S.branch.error})') + ')'); throw e; });
  check(await board.run('S.branch.in_use.folder') === wt.cwd, 'a branch already on a MicroVM is in use, in that folder: ' + await board.run('JSON.stringify(S.branch.in_use)'));
  await board.run('closeBranch(); true');
  // The settings tell the worktree from the checkout, and say it is one
  await board.run('openSettings("gitaccounts", true); true');
  cfg = await connect(await settingsOn(/section=gitaccounts/), 'the settings');
  await until(() => cfg.run('typeof TOKEN === "string" && !!TOKEN'), 'the settings page, read in', 30000);
  const familyOf = (p) => cfg.run(`fetch("/api/family?path=" + encodeURIComponent(${JSON.stringify(p)}), {headers:{"X-Token":TOKEN}}).then(r => r.json())`);
  const wfam = await familyOf(wt.cwd);
  const hfam = await familyOf(CHECKOUT);
  check(wfam.cut === true && !!wfam.host && hfam.cut === false, 'the settings call the worktree a worktree and the checkout the checkout: ' + JSON.stringify({ worktree: wfam.cut, checkout: hfam.cut }));
  try { cfg.ws.close(); } catch {}
  cfg = null;
  await board.run('send({kind:"closesettings"}); true');

  console.log('4g. work in a worktree does not start the checkout\'s machine');
  // The checkout's machine paused, as it is after its minutes. Asking GitHub
  // about the worktree's branch -- its pull requests and CI, what the panel
  // asks every few seconds while CI runs -- and looking for the folder a
  // pull request's branch is on are answered without it
  const stateOf = async (id) => ((await ours()).find((s) => s.sandboxID === id) || {}).state;
  // Paused once what 4f asked of it is over, as a machine is paused after
  // its minutes untouched: one paused with a call still arriving is started
  // again by the service for that call
  await sleep(15000);
  await e2b('POST', `/sandboxes/${home.sandbox}/pause`);
  await until(async () => (await stateOf(home.sandbox)) === 'paused', 'the checkout\'s machine paused', 60000);
  await board.run(`send({kind:"select", tab: ${ai.index}}); true`);
  for (let i = 0; i < 3; i++) {
    await board.run('gitAskPrs(); true');
    await board.run(`gitIssuesAsk("pr_place", {project: ${JSON.stringify(project().name)}, number: 1, head: ${JSON.stringify(BRANCH)}, base: "master"}); true`);
    await sleep(8000);
  }
  check((await stateOf(home.sandbox)) === 'paused', 'the checkout\'s machine stays paused: ' + await stateOf(home.sandbox));

  console.log('4b. what it serves answers from anywhere, at the address the menu lists');
  await inside(`cd ${wt.cwd} && printf '<?php echo "served-from-the-" . "worktree";' > index.php && (nohup php -S 0.0.0.0:8000 >/dev/null 2>&1 &) ; sleep 1; echo ok`);
  const g = `(S.groups || []).find(x => x.folder === ${JSON.stringify(wt.cwd)})`;
  await until(() => board.run(`!!${g}`), 'the worktree\'s card', 30000);
  check(await board.run(`onMicrovm(${g})`), 'the card knows it is on a MicroVM');
  // Opened as a person opens it: a right-click on the card, then the entry.
  // The card is built again while its terminal is at work, and the list is
  // drawn all the same
  const card = await board.run(`(() => { const r = [...document.querySelectorAll("#tabs .tab.folder")].find(x => (x.querySelector(".nm") || {}).textContent === ${g}.name); if (!r) return null; const b = r.getBoundingClientRect(); return {x: b.left + 30, y: b.top + b.height / 2}; })()`);
  check(!!card, 'the worktree\'s card is on the board');
  await board.press(card.x, card.y, 'right');
  await until(() => board.run('[...document.querySelectorAll(".fmenu div")].some(d => d.textContent === "公開 URL")'), 'the card\'s menu', 10000);
  await sleep(2000);
  const entry = await board.run('(() => { const d = [...document.querySelectorAll(".fmenu div")].find(d => d.textContent === "公開 URL"); const b = d.getBoundingClientRect(); return {x: b.left + b.width / 2, y: b.top + b.height / 2}; })()');
  await board.press(entry.x, entry.y);
  await until(() => board.run(`!!(S.far_ports && !S.far_ports.busy && S.far_ports.folder === gkey(${g}))`), 'the addresses', 60000);
  await until(() => board.run('/8000/.test((document.querySelector(".fmenu.farports") || {textContent: ""}).textContent)'), 'the addresses drawn under the pointer', 10000)
    .catch(async (e) => { console.log('    (the page has: ' + await board.run('JSON.stringify({open: !!farPortsOpen, menus: [...document.querySelectorAll(".fmenu")].map(m => m.textContent)})') + ')'); throw e; });
  const ports = await board.run('S.far_ports.ports');
  const p8000 = ports.find((p) => p.port === 8000);
  check(!!p8000 && p8000.url === `https://8000-${wt.sandbox}.e2b.app`, 'the port and its public URL are listed: ' + JSON.stringify(ports));
  check(await board.run('!!document.querySelector(".fmenu.farports")'), 'and shown where the menu was');
  // Read, then chosen: what the list is heads it and cannot be pressed; a
  // port is one line, with a copy button for its address; what the image
  // itself runs as root (SSH on 22, the port mapper on 111) is not offered
  check(await board.run('(() => { const m = document.querySelector(".fmenu.farports"); const lines = [...m.querySelectorAll(".farport")]; return !!m.querySelector(".farhead") && !m.querySelector(".farhead").onclick && lines.length === ports.length && lines.every(l => !!l.querySelector(".fcopy")); })()'.replace('ports.length', String(ports.length))),
    'a heading that is read, and one line a port with its copy button');
  check(!ports.some((p) => p.port === 22 || p.port === 111), 'the image\'s own ports are not offered: ' + ports.map((p) => p.port).join(', '));
  // Every line the same width, and the list inside the window
  check(await board.run('(() => { const m = document.querySelector(".fmenu.farports"); const b = m.getBoundingClientRect(); const w = [...m.children].map(c => Math.round(c.getBoundingClientRect().width)); return w.every(x => x === w[0]) && b.bottom <= innerHeight - 8; })()'),
    'every line of the list is as wide as the others, and the list is inside the window');
  // How a server is started differs from project to project: the machine's
  // AI is handed it, in a tab of its own in the folder
  await board.run('document.querySelector(".fmenu.farports .farserve").click(); true');
  await until(() => board.run(`(S.tabs || []).some(t => t.name === "サーバー起動" && t.group === (S.groups || []).indexOf(${g}))`), 'a tab for the AI to start the server in', 60000)
    .catch(async (e) => { console.log('    (the board says: ' + await board.run('S.flash || ""') + ')'); throw e; });
  check(true, 'the AI is handed starting the server, in a tab of its own in the folder');
  await board.run('(() => { const t = (S.tabs || []).find(t => t.name === "サーバー起動"); if (t) send({kind:"closetab", tab:t.index, key:t.key || "", sure:true}); return true; })()');
  await until(() => board.run('!(S.tabs || []).some(t => t.name === "サーバー起動")'), 'that tab closed', 30000);
  await board.run(`openFarPorts(${g}, {clientX: 200, clientY: 200}); true`);
  await board.shot('3b-urls');
  if (p8000) {
    const got = await fetch(p8000.url).then((r) => r.text()).catch((e) => String(e));
    check(got.trim() === 'served-from-the-worktree', 'the URL answers from anywhere: ' + got.trim().slice(0, 60));
  }
  await board.run('closeFolderMenu(); true');

  console.log('5a. the machines, from the settings: preparing asked twice, a folder taken off the list, and its machine');
  await board.run('openSettings("gitaccounts", true); true');
  cfg = await connect(await settingsOn(/section=gitaccounts/), 'the settings');
  await until(() => cfg.run('typeof TOKEN === "string" && !!TOKEN'), 'the settings page, read in', 30000);
  const api = (p, body) => cfg.run(`fetch(${JSON.stringify(p)}, {method:"POST", headers:{"X-Token":TOKEN}, body:${JSON.stringify(JSON.stringify(body))}}).then(r => r.json())`);
  const machines = () => cfg.run('fetch("/api/microvm/machines", {headers:{"X-Token":TOKEN}}).then(r => r.json())');
  // Its setup, for this step, is one line that takes a while: what runs
  // while it is asked again, and what its row says it is doing
  const setupTo = (line) => {
    const c = saved();
    c.desks[0].projects.find((p) => p.name === PROJECT).machine_setup = line;
    fs.writeFileSync(CONFIG, JSON.stringify(c, null, 2));
  };
  setupTo('echo shk-slow && sleep 60');
  await until(() => (project().machine_setup || '') === 'echo shk-slow && sleep 60', 'the setup saved', 10000);
  // Asked once: a row for each machine of the project, each saying the
  // command it is on, which of how many, and for how long
  const prepare = { desk: desk().id, project: PROJECT };
  const first = await api('/api/project/machine-setup', prepare);
  check(first.ok && first.machines === 2, 'preparing starts on the checkout\'s machine and the worktree\'s: ' + JSON.stringify(first));
  const preparingRows = () => board.run('(S.making || []).filter(m => m.stage === "vm_preparing").length');
  await until(async () => (await preparingRows()) === 2, 'a row for each machine', 60000);
  await until(() => board.run('[...document.querySelectorAll(".making .mstep")].some(r => /手順 \\d+\\/\\d+：.+・\\d+:\\d\\d/.test(r.textContent))'), 'the row to say the command it is on', 30000)
    .catch(async (e) => { console.log('    (the rows say: ' + await board.run('[...document.querySelectorAll(".making")].map(r => r.textContent).join(" | ")') + ')'); throw e; });
  check(true, 'the row says the command it is on, and for how long: ' + await board.run('document.querySelector(".making .mstep").textContent'));
  // Asked again while it runs: joined, said as that, and not started twice
  const again = await api('/api/project/machine-setup', prepare);
  check(again.ok && again.already === again.machines, 'a second ask says it is running already: ' + JSON.stringify(again));
  await sleep(3000);
  check((await preparingRows()) <= 2, 'and the install is not started a second time');
  // The setup line, said by its number; its own words only when asked
  const setupRow = '[...document.querySelectorAll(".making .mstep")].find(r => /マシンの設定 1\\/1 行目/.test(r.textContent))';
  await until(() => board.run(`!!${setupRow}`), 'the setup line to run, said by its number', 600000)
    .catch(async (e) => { console.log('    (the rows say: ' + await board.run('[...document.querySelectorAll(".making")].map(r => r.textContent).join(" | ")') + ')'); throw e; });
  check(!(await board.run('[...document.querySelectorAll(".making")].some(r => r.textContent.includes("shk-slow"))')),
    'the row does not show the command as written: ' + await board.run(`${setupRow}.textContent`));
  await board.run(`${setupRow}.parentElement.querySelector(".mline").click(); true`);
  await until(() => board.run('[...document.querySelectorAll(".making .mcmd")].some(c => c.textContent.includes("shk-slow"))'), 'the command shown when asked', 10000);
  check(true, 'and shows it when asked');
  // Asked again with a setup saved since it began: said as running again
  // afterwards, and run once the first is done
  setupTo('echo shk-again');
  await until(() => (project().machine_setup || '') === 'echo shk-again', 'the new setup saved', 10000);
  const changed = await api('/api/project/machine-setup', prepare);
  check(changed.ok && changed.again === changed.machines, 'an ask with settings saved since says it runs again afterwards: ' + JSON.stringify(changed));
  await until(async () => (await preparingRows()) === 0, 'the machines prepared', 600000)
    .catch(async (e) => { console.log('    (the board says: ' + JSON.stringify(await board.run('S.making')) + ')'); throw e; });
  await until(() => (project().homes || []).some((h) => (h.prepared || '').includes('shk-again')), 'the checkout written down as prepared with the setup saved since', 60000)
    .catch(async (e) => { console.log('    (the checkout says it has: ' + JSON.stringify((project().homes || []).map((h) => h.prepared)) + ')'); throw e; });
  check(true, 'the setup saved while it ran is what the checkout is prepared with');
  // Taken off the list: its machine stays, and the list of machines says
  // what the folder was, to put back or to delete -- not "being made"
  // A tab still at something -- an AI waiting on its own question -- keeps a
  // folder on the list, and is closed first, as a person would
  const atWork = `(S.tabs || []).filter(t => ${g} && t.group === (S.groups || []).indexOf(${g}) && /BUSY|QUESTION/.test(t.state || ""))`;
  console.log('    (at work in the worktree: ' + await board.run(`JSON.stringify(${atWork}.map(t => t.name + ":" + t.state))`) + ')');
  await board.run(`${atWork}.forEach(t => send({kind:"closetab", tab:t.index, key:t.key || "", sure:true})); true`);
  await until(async () => (await board.run(`${atWork}.length`)) === 0, 'the worktree\'s tabs at rest', 60000);
  await board.run(`send({kind:"folderclose", folder: gkey(${g})}); true`);
  await until(() => !folderAt(wt.cwd), 'the worktree off the list', 60000)
    .catch(async (e) => { console.log('    (the board says: ' + await board.run('S.flash || ""') + ')'); throw e; });
  const row = async () => ((await machines()).machines || []).find((m) => m.id === wt.sandbox) || null;
  await until(async () => !!(await row()), 'the machine in the list', 60000);
  const r = await row();
  check(r.making === false && r.off_list === wt.cwd && !r.used.length,
    'the machine is one to put back or delete, not one being made: ' + JSON.stringify({ making: r.making, off_list: r.off_list }));
  // What its folder holds is asked before the machine goes: the page 4b
  // served is not committed, so it is not deleted
  const dropped = await api('/api/microvm/drop', { id: wt.sandbox });
  check(!dropped.ok && /削除していません/.test(dropped.error || '') && /コミットしていない/.test(dropped.error || ''),
    'deleting it is refused while its folder has work that is nowhere else: ' + (dropped.error || JSON.stringify(dropped)));
  check((await ours()).some((s) => s.sandboxID === wt.sandbox), 'and the machine is still there');
  const back = await api('/api/microvm/restore', { id: wt.sandbox, desk: desk().name });
  check(back.ok, 'put back on the list: ' + JSON.stringify(back));
  await until(() => !!folderAt(wt.cwd), 'the worktree back on the list', 60000);
  await until(() => board.run(`!!${g}`), 'the worktree\'s card back', 60000);
  try { cfg.ws.close(); } catch {}
  cfg = null;
  await board.run('send({kind:"closesettings"}); true');

  console.log('5. deleting the worktree deletes its machine, and only that');
  // The page 4b served is not committed: work that exists only on that
  // machine, so the machine is not deleted, and the board says why
  await board.run(`discardFolder(${g}); true`);
  await until(() => board.run('S.flash || ""').then((t) => /削除していません/.test(t)), 'the refusal of a machine with work on it', 60000);
  check(/コミットしていないものが 1 件/.test(await board.run('S.flash')) && !!folderAt(wt.cwd) && (await ours()).some((s) => s.sandboxID === wt.sandbox),
    'what is not committed on the machine stops it, and the folder and its machine stay: ' + await board.run('S.flash'));
  // Put away, and asked again: the commit 4c made is on no server either --
  // the machine is all there is of it -- so that stops it too
  await inside(`cd ${wt.cwd} && rm -f index.php`);
  await board.run('S.flash = ""; true');
  await board.run(`discardFolder(${g}); true`);
  await until(() => board.run('S.flash || ""').then((t) => /プッシュしていないコミット/.test(t)), 'the refusal of a commit that is nowhere else', 60000);
  check(!!folderAt(wt.cwd) && (await ours()).some((s) => s.sandboxID === wt.sandbox), 'a commit not pushed stops it as well, and nothing goes');
  // Taken back to where the branch was cut, and asked once more
  await inside(`cd ${wt.cwd} && git reset -q --hard "$(git rev-list HEAD --not --remotes | tail -n 1)~1"`);
  await board.run(`discardFolder(${g}); true`);
  // What the board says on the way, kept to be shown when it does not go
  const said = new Set();
  await until(async () => {
    said.add(await board.run('S.flash || ""'));
    return !(await ours()).some((s) => s.sandboxID === wt.sandbox);
  }, 'the worktree\'s machine to go', 60000)
    .catch(async (e) => {
      console.log('    (the board said: ' + [...said].filter(Boolean).join(' | ') + ')');
      console.log('    (git there says: ' + JSON.stringify(await inside(`cd ${wt.cwd} && git status --porcelain --untracked-files=all`).catch((x) => String(x))) + ')');
      throw e;
    });
  check(!folderAt(wt.cwd), 'the folder is off the desk');
  check((await ours()).some((s) => s.sandboxID === home.sandbox), 'the checkout\'s machine stays');
  await board.shot('4-after');

  console.log('6. adding a git account asks for what nearly everybody fills in, the rest folded');
  await board.run('openSettings("gitaccounts", true); true');
  cfg = await connect(await settingsOn(/section=gitaccounts/), 'the settings');
  await until(() => cfg.run('!![...document.querySelectorAll("button")].find(b => b.textContent === "＋ git アカウントを追加")'), 'the git accounts card', 30000);
  await cfg.run('[...document.querySelectorAll("button")].find(b => b.textContent === "＋ git アカウントを追加").click(); true');
  await until(() => cfg.run('!!document.querySelector(".modal .fold")'), 'the form with its fold');
  const shown = await cfg.run('[...document.querySelectorAll(".modal .mbody > .field:not([hidden]) > label, .modal .mbody > .fold .foldhead")].map(e => e.textContent.trim())');
  check(shown[0] === '名前' && shown.includes('トークン') && shown.includes('対象のオーナー') && !shown.includes('表示名') && !shown.includes('サーバー'),
    'name, token and owners are asked; the rest is under one line: ' + shown.join(' | '));
  check(await cfg.run('document.querySelector(".modal .foldbody").hidden'), 'the fold starts closed');
  await cfg.run('document.querySelector(".modal .foldhead").click(); true');
  check(await cfg.run('!document.querySelector(".modal .foldbody").hidden && [...document.querySelectorAll(".modal .foldbody label")].some(l => l.textContent === "表示名")'), 'opened, the display name and the server are there');
  await cfg.shot('5-account-form');
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
