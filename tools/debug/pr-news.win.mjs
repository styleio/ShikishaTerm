/**
 * A pull request's review coming back, told: the rows, the toast with its
 * button, and the desk's default destination -- through the running app's own
 * window and against GitHub itself.
 *
 * Two open pull requests of a public repository whose reviews come from a bot
 * that reviews every push (PRNEWS_REPO, by default one known to have them):
 *
 *   - one whose newest commit has the bot's review, with threads still open
 *   - one whose newest commit has the bot's 👍 and no review (nothing found)
 *
 * Each branch is shallow-cloned, and this checkout's build is put beside them
 * with both as projects, signed in as an account of the copy's (the token
 * from `.private/.env`, read only). What the app remembers of each pull
 * request is set as it would be just before a push -- the commit before the
 * newest, the bot already known as its reviewer -- so the app's first look
 * sees the push and then the review of it. Checked:
 *
 *   - the review with findings is told: a toast naming the pull request and
 *     how many findings were not yet given to an AI, with the button that
 *     hands them over; the desk's default destination is sent the same words
 *   - its row says how many there are to fix
 *   - the 👍 is told as a review that found nothing, without a button
 *   - started again, neither is told a second time
 *   - the button is answered in a toast of its own (here, that no AI is set
 *     up: nothing is handed to anybody)
 *   - with the project's review tick off, its review is not told
 *
 *     cargo build
 *     node tools/debug/pr-news.win.mjs
 *
 * Needs Windows, git, Node, network, and GITHUB_PAT in .private/.env (or
 * PRIVATE_ENV naming the file). Reads only: nothing is written to GitHub.
 * Nothing of a copy somebody is using is read, written or stopped; the copy
 * is stopped on the way out (PRNEWS_KEEP=1 leaves it running to be looked at).
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-pr-news');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'local');
const SHOTS = path.join(ROOT, 'target', 'shots', 'pr-news');
const REPO = process.env.PRNEWS_REPO || 'openai/codex';
const [OWNER, NAME] = REPO.split('/');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-pr-news*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const lines = (text) => text.split(/\r?\n/);

const envFile = process.env.PRIVATE_ENV || path.join(ROOT, '.private', '.env');
const dotenv = fs.existsSync(envFile) ? Object.fromEntries(lines(fs.readFileSync(envFile, 'utf8'))
  .map((l) => l.match(/^([A-Z0-9_]+)=(.*)$/)).filter(Boolean).map((m) => [m[1], m[2].replace(/^"|"$/g, '')])) : {};
const PAT = dotenv.GITHUB_PAT;
if (!PAT) die('GITHUB_PAT is needed in .private/.env');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

const graphql = async (query) => {
  const r = await fetch('https://api.github.com/graphql', {
    method: 'POST', headers: { Authorization: 'Bearer ' + PAT, 'User-Agent': 'pr-news-check' },
    body: JSON.stringify({ query }),
  });
  const v = await r.json();
  if (!v.data) die('GitHub said: ' + JSON.stringify(v.errors || v));
  return v.data;
};

console.log('0. two pull requests of ' + REPO + ' a bot has reviewed');
const found = (await graphql(`query{repository(owner:"${OWNER}",name:"${NAME}"){pullRequests(states:OPEN,last:60){nodes{
  number headRefName headRefOid headRepositoryOwner{login}
  commits(last:2){nodes{commit{oid committedDate}}}
  reviews(last:20){nodes{id author{login __typename} commit{oid}}}
  reviewThreads(first:50){nodes{isResolved}}
  reactions(last:20){nodes{content createdAt user{login}}}}}}}`)).repository.pullRequests.nodes
  .filter((p) => (p.headRepositoryOwner || {}).login === OWNER && p.commits.nodes.length === 2);
const botOnHead = (p) => p.reviews.nodes.filter((r) => (r.author || {}).__typename === 'Bot' && (r.commit || {}).oid === p.headRefOid);
const withFindings = found.find((p) => botOnHead(p).length && p.reviewThreads.nodes.some((t) => !t.isResolved));
const head = (p) => p.commits.nodes[1].commit;
const thumbed = found.find((p) => !botOnHead(p).length && p.reactions.nodes.some((r) => r.content === 'THUMBS_UP'
  && /\[bot\]$/.test((r.user || {}).login || '') && Date.parse(r.createdAt) > Date.parse(head(p).committedDate)));
if (!withFindings || !thumbed) die('no pull request of ' + REPO + ' fits just now; set PRNEWS_REPO to another');
console.log(`   #${withFindings.number} (${withFindings.headRefName}) has the bot's review of its newest commit`);
console.log(`   #${thumbed.number} (${thumbed.headRefName}) has the bot's 👍 since its newest commit`);
const bot = (p) => p === withFindings
  ? botOnHead(p)[0].author.login
  : p.reactions.nodes.find((r) => r.content === 'THUMBS_UP' && /\[bot\]$/.test(r.user.login)).user.login.replace(/\[bot\]$/, '');

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, SHOTS, path.join(APP, 'data')]) fs.mkdirSync(d, { recursive: true });
const dirs = {};
for (const [key, p] of [['review', withFindings], ['thumb', thumbed]]) {
  dirs[key] = path.join(RUN, key);
  const c = spawnSync('git', ['clone', '-q', '--depth', '1', '--single-branch', '--branch', p.headRefName,
    `https://github.com/${REPO}.git`, dirs[key]], { encoding: 'utf8' });
  if (c.status !== 0) die('clone failed: ' + c.stderr);
}
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const config = (reviewTick) => ({
  language: process.env.PRNEWS_LANG || 'ja',
  remote: { enabled: false },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  // No AI for the button to start: what it answers is all that is checked
  ai_engine: 'nobody-here',
  git_accounts: [{ name: 'check', owners: [OWNER] }],
  desks: [{
    name: 'Check', id: 'check',
    notify: { 'このPC': { type: 'windows' } },
    primary_notify: 'このPC',
    projects: [
      { name: 'review', at: dirs.review, git_account: 'check', ...(reviewTick ? {} : { git: { notify_review: false } }) },
      { name: 'thumb', at: dirs.thumb, git_account: 'check' },
    ],
    folders: [
      { cwd: dirs.review, tabs: [{ name: 'review', id: 'review', command: 'cmd.exe' }] },
      { cwd: dirs.thumb, tabs: [{ name: 'thumb', id: 'thumb', command: 'cmd.exe' }] },
    ],
  }],
});
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.writeFileSync(path.join(APP, 'config', 'secrets.json'), JSON.stringify({ tokens: { 'git/check': PAT } }, null, 2));
// Remembered as just before the push: the commit before the newest, the bot
// known as a reviewer, and the reviews of the commits before it already seen
const remembered = () => {
  const prs = {};
  for (const p of [withFindings, thumbed]) {
    const before = p.commits.nodes[0].commit;
    prs[`${REPO.toLowerCase()}#${p.number}`] = {
      head: before.oid, since: Math.floor(Date.parse(before.committedDate) / 1000), waiting: false,
      reviewers: [bot(p)], seen: p.reviews.nodes.filter((r) => (r.commit || {}).oid !== p.headRefOid).map((r) => r.id),
      ended_at: 0, ended: 'none', ended_on: '', ci_running: '', ci_told: '', at: 1,
    };
  }
  fs.writeFileSync(path.join(APP, 'data', 'pr-news.json'), JSON.stringify({ version: 1, prs }, null, 2));
};

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
const hooksLog = () => {
  const f = path.join(APP, 'logs', 'hooks.log');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '';
};

// Start the copy and come back with its board, every toast shown from then on kept
async function start() {
  stopApp();
  await sleep(800);
  try { fs.rmSync(path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort')); } catch (e) {}
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  await until(() => portOf(), 'the board\'s DevTools port');
  let target;
  await until(async () => {
    const list = await (await fetch(`http://127.0.0.1:${portOf()}/json/list`)).json();
    target = list.find((t) => t.type === 'page' && /127\.0\.0\.1/.test(t.url));
    return !!target;
  }, 'the board page');
  const { ws, send, run } = await connectCdp(target.webSocketDebuggerUrl);
  await until(() => run('typeof S !== "undefined" && !!(S && S.tabs && S.tabs.length)'), 'the board state');
  await run(`window.__toasts = []; const shown = toast;
    toast = function (text, warn, act) { window.__toasts.push({text, warn: !!warn, act: act ? act.label : null}); return shown.apply(this, arguments); }; true`);
  const shot = async (name) => {
    const { data } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, name + '.png'), Buffer.from(data, 'base64'));
  };
  return { run, shot, close: () => ws.close() };
}

let board;
try {
  fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify(config(true), null, 2));
  remembered();
  board = await start();

  console.log('1. the review with findings, and the 👍, are told');
  // What the screen said of each pull request's review, as it keeps it: in
  // time for a window still loading when the news came. The pushes set up
  // here are noticed, so CI ending is told about as well; that is not this
  // Each said the way the words for it say it: the part between the number
  // and the title, which names what came back
  const middle = (key) => `(T["${key}"] || "").split("{n}").pop().split("{count}").pop().split("{title}")[0]`;
  const toastOf = (n) => board.run(`JSON.stringify(PRN.said.find(t => t.text.includes("#${n}")
    && [${middle("pr.told.found")}, ${middle("pr.told.clean")}].some(m => t.text.includes(m))) || null)`).then(JSON.parse);
  await until(async () => (await toastOf(withFindings.number)) && (await toastOf(thumbed.number)), 'both toasts', 120000);
  const a = await toastOf(withFindings.number), b = await toastOf(thumbed.number);
  console.log('   ' + a.text + '\n   ' + b.text);
  check(/\d/.test(a.text) && a.act === await board.run('T["git.review.fix"]'), 'the findings are counted, with the button that hands them to an AI');
  const clean = await board.run(middle("pr.told.clean"));
  check(b.act === null && b.text.includes(clean), 'the 👍 is a review that found nothing, without a button');
  // The lines the default destination was sent about each one's review
  const reviewSent = (log, n) => lines(log).filter((l) => l.includes(`pr news ${REPO}#${n}:`) && /NOTIFY\[/.test(l)
    && /レビューが付きました|レビューが終わりました|a review came back|the review came back/.test(l));
  check(reviewSent(hooksLog(), withFindings.number).length === 1, 'the desk\'s default destination was sent the review');
  check(reviewSent(hooksLog(), thumbed.number).length === 1, 'the desk\'s default destination was sent the 👍');

  console.log('2. the rows say it');
  await until(() => board.run(`[...document.querySelectorAll(".tab .prnews")].some(e => e.classList.contains("need"))`), 'the row of findings', 30000)
    .catch(async (e) => { console.log('   places: ' + await board.run('JSON.stringify(S.tabs.map(t => [t.name, t.place]))')); throw e; });
  const chips = await board.run(`JSON.stringify([...document.querySelectorAll(".tab .prnews")].map(e => [e.textContent, e.className]))`);
  console.log('   ' + chips);
  check(JSON.parse(chips).some(([t, c]) => /need/.test(c) && /\d/.test(t)), 'the row with findings says how many there are to fix');
  check(JSON.parse(chips).some(([, c]) => /ok/.test(c)), 'the row of the 👍 says nothing was found');
  await board.shot('1-told');

  console.log('3. started again, neither is told a second time');
  board.close();
  const logAt = hooksLog().length;
  board = await start();
  await until(() => board.run(`[...document.querySelectorAll(".tab .prnews")].some(e => e.classList.contains("need"))`), 'the rows again', 60000);
  await sleep(5000);
  check(!(await toastOf(withFindings.number)) && !(await toastOf(thumbed.number)), 'nothing was told again on screen');
  check(!/pr news .*NOTIFY\[/.test(hooksLog().slice(logAt)), 'nothing was sent again');
  const before = await board.run('window.__toasts.length');

  console.log('4. the button is answered');
  await board.run(`prNewsToast(Object.assign({}, ${JSON.stringify({ what: 'found', project: 'review', number: withFindings.number,
    head: withFindings.headRefName, sha: withFindings.headRefOid, text: 'check' })})); document.getElementById("toastact").click(); true`);
  await until(() => board.run('window.__toasts.length > ' + (before + 1)), 'the answer to the button', 60000);
  const answer = await board.run('JSON.stringify(window.__toasts[window.__toasts.length - 1])').then(JSON.parse);
  console.log('   ' + answer.text);
  check(!!answer.text && answer.act === null, 'the press was answered in a toast of its own');
  board.close();
  board = null;

  console.log('5. with the project\'s review tick off, its review is not told');
  fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify(config(false), null, 2));
  remembered();
  const logBefore = hooksLog().length;
  board = await start();
  await until(async () => !!(await toastOf(thumbed.number)), 'the 👍 again', 120000);
  await sleep(5000);
  check(!(await toastOf(withFindings.number)), 'the review was not told on screen');
  const sent = reviewSent(hooksLog().slice(logBefore), withFindings.number);
  check(sent.length === 0, 'nothing was sent of its review' + (sent.length ? ': ' + sent.join(' | ') : ''));
  const left = JSON.parse(fs.readFileSync(path.join(APP, 'data', 'pr-news.json'), 'utf8')).prs[`${REPO.toLowerCase()}#${withFindings.number}`];
  check(left && left.head === withFindings.headRefOid && left.ended === 'reviewed', 'what came of it is remembered all the same');
} catch (e) {
  console.error(e.message || e);
  failures += 1;
} finally {
  if (board) board.close();
  if (!process.env.PRNEWS_KEEP) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
