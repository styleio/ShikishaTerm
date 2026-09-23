/**
 * A tab that came up on a conversation of nobody's offers the way back.
 *
 * The case it is for is the one that costs a day: the app has lost which
 * conversation a tab was having -- the settings were saved and the tab was
 * started again, a folder was written another way, a record was thrown away --
 * and the tab comes up clean, looking exactly like every other clean tab. What
 * is left is the CLI's own records, which say which folder each conversation
 * belongs to, and they are on the disk whatever the app remembers.
 *
 * So this starts the app built in this checkout, with a folder that has two
 * conversations behind it and nothing remembered about the tab, and checks
 * that the caption offers the way back, that pressing it lists what was said
 * here newest first, and that picking one relaunches the tab resuming it.
 *
 * Then it starts the copy again over a conversation that was written down and
 * is not on the machine any more -- the loss itself, which used to happen
 * without a word -- and checks that the tab says so, that saying so outlasts
 * the first thing the person types, and that it stops once it has been taken
 * up.
 *
 * Last, it starts the copy with a page in front of the tab -- the screen
 * numbers then run ahead of the tab list -- and checks that the list is the
 * pressed tab's and that picking from it relaunches that tab, not its
 * neighbour. On 2026-09-23 the list opened empty in exactly that arrangement.
 * Along the way it checks that each start keeps a copy of what it read
 * (`last-session.prev`) and says in the log why a tab came up clean.
 *
 *     cargo build
 *     node tools/debug/past-back.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder,
 * its own LOCALAPPDATA, its own home, and a stub CLI on its PATH -- so no
 * account is used and nothing leaves the machine. Nothing of a copy somebody
 * is using is read, written or stopped. The app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9349;
const RUN = path.join(os.tmpdir(), 'sk-pastback');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work', 'shop');
const HOME = path.join(RUN, 'home');
const STUB = path.join(RUN, 'stub');
const CONFIG = path.join(APP, 'config', 'config.json');
const ARGV = path.join(RUN, 'argv.txt');

const OLDER = '11111111-1111-4111-8111-111111111111';
const NEWER = '22222222-2222-4222-8222-222222222222';
const ELSEWHERE = '33333333-3333-4333-8333-333333333333';

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

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, HOME, STUB, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });

// The CLI, as far as this check needs one: it writes down the command line it
// was given -- which is how what the app asked it to resume is read -- and
// then sits there so the tab lives
fs.writeFileSync(path.join(STUB, 'claude.cmd'),
  '@echo off\r\n'
  + `echo %*> "${ARGV}"\r\n`
  + ':wait\r\n'
  + 'timeout /t 60 /nobreak >nul\r\n'
  + 'goto wait\r\n');

// Two conversations in this folder and one somewhere else, written the way
// Claude Code writes them: a file per conversation, named after it, saying
// which folder it belongs to and what was asked in it
const records = path.join(HOME, '.claude', 'projects', 'shop');
fs.mkdirSync(records, { recursive: true });
const said = (cwd, text) => JSON.stringify({
  type: 'user', cwd, message: { role: 'user', content: text },
}) + '\n';
const write = (id, cwd, text, minutesAgo) => {
  const at = path.join(records, id + '.jsonl');
  fs.writeFileSync(at, said(cwd, text));
  const when = new Date(Date.now() - minutesAgo * 60000);
  fs.utimesSync(at, when, when);
};
write(OLDER, WORK, 'take an email address on the login page', 90);
write(NEWER, WORK, 'say which of the two was wrong when a sign-in fails', 10);
write(ELSEWHERE, path.join(RUN, 'work', 'other'), 'nothing to do with this folder', 5);

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  desks: [{
    name: 'Check', id: 'check',
    folders: [{ cwd: WORK, tabs: [{ name: 'claude', id: 'claude', command: 'claude' }] }],
  }],
}, null, 2));

// Started the way the window itself is started, through a shell of its own:
// the copy gets its own home, its own LOCALAPPDATA and the stub CLI on its
// PATH, and the agent's own variables are left behind so that what runs inside
// is not taken for a child of the session that started it. (Handed to
// PowerShell rather than spawned from here because the WebView2 the window
// draws in reads its argument out of the environment it is created with, and
// an environment built by hand in Node did not reach it.)
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
// Started and talked to more than once: this check watches a tab come up
// twice, once knowing nothing and once having lost something, and the second
// is a different copy of the window with a page of its own
const startApp = () => {
  const started = ps('-File', starter);
  if (started.status !== 0) die('the copy did not start:\n' + started.stdout + started.stderr);
};
/** The window's page, once it has one: its DevTools targets, or nothing. */
const page = async (seconds) => {
  // Every turn waits, including the one where the port answers with no page
  // yet: a loop that only sleeps on a refused connection spends its whole
  // budget in a few milliseconds
  for (let i = 0; i < seconds * 2; i++) {
    try {
      const found = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const pages = found.filter((t) => t.type === 'page');
      if (pages.length) return pages;
    } catch { /* not up yet */ }
    await sleep(500);
  }
  return null;
};
/** Start the copy and answer with a way to run things in its page. */
const connect = async () => {
  // A copy started over a folder that was made a moment ago comes up now and
  // then with its window drawn and no debugging port behind it -- the argument
  // that opens one reaches the window through the environment it is created
  // with, and on that first start it sometimes does not. Started again rather
  // than waited on: the second start has never failed to answer, and waiting
  // out a port that is not coming costs the whole run
  let targets;
  for (let attempt = 1; attempt <= 3 && !targets; attempt++) {
    startApp();
    targets = await page(45);
    if (!targets) {
      console.log(`  (start ${attempt} came up without its DevTools port -- starting it again)`);
      stopApp();
      await sleep(2000);
    }
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
  return async (expression) => {
    const r = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
};
let run = await connect();
const until = async (test, what, ms = 40000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};
const hookLog = () => {
  try { return fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8'); } catch { return ''; }
};
const launched = () => {
  try { return fs.readFileSync(ARGV, 'utf8').trim(); } catch { return ''; }
};

try {
  console.log('1. the tab comes up on a conversation of nobody\'s, and says so');
  await until(() => Promise.resolve(/--session-id/.test(launched())), 'the CLI to be launched');
  check(!/--resume/.test(launched()), 'it was not resumed: ' + launched());
  check(hookLog().includes('"claude" starts clean: this desk is not in the last session at all'),
    'the log says the tab came up clean because nothing was remembered');
  await until(() => run('!!(S && S.tabs && S.tabs.some(t => t.past))'), 'the offer to appear');
  check(await run('!!document.querySelector("#panes .pane .past:not([hidden])")'),
    'the caption offers the way back');

  console.log('2. pressing it lists what was said in this folder, newest first');
  await run('document.querySelector("#panes .pane .past").click(); true');
  await until(() => run('!!(S && S.past && S.past.hits && S.past.hits.length)'), 'the list');
  const rows = await run(`Array.from(document.querySelectorAll("#past .vrow")).map(r => ({
    when: r.querySelector(".vwhen").textContent,
    said: r.querySelector(".vname").textContent,
  }))`);
  check(rows.length === 2, `the folder's own two conversations are listed (${rows.length})`);
  check(/sign-in fails/.test(rows[0]?.said || ''), 'the newest is first: ' + JSON.stringify(rows[0]));
  check(rows.every((r) => !/nothing to do/.test(r.said)),
    'a conversation from another folder is not offered');
  check((rows[0]?.said || '').length > 0, 'each row says what was asked in it');

  console.log('3. picking one puts that tab back into it');
  await run('document.querySelectorAll("#past .vrow")[0].click(); true');
  await until(() => Promise.resolve(new RegExp('--resume\\s+' + NEWER).test(launched())),
    'the CLI to be relaunched resuming it');
  check(new RegExp('--resume\\s+' + NEWER).test(launched()),
    'the tab was relaunched resuming the conversation picked: ' + launched());
  check(hookLog().includes('restarted "claude" carrying'), 'the app wrote down what it carried');

  console.log('4. the offer is gone, because the tab is in a conversation now');
  await until(() => run('!!(S && S.tabs && !S.tabs.some(t => t.past))'), 'the offer to go');
  check(await run('!document.querySelector("#panes .pane .past:not([hidden])")'),
    'the caption no longer offers it');

  // The case that costs the day, and the one this app used to be silent about:
  // the tab WAS having a conversation, it is written down, and it does not come
  // back. Set up by remembering one whose record is not on this machine, which
  // is what a thrown-away record, a renamed folder or a shifted tab name all
  // come to in the end
  console.log('5. a tab that lost the conversation written down for it says so');
  stopApp();
  await sleep(1500);
  fs.writeFileSync(path.join(APP, 'data', 'last-session'), JSON.stringify({
    version: 1,
    desks: [{
      name: 'Check', id: 'check',
      tabs: [{
        title: 'claude', id: 'claude', cwd: WORK, program: 'claude',
        session: '44444444-4444-4444-8444-444444444444', source: 'Minted',
      }],
    }],
  }, null, 2));
  fs.rmSync(ARGV, { force: true });
  run = await connect();
  await until(() => Promise.resolve(/--session-id/.test(launched())), 'the CLI to be launched');
  check(!/--resume/.test(launched()), 'it could not be resumed, so it starts clean: ' + launched());
  await until(() => run('!!(S && S.tabs && S.tabs.some(t => t.past && t.lost))'),
    'the offer to appear, saying what happened');
  check(await run('!!document.querySelector("#panes .pane .past.lost:not([hidden])")'),
    'the caption says the conversation did not come back');
  check(hookLog().includes('"claude" starts clean:'),
    'the log says why this tab came up clean');
  const kept = (() => {
    try { return fs.readFileSync(path.join(APP, 'data', 'last-session.prev'), 'utf8'); } catch { return ''; }
  })();
  check(kept.includes('44444444-4444-4444-8444-444444444444'),
    'the start kept a copy of the memory it was handed');
  check(hookLog().includes('last session: 1 tab(s) remembered across 1 desk(s)'),
    'the log says how much the start was handed');

  console.log('6. and it is still offered after the person has typed');
  await run('send({kind:"key", text:"hello"}); send({kind:"key", named:"enter"}); true');
  await sleep(1500);
  check(await run('!!document.querySelector("#panes .pane .past.lost:not([hidden])")'),
    'typing does not take the way back away');

  console.log('7. until it is taken up, which is when it has been seen');
  await run('document.querySelector("#panes .pane .past").click(); true');
  await until(() => run('!!(S && S.past && S.past.hits && S.past.hits.length)'), 'the list');
  await run('document.querySelector("#past .vclose").click(); true');
  await until(() => run('!!(S && S.tabs && !S.tabs.some(t => t.past))'), 'the offer to go');
  check(await run('!document.querySelector("#panes .pane .past:not([hidden])")'),
    'the caption is an ordinary caption again');

  console.log('8. with a page in front of the tab, the list is still the tab\'s own');
  stopApp();
  await sleep(1500);
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'en',
    remote: { enabled: false },
    desks: [{
      name: 'Check', id: 'check',
      folders: [{ cwd: WORK, tabs: [
        { name: 'page', id: 'page', command: 'browser https://example.com/' },
        { name: 'claude', id: 'claude', command: 'claude' },
      ] }],
    }],
  }, null, 2));
  fs.rmSync(path.join(APP, 'data', 'last-session'), { force: true });
  fs.rmSync(ARGV, { force: true });
  run = await connect();
  await until(() => Promise.resolve(/--session-id/.test(launched())), 'the CLI to be launched');
  await until(() => run('!!(S && S.tabs && S.tabs.some(t => t.past))'), 'the offer to appear');
  const numbered = await run('S.tabs.map(t => ({ index: t.index, name: t.name, past: !!t.past }))');
  const offered = numbered.find((t) => t.past);
  check(offered && offered.index === 2,
    'the tab is screen 2, behind the page: ' + JSON.stringify(numbered));
  await run('(() => { const t = S.tabs.find(t => t.past); window.__openPast(t.index, t.name); return true; })()');
  await until(() => run('!!(S && S.past)'), 'an answer to the list');
  const answered = await run('({ tab: S.past.tab, hits: (S.past.hits || []).length })');
  check(answered.tab === 2 && answered.hits === 2,
    'the list is the pressed tab\'s two conversations: ' + JSON.stringify(answered));
  await until(() => run('document.querySelectorAll("#past .vrow").length === 2'), 'the rows');
  await run('document.querySelectorAll("#past .vrow")[0].click(); true');
  await until(() => Promise.resolve(new RegExp('--resume\\s+' + NEWER).test(launched())),
    'the tab to be relaunched resuming it');
  check(new RegExp('--resume\\s+' + NEWER).test(launched()),
    'the pressed tab was relaunched into the conversation picked: ' + launched());
} catch (e) {
  console.error(e.message);
  const tail = hookLog().split('\n').slice(-12).join('\n');
  if (tail.trim()) console.error('--- the end of hooks.log ---\n' + tail);
  failures += 1;
} finally {
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
process.exit(failures ? 1 : 0);
