/**
 * A check's log, opened from the git panel's CI list, through the running
 * app's own window and against GitHub itself.
 *
 * This checkout's build in a folder of its own, over a shallow clone of this
 * program's own public repository, with the project signed in to GitHub as an
 * account of the copy's (the token from `.private/.env`, read only). The git
 * panel is opened on the clone's `main`, its CI list opened, and a check
 * GitHub Actions ran is pressed. Checked:
 *
 *   - the row says the log is on its way, and then an editor opens in front
 *   - the editor only reads: its name is the check and the commit, the
 *     read-only mark is on, Ace refuses typing, and a save is refused
 *   - what it shows is the job's log -- the first lines match what GitHub
 *     serves for that job -- without colour codes, stamps cut to the time
 *   - a phone-width board is offered the same rows to press
 *
 * A check with no log to download (a commit status) would open its page in
 * a browser tab; `main` of this repository has none, so that road is covered
 * by the unit tests and not here.
 *
 *     cargo build
 *     node tools/debug/ci-log.win.mjs
 *
 * Needs Windows, git, Node, Chrome (for the phone width), network, and
 * GITHUB_PAT in .private/.env with read access to Actions on
 * styleio/ShikishaTerm. Reads only. Nothing of a copy somebody is using is
 * read, written or stopped; the copy is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-ci-log');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'ShikishaTerm');
const LOCAL = path.join(RUN, 'local');
const SHOTS = path.join(ROOT, 'target', 'shots', 'ci-log');
const REPO = 'styleio/ShikishaTerm';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-ci-log*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

// A worktree has no .private of its own: PRIVATE_ENV names the checkout's
const envFile = process.env.PRIVATE_ENV || path.join(ROOT, '.private', '.env');
const dotenv = fs.existsSync(envFile) ? Object.fromEntries(fs.readFileSync(envFile, 'utf8').split(/\r?\n/)
  .map((l) => l.match(/^([A-Z0-9_]+)=(.*)$/)).filter(Boolean).map((m) => [m[1], m[2].replace(/^"|"$/g, '')])) : {};
const PAT = dotenv.GITHUB_PAT;
if (!PAT) die('GITHUB_PAT is needed in .private/.env');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('a shallow clone of ' + REPO + ', and this checkout\'s build beside it');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const cloned = spawnSync('git', ['clone', '-q', '--depth', '1', `https://github.com/${REPO}.git`, WORK], { encoding: 'utf8' });
if (cloned.status !== 0) die('clone failed: ' + cloned.stderr);
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: process.env.CILOG_LANG || 'ja',
  remote: { enabled: false },
  git_accounts: [{ name: 'check', owners: ['styleio'] }],
  desks: [{
    name: 'Check', id: 'check',
    projects: [{ name: 'ShikishaTerm', at: WORK, git_account: 'check' }],
    folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }],
  }],
}, null, 2));
fs.writeFileSync(path.join(APP, 'config', 'secrets.json'), JSON.stringify({ tokens: { 'git/check': PAT } }, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};
const portOf = () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};

async function connect(url) {
  const ws = new WebSocket(url);
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
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  const shot = async (name) => {
    const { data } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, name + '.png'), Buffer.from(data, 'base64'));
  };
  return { send, run, shot, close: () => ws.close() };
}

let board;
try {
  await until(() => portOf(), 'the board\'s DevTools port');
  let target;
  await until(async () => {
    const list = await (await fetch(`http://127.0.0.1:${portOf()}/json/list`)).json();
    target = list.find((t) => t.type === 'page' && /127\.0\.0\.1/.test(t.url));
    return !!target;
  }, 'the board page');
  board = await connect(target.webSocketDebuggerUrl);
  await until(() => board.run('typeof S !== "undefined" && !!(S && S.tabs && S.tabs.length)'), 'the board state');

  console.log('1. the git panel lists the CI of main');
  await board.run('sideReveal("git"); setSideWidth(460); true');
  await until(() => board.run('!!(G.branch && G.branch.name)'), 'the branch', 60000);
  await until(() => board.run('!!(G.checks && G.checks.items && G.checks.items.length)'), 'the checks from GitHub', 90000);
  await board.run('G.ciOpen = true; drawGit(); true');
  const rows = await board.run('[...document.querySelectorAll("#gitpanel .gcheck button.glog")].length');
  check(rows > 0, 'every check is a row to press: ' + rows);
  await board.shot('1-ci-list');

  // A job still running has no log yet: its page, live, in a browser tab
  const running = await board.run('(G.checks.items.find(i => i.job != null && i.verdict === "pending") || {}).name || ""');
  if (running) {
    console.log('1b. a check still running opens its page instead, and says why');
    const pages = await board.run('S.tabs.filter(t => t.kind === "browser").length');
    await board.run(`[...document.querySelectorAll("#gitpanel .gcheck button.glog")]
      .find(b => b.querySelector(".nm").textContent === ${JSON.stringify(running)}).click(); true`);
    await until(() => board.run('G.ciLog === null'), 'the answer about ' + running, 30000);
    check(await board.run(`!G.bad && G.said === (T["git.ci.log.running"] || "").replaceAll("{name}", ${JSON.stringify(running)})`),
      'the panel says it is still running and its page is open: ' + await board.run('G.said'));
    await until(() => board.run(`S.tabs.filter(t => t.kind === "browser").length > ${pages}`), 'a browser tab for it', 30000);
    check(true, 'a browser tab was opened for its page');
    await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
    await until(() => board.run('S.tabs.find(t => t.index === S.active).name === "shell"'), 'the terminal in front');
    await board.run('sideReveal("git"); G.ciOpen = true; drawGit(); true');
  }
  // A check GitHub Actions did not run has no log to download: its page
  const other = await board.run('(G.checks.items.find(i => i.job == null && i.url) || {}).name || ""');
  if (other) {
    console.log('1c. a check with no log opens its page instead, and says why');
    const pages = await board.run('S.tabs.filter(t => t.kind === "browser").length');
    await board.run(`[...document.querySelectorAll("#gitpanel .gcheck button.glog")]
      .find(b => b.querySelector(".nm").textContent === ${JSON.stringify(other)}).click(); true`);
    await until(() => board.run('G.ciLog === null'), 'the answer about ' + other, 30000);
    check(await board.run(`!G.bad && G.said === (T["git.ci.log.page"] || "").replaceAll("{name}", ${JSON.stringify(other)})`),
      'the panel says it has no log and its page is open: ' + await board.run('G.said'));
    await until(() => board.run(`S.tabs.filter(t => t.kind === "browser").length > ${pages}`), 'a browser tab for it', 30000);
    check(true, 'a browser tab was opened for its page');
    await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
    await until(() => board.run('S.tabs.find(t => t.index === S.active).name === "shell"'), 'the terminal in front');
    await board.run('sideReveal("git"); G.ciOpen = true; drawGit(); true');
    await until(() => board.run('!!(G.checks && G.checks.items)'), 'the checks again', 90000);
  }
  // A finished one to read. CI on the newest commit may be at work; waited
  // for rather than skipped, since reading one is the point of this check
  await until(() => board.run('!!G.checks.items.find(i => i.job != null && i.verdict !== "pending")'), 'a finished job', 20 * 60000);
  await board.run('G.ciOpen = true; drawGit(); true');
  const job = await board.run('G.checks.items.find(i => i.job != null && i.verdict !== "pending").name');
  check(!!job, 'a finished check GitHub Actions ran is among them: ' + job);
  // Read now: once the editor is in front the panel follows it and reads again
  const jobId = await board.run(`G.checks.items.find(i => i.name === ${JSON.stringify(job)}).job`);

  console.log('2. pressing it opens its log in an editor that only reads');
  await board.run(`[...document.querySelectorAll("#gitpanel .gcheck button.glog")]
    .find(b => b.querySelector(".nm").textContent === ${JSON.stringify(job)}).click(); true`);
  check(await board.run('G.ciLog !== null'), 'the row says the log is on its way');
  // A refusal is said on the panel at once; waiting out the clock for it
  // would only hide what it said
  for (let end = Date.now() + 90000; ; await sleep(250)) {
    const said = await board.run('G.bad ? G.said : ""');
    if (said) throw new Error('the log was refused: ' + said);
    if (await board.run('!!(editorTab() && editorTab().read_only && ED.text && ED.text.length > 0)')) break;
    if (Date.now() > end) throw new Error('timed out waiting for the log in the editor');
  }
  const t = await board.run('({file: editorTab().file, ro: edAce.getReadOnly(), mark: document.querySelector("#editpanel .mark") ? document.querySelector("#editpanel .mark").textContent : "", first: ED.text.split("\\n").slice(0, 3)})');
  check(/^[0-9a-f]{7}$/.test(t.file.slice((job.replace(/[\\/]/g, '-') + ' @ ').length)) && t.file.startsWith(job.replace(/[\\/]/g, '-') + ' @ '),
    'named after the check and its commit: ' + t.file);
  check(t.ro === true, 'the editor refuses typing');
  check(!/\u001b\[/.test(await board.run('ED.text')), 'no colour codes are left');
  check(/^\d\d:\d\d:\d\d /.test(t.first[0]), 'each line starts with the time of day: ' + t.first[0]);
  check(await board.run('G.ciLog === null && !G.bad'), 'the row is quiet again, and nothing went wrong');
  await board.shot('2-log');

  // The same log, straight from GitHub, as the check says
  const raw = await (await fetch(`https://api.github.com/repos/${REPO}/actions/jobs/${jobId}/logs`,
    { headers: { Authorization: 'Bearer ' + PAT, 'User-Agent': 'ci-log-check' } })).text();
  const wanted = raw.split(/\r?\n/)[0].replace(/\u001b\[[0-9;]*[A-Za-z]/g, '').replace(/^\S*T(\d\d:\d\d:\d\d)\.\d+Z /, '$1 ');
  check(t.first[0] === wanted.replace(/^\uFEFF/, ''), 'the first line is GitHub\'s first line: ' + wanted.slice(0, 60));

  console.log('3. a save is refused');
  await board.run('editSave(); true');
  await sleep(800);
  check(await board.run('!ED.dirty && editorTab().read_only'), 'nothing to save and still read-only');

  console.log('4. a phone-width board is offered the same rows');
  await board.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await sleep(600);
  // The terminal in front again: the panel is the folder's, and a list of no
  // rows would pass any test of its rows
  await board.run('send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index}); true');
  await until(() => board.run('S.tabs.find(t => t.index === S.active).name === "shell"'), 'the terminal in front');
  await board.run('sideReveal("git"); true');
  await until(() => board.run('!!(G.checks && G.checks.items && G.checks.items.length)'), 'the checks again', 90000);
  await board.run('G.ciOpen = true; drawGit(); true');
  await sleep(600);
  const heights = await board.run('[...document.querySelectorAll("#gitpanel .gcheck button.glog")].map(b => b.getBoundingClientRect().height)');
  check(heights.length > 0 && heights.every((h) => h >= 22), 'the rows are there, each at least 22px tall to a finger: ' + heights.join(', '));
  await board.shot('3-phone');
  await board.send('Emulation.clearDeviceMetricsOverride');
} catch (e) {
  console.error(e.message || e);
  failures += 1;
} finally {
  if (board) board.close();
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
