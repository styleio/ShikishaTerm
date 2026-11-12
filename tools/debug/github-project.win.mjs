/**
 * A GitHub Project on the Issue tab, through the running app and GitHub itself.
 *
 * A folder whose origin is GHP_REPO (owner/name) is made a project of a copy
 * of this checkout's build, signed in as GHP_ACCOUNT: "pc" for the account
 * git on this PC holds, else the token GITHUB_PAT in .private/.env. The Issue
 * tab is opened and switched to Project. ("pc:<login>" picks one of several
 * accounts git on this PC holds.) Then, by what the token may do:
 *
 *   - it may not see GitHub Projects: the tab says so, with the way to the
 *     account's settings (this is checked whenever it is the answer)
 *   - it may: the GitHub Projects of the repository are listed; the first
 *     (or GHP_PICK, an address) is opened, and its view is drawn -- a board
 *     in columns, or a table -- with as many cards as GitHub says it has
 *
 * Only with GHP_WRITE=1 is anything written, and only to the GitHub Project
 * opened: the first card is moved to another column, read back from GitHub
 * there, and moved back. Point it at a GitHub Project of your own.
 *
 *     cargo build
 *     set GHP_REPO=owner/name
 *     node tools/debug/github-project.win.mjs
 *
 * Nothing of a copy somebody is using is read, written or stopped; the copy
 * is stopped on the way out (GHP_KEEP=1 leaves it running to be looked at).
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-gh-project');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'local');
const DIR = path.join(RUN, 'repo');
const SHOTS = path.join(ROOT, 'target', 'shots', 'github-project');
const REPO = process.env.GHP_REPO || '';
const ACCOUNT = process.env.GHP_ACCOUNT || 'token';
const WRITE = process.env.GHP_WRITE === '1';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-gh-project*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const lines = (text) => text.split(/\r?\n/);
if (!/^[\w.-]+\/[\w.-]+$/.test(REPO)) die('set GHP_REPO to owner/name');
const envFile = process.env.PRIVATE_ENV || path.join(ROOT, '.private', '.env');
const dotenv = fs.existsSync(envFile) ? Object.fromEntries(lines(fs.readFileSync(envFile, 'utf8'))
  .map((l) => l.match(/^([A-Z0-9_]+)=(.*)$/)).filter(Boolean).map((m) => [m[1], m[2].replace(/^"|"$/g, '')])) : {};
const PAT = dotenv.GITHUB_PAT;
if (!ACCOUNT.startsWith('pc') && !PAT) die('GITHUB_PAT is needed in .private/.env (or GHP_ACCOUNT=pc)');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, DIR, SHOTS, path.join(APP, 'config')]) fs.mkdirSync(d, { recursive: true });
const git = (...a) => { const r = spawnSync('git', ['-C', DIR, ...a], { encoding: 'utf8' }); if (r.status !== 0) die('git: ' + r.stderr); };
git('init', '-q', '-b', 'main');
git('remote', 'add', 'origin', `https://github.com/${REPO}.git`);
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const onPc = ACCOUNT === 'pc' || ACCOUNT.startsWith('pc:');
const account = ACCOUNT === 'pc' ? '@pc' : onPc ? '@' + ACCOUNT : 'check';
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: process.env.GHP_LANG || 'ja', remote: { enabled: false },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' }, ai_engine: 'nobody-here',
  git_accounts: onPc ? [] : [{ name: 'check', owners: [REPO.split('/')[0]] }],
  desks: [{ name: 'Check', id: 'check',
    projects: [{ name: 'repo', at: DIR, git_account: account }],
    folders: [{ cwd: DIR, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));
if (!onPc) fs.writeFileSync(path.join(APP, 'config', 'secrets.json'), JSON.stringify({ tokens: { 'git/check': PAT } }, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};
const portOf = () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(lines(fs.readFileSync(f, 'utf8'))[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};

let ws;
try {
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  await until(() => portOf(), 'the board\'s DevTools port');
  let target;
  await until(async () => {
    const list = await (await fetch(`http://127.0.0.1:${portOf()}/json/list`)).json();
    target = list.find((t) => t.type === 'page' && /127\.0\.0\.1/.test(t.url));
    return !!target;
  }, 'the board page');
  const cdp = await connectCdp(target.webSocketDebuggerUrl);
  ws = cdp.ws;
  const { run, send } = cdp;
  const shot = async (name) => {
    const { data } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, name + '.png'), Buffer.from(data, 'base64'));
  };
  await until(() => run('typeof S !== "undefined" && !!(S && S.tabs && S.tabs.length)'), 'the board state');

  console.log('1. the Issue tab, switched to Project');
  await run('send({kind:"openissues"}); true');
  await until(() => run('!document.getElementById("issuespanel").hidden && Array.isArray(I.projects) && I.projects.length > 0'), 'the Issue tab', 30000);
  await run('[...document.querySelectorAll("#issuespanel .seg button")].find(b => b.textContent === (T["issues.kind.ghp"] || "")).click(); true');
  check(await run('I.kind === "ghp"'), 'the Project button switches the tab to GitHub Projects');
  // The list of them (nothing chosen yet) or the reason none can be seen
  await until(() => run('I.gh.list !== null || !!I.gh.listErr'), 'the GitHub Projects or why not', 60000);
  const listed = JSON.parse(await run('JSON.stringify({list: I.gh.list, err: I.gh.listErr, fix: I.gh.listFix, hidden: I.gh.hidden})'));
  await shot('1-chosen-from');
  if (listed.err) {
    console.log('   GitHub said: ' + listed.err);
    const denied = await run('T["err.ghproject.denied"]');
    check(listed.err === denied || /organization forbids/.test(listed.err), 'a token that may not see GitHub Projects is told so');
    check(listed.fix && await run('[...document.querySelectorAll("#issuespanel .bwarn button")].length > 0'), 'with the way to the account\'s settings beside it');
  } else {
    console.log(`   ${listed.list.length} listed, ${listed.hidden} hidden: ` + listed.list.map((p) => p.title + ' (' + p.from + ')').join(', '));
    check(listed.list.length > 0, 'the GitHub Projects of the repository are listed');
    const pick = process.env.GHP_PICK || (listed.list[0] || {}).url;
    console.log('2. ' + pick + ' opened');
    await run(`ghpPick(${JSON.stringify(pick)}); true`);
    await until(() => run('!!I.gh.data || !!I.gh.err'), 'the GitHub Project', 90000);
    const read = JSON.parse(await run('JSON.stringify({err: I.gh.err, d: I.gh.data && {title: I.gh.data.title, view: I.gh.data.view, views: I.gh.data.views.map(w => [w.number, w.layout]), cards: I.gh.data.cards.length, columns: I.gh.data.columns}})'));
    await shot('2-opened');
    if (read.err) { check(false, 'the GitHub Project was read: ' + read.err); throw new Error('stop'); }
    console.log('   ' + JSON.stringify(read.d));
    const layout = (read.d.views.find(([n]) => n === read.d.view) || [])[1];
    const drawn = await run('JSON.stringify({cards: document.querySelectorAll("#issuespanel .ghcard").length, rows: document.querySelectorAll("#issuespanel .ght tbody tr:not(.grp)").length, cols: document.querySelectorAll("#issuespanel .ghcol").length})');
    console.log('   drawn: ' + drawn);
    const n = JSON.parse(drawn);
    check(layout === 'board' && read.d.columns ? n.cards === read.d.cards && n.cols > 0 : n.rows === read.d.cards,
      'every card GitHub has is drawn, as the view lays them out');
    // The choice stays with the project
    await until(() => run(`(I.projects.find(p => p.name === "repo") || {}).gh_project === ${JSON.stringify(pick)}`), 'the choice kept in the settings', 15000)
      .then(() => check(true, 'the GitHub Project chosen is kept on the project'))
      .catch(() => check(false, 'the GitHub Project chosen is kept on the project'));
    if (WRITE) {
      console.log('3. a card moved, read back, and moved back');
      const plan = JSON.parse(await run(`(() => { const d = I.gh.data; const f = d.fields.find(x => x.id === d.columns);
        const c = d.cards.find(x => x.kind !== "hidden"); if (!f || !c) return "null";
        const now = ghpKey(c, f); const to = (f.options || f.iterations).find(o => o.id !== now);
        return JSON.stringify({card: c.id, field: f.id, now, to: to && to.id, iter: f.type === "ITERATION"}); })()`));
      if (!plan || !plan.to) check(false, 'a card and another column to move it to');
      else {
        const value = (id) => id === null ? null : plan.iter ? { iteration: id } : { option: id };
        const move = async (id) => {
          await run(`(() => { const d = I.gh.data; ghpSet(d.cards.find(x => x.id === ${JSON.stringify(plan.card)}), d.fields.find(x => x.id === ${JSON.stringify(plan.field)}), ${JSON.stringify(value(id))}); return true; })()`);
          await until(() => run(`(() => { const c = (I.gh.data.cards || []).find(x => x.id === ${JSON.stringify(plan.card)}); const f = I.gh.data.fields.find(x => x.id === ${JSON.stringify(plan.field)}); return !!c && ghpKey(c, f) === ${JSON.stringify(id)} && !I.bad; })()`), 'GitHub to have it', 60000);
        };
        await move(plan.to);
        await sleep(3000);
        check(await run(`(() => { const c = I.gh.data.cards.find(x => x.id === ${JSON.stringify(plan.card)}); return ghpKey(c, I.gh.data.fields.find(x => x.id === ${JSON.stringify(plan.field)})) === ${JSON.stringify(plan.to)}; })()`),
          'the card is in the other column as GitHub reads it back');
        await shot('3-moved');
        await move(plan.now);
        check(true, 'the card was moved back');
      }
    }
  }
} catch (e) {
  if (e.message !== 'stop') { console.error(e); failures += 1; }
} finally {
  try { ws && ws.close(); } catch (e) {}
  if (!process.env.GHP_KEEP) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
