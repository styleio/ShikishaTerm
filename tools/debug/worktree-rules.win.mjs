/**
 * A project's worktree rules, from the moment it is added to its first
 * worktree, through the running app's own window.
 *
 * A project a server reads where it stands (a site with an .htaccess) has to
 * have its worktrees beside it, and a config file that names the checkout has
 * to be copied with that name rewritten, or the worktree is served pointing
 * back at the checkout. This starts the app built in this checkout, in a
 * folder of its own, with such a project, and walks the way a person does:
 *
 *   1. the board takes a just-added project to its rules (Settings, the
 *      "Worktree Creation Rules" page, with the way on above them)
 *   2. the page says where a worktree would go -- beside the checkout, with
 *      what says the project is served where it stands -- and what each
 *      ignored line holds
 *   3. a proposal like the AI's is shown on "Is this right?", with the lines
 *      of the config file before and after, and saved with "Save and
 *      continue" (the AI itself is not asked: that would spend a turn of
 *      somebody's account, and what it answers is checked by the unit tests)
 *   4. the board opens the worktree dialog on its own, the folder in it is
 *      the one the page said, and pressing it makes that folder with the
 *      config file rewritten and the rest copied
 *
 * Every step is checked against config/config.json and the disk, not only
 * against the pages.
 *
 *     cargo build
 *     node tools/debug/worktree-rules.win.mjs
 *
 * Needs Windows, Node and git. Isolated the way a new-user run is: its own
 * folder, its own LOCALAPPDATA, started from its own folder. The settings are
 * a page in the app's *pages* WebView2 environment, so the copy is started
 * with --remote-debugging-port=0 and each environment's port is read from its
 * DevToolsActivePort file (see quick-actions.win.mjs). Photographs land in
 * target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-rules');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const REPO = path.join(WORK, 'site');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
// Where the page should say a worktree named feature/x goes: beside the
// checkout, at the checkout's depth, named for the checkout and the work
const BESIDE = path.join(WORK, 'site-feature-x');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const same = (a, b) => String(a || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
  === String(b || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, REPO, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const git = (...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd: REPO, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
  return r.stdout;
};
// A site: what the server is pointed at is one folder down, as sites keep it
git('init', '-q', '-b', 'main');
fs.mkdirSync(path.join(REPO, 'webroot'), { recursive: true });
fs.writeFileSync(path.join(REPO, 'webroot', '.htaccess'), 'RewriteEngine On\n');
fs.writeFileSync(path.join(REPO, 'webroot', 'index.php'), '<?php require "../config/local.php";\n');
fs.writeFileSync(path.join(REPO, '.gitignore'), 'config/local.php\nvendor/\n');
git('add', '-A');
git('commit', '-q', '-m', 'start');
// What git does not carry: a config naming the checkout by its folder name,
// the way a server's own path and URL do, and what was installed
fs.mkdirSync(path.join(REPO, 'config'), { recursive: true });
const LOCAL_PHP = [
  '<?php',
  "define('_ROOT_', '/srv/work/site/');",
  "define('_HOME_', 'http://dev.local/work/site/webroot/');",
  "$db_pass = 'hunter2';",
  '',
].join('\n');
fs.writeFileSync(path.join(REPO, 'config', 'local.php'), LOCAL_PHP);
fs.mkdirSync(path.join(REPO, 'vendor', 'lib'), { recursive: true });
fs.writeFileSync(path.join(REPO, 'vendor', 'lib', 'a.php'), '<?php // installed\n');

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: false },
  desks: [{ name: 'Check', id: 'check', folders: [{ cwd: REPO, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
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
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};

/** One DevTools page, spoken to over its socket */
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
    fs.writeFileSync(path.join(SHOTS, `worktree-rules-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  const click = async (sel) => {
    const p = await run(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null;
      e.scrollIntoView({block:'center'}); const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
    if (!p) throw new Error('nothing at ' + sel + ' on ' + name);
    for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
      await send('Input.dispatchMouseEvent', { type, x: p.x, y: p.y, button: 'left', clickCount: 1 });
    }
    await sleep(250);
  };
  return { ws, run, shot, click };
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
let cfg = null;
const saved = () => JSON.parse(fs.readFileSync(CONFIG, 'utf8'));
const project = () => (saved().desks[0].projects || [])[0] || null;

try {
  await until(() => board.run('typeof rulesFirst === "function" && !!(S && S.groups && S.groups.length)'), 'the board');

  console.log('1. a project just added is taken to its worktree rules');
  // What the board does once a folder it was asked to add is on the list
  await board.run(`rulesFirst(${JSON.stringify(REPO)}); true`);
  let cfgTarget;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    if (!p) return false;
    cfgTarget = (await targetsOf(p)).find((t) => t.type === 'page' && /section=project-first/.test(t.url));
    return !!cfgTarget;
  }, 'the settings page on the new project\'s rules', 30000);
  cfg = await connect(cfgTarget, 'the settings');
  await until(() => cfg.run('!!document.getElementById("project-first")'), 'the way on, above the rules', 30000);
  check(await cfg.run('sel.psection') === 'rules', 'the page is the worktree creation rules');
  check(await cfg.run('document.querySelector(".projbanner .nm").textContent') === 'site', 'the column names the project');
  const pages = await cfg.run('[...document.querySelectorAll("#nav .navitem")].slice(-4).map(b => b.querySelector("span").textContent)');
  check(pages.length === 4 && pages[0] === 'ワークツリーの作成ルール', 'the project\'s pages are listed under it: ' + pages.join(' / '));
  check(await cfg.run('!!document.getElementById("project-prefix") && !!document.getElementById("project-place") && !!document.getElementById("project-bring")'),
    'prefix, placement and the files to inherit are on the page');
  check(await cfg.run('document.querySelector("#project-first .primary").textContent') === 'このまま次へ', 'nothing changed yet: the press keeps what is there');

  console.log('2. where a worktree would go, and what the lines hold');
  await until(() => cfg.run('!!document.querySelector("#placesaid code") && !!document.querySelector("#placesaid code").textContent'), 'the place a worktree would go', 30000);
  const place = await cfg.run('document.querySelector("#placesaid code").textContent');
  check(same(place, BESIDE), 'beside the checkout, named for it and the work: ' + place);
  const served = await cfg.run('document.getElementById("placesaid").textContent');
  check(served.includes('webroot/.htaccess'), 'it says what makes the project served where it stands');
  check(await cfg.run('document.querySelector("#project-place input.grow").placeholder') === '..', 'the default shown is beside the checkout');
  await until(() => cfg.run('document.querySelectorAll("#project-bring .igsize").length >= 2'), 'what each line holds', 30000);
  const hows = await cfg.run('[...document.querySelectorAll("#project-bring .igitem")].map(i => i.querySelector(".igpat .mono").textContent + "=" + i.querySelector("select").value)');
  check(hows.includes('config/local.php=copy') && hows.includes('vendor/=copy'), 'every line starts as a copy: ' + hows.join(', '));
  check(await cfg.run('!!Array.from(document.querySelectorAll("#project-bring button")).find(b => b.textContent === "AIで設定")'), 'the AI is offered on the card');
  check(await cfg.run('!!document.getElementById("project-extra")'), 'files from elsewhere are part of the same card');
  await cfg.shot('1-rules');

  console.log('2b. asking the AI starts from what to tell it');
  const modals = () => cfg.run('document.querySelectorAll(".modal").length');
  const before = await modals();
  await cfg.run('Array.from(document.querySelectorAll("#project-bring button")).find(b => b.textContent === "AIで設定").click(); true');
  const top = '[...document.querySelectorAll(".modal")].pop()';
  await until(() => cfg.run(`!!(${top} && ${top}.querySelector("textarea"))`), 'the AI dialog');
  const hint = await cfg.run(`${top}.querySelector("textarea").value`);
  check(hint.includes('[コピー後に置換が必要なファイル]'), 'the box says what is worth writing: ' + JSON.stringify(hint.split('\n')[0]));
  await cfg.shot('1b-ai');
  await cfg.run(`${top}.querySelector(".mfoot .quiet").click(); true`);
  check(await modals() === before, 'cancelled, the dialog is gone');
  check(!project() || !project().ai_hint, 'nothing is written by opening it');

  console.log('3. a proposal is shown before anything is saved, and saved on the way on');
  await cfg.run(`(() => {
    const desk = desks[sel.desk];
    const p = deskProjects(desk).projects.find(x => x.key === sel.proj);
    inheritConfirm(desk, p, {ok: true, name: "site-feature-x", folder: ${JSON.stringify(BESIDE)}, lines: [
      {source: ".gitignore", pattern: "config/local.php", how: "replace",
       replace: [{find: "/site/", with: "/{name}/", regex: false}], reason: "The checkout is named in its paths."},
      {source: ".gitignore", pattern: "vendor/", how: "copy", replace: [], reason: "Needed to run."}]});
    return true; })()`);
  await until(() => cfg.run('document.querySelectorAll(".aidiff").length >= 2'), 'the lines of the file, before and after', 30000);
  const diff = await cfg.run('[...document.querySelectorAll(".aidiff .after")].map(d => d.textContent)');
  check(diff.some((d) => d.includes('/srv/work/site-feature-x/')) && diff.some((d) => d.includes('dev.local/work/site-feature-x/webroot/')),
    'the rewritten lines name the worktree: ' + diff.join(' | '));
  check(!project() || !(project().bring || []).length, 'nothing is saved while the proposal is only shown');
  await cfg.shot('2-proposal');
  await cfg.click('.modal .mfoot .primary');
  await until(() => (project()?.bring || []).some((r) => r.pattern === 'config/local.php' && r.how === 'replace'), 'the proposal in the settings file', 20000);
  const rule = project().bring.find((r) => r.pattern === 'config/local.php');
  check(JSON.stringify(rule.replace) === JSON.stringify([{ find: '/site/', with: '/{name}/' }]), 'the replacement is saved as proposed: ' + JSON.stringify(rule.replace));

  console.log('4. the board opens the first worktree, where the page said');
  await until(() => board.run('!document.getElementById("branch").hidden'), 'the worktree dialog on the board', 30000);
  await board.run(`branchTab = "name"; drawBranchTabs(document.getElementById("branch")); true`);
  await board.run(`(() => { const q = document.getElementById("bq"); q.value = "feature/x"; q.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => board.run('!!(S.branch && S.branch.asked === "feature/x" && S.branch.folder)'), 'the app\'s answer about feature/x', 30000);
  const folder = await board.run('S.branch.folder');
  check(same(folder, BESIDE), 'the dialog makes it where the page said: ' + folder);
  // The dialog starts from the rules just saved, not from what it was opened on
  // The settings just saved may be read in a moment after the dialog opens;
  // the dialog asks again when they are, and nothing is pressed until then
  await until(() => board.run('!!(S.branch && S.branch.project_name === "site")'), 'the dialog\'s answer from the saved rules', 15000)
    .catch(async (e) => {
      console.log('    (the app\'s answer: ' + JSON.stringify(await board.run(
        `({project: S.branch.project_name, lines: S.branch.carry_lines, gen: S.settings_gen})`)) + ')');
      throw e;
    });
  const offered = await board.run('Object.fromEntries((S.branch.carry || []).map(c => [c.name, c.how]))');
  check(offered['config/local.php'] === 'replace', 'the dialog offers the saved rule: ' + JSON.stringify(offered));
  const rows = await board.run('Object.fromEntries(Array.from(document.querySelectorAll("#branch .bcarry select")).map(s => [s.dataset.name, s.value]))');
  check(rows['config/local.php'] === 'replace', 'and shows it: ' + JSON.stringify(rows));
  await board.run(`document.querySelector("#branch .bgo .go").click(); true`);
  await until(() => fs.existsSync(path.join(BESIDE, 'config', 'local.php')), 'the worktree with its config', 60000);
  await sleep(1000);
  const written = fs.readFileSync(path.join(BESIDE, 'config', 'local.php'), 'utf8');
  check(written.includes("'/srv/work/site-feature-x/'") && written.includes('http://dev.local/work/site-feature-x/webroot/'),
    'the config names the worktree, on the server\'s path and in the URL');
  check(written.includes("$db_pass = 'hunter2';"), 'the rest of the file is as it was');
  check(fs.existsSync(path.join(BESIDE, 'vendor', 'lib', 'a.php')), 'what was installed is copied');
  check(!fs.lstatSync(path.join(BESIDE, 'vendor')).isSymbolicLink(), 'as a copy, not a link');
  check(fs.readFileSync(path.join(REPO, 'config', 'local.php'), 'utf8') === LOCAL_PHP, 'the checkout\'s own config is untouched');
  const made = spawnSync('git', ['-C', BESIDE, 'branch', '--show-current'], { encoding: 'utf8' }).stdout.trim();
  check(made === 'feature/x', 'git is on the branch: ' + made);
  await sleep(800);
  await board.shot('3-made');

  console.log('5. the program-wide settings for worktrees');
  try { cfg.ws.close(); } catch {}
  await board.run('openSettings("worktrees", true); true');
  let globalTarget;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    if (!p) return false;
    globalTarget = (await targetsOf(p)).find((t) => t.type === 'page' && /section=worktrees/.test(t.url));
    return !!globalTarget;
  }, 'the settings on the worktrees card', 30000);
  cfg = await connect(globalTarget, 'the settings');
  await until(() => cfg.run('sel.global && sel.section === "worktrees"'), 'the worktrees card');
  const markers = await cfg.run('[...document.querySelectorAll(".subsec input")].map(i => i.value)');
  check(markers.length === 2 && markers[0].includes('.htaccess') && markers[1].includes('htdocs'),
    'the markers are shown filled with the defaults: ' + markers.join(' | '));
  check(await cfg.run('!!document.querySelector("#detail input[type=checkbox]") && document.querySelector("#detail input[type=checkbox]").checked'),
    'worktrees go in a folder named for their project unless turned off');
  await cfg.shot('4-global');
} catch (e) {
  check(false, e.message);
} finally {
  try { board.ws.close(); } catch {}
  try { cfg && cfg.ws.close(); } catch {}
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);
