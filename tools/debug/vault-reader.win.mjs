/**
 * A past conversation found by Find, read whole, and picked back up.
 *
 * Starts the app built in this checkout, isolated, over a home with three
 * conversations written the way Claude Code writes them, and uses Find the way
 * a person does:
 *
 *   - a word said after megabytes of tool output is found (every record is
 *     read all the way through), and a word that is only in the folder a
 *     conversation ran in finds nothing (only what a reader sees is matched)
 *   - pressing a result opens the conversation in the same box, from its first
 *     word to its last, with the word marked, brought into view, and walked by
 *     the arrows at the foot; the work between a question and its answer is
 *     one line that opens when pressed
 *   - long things said and long code are folded, and what holds the word is not
 *   - Back returns to the same list, scrolled where it was
 *   - Resume on a conversation whose folder is there reopens it in that
 *     folder; on one whose worktree was removed it writes nothing into the
 *     settings and offers the branch made into a worktree again, and the
 *     desk's folders, by name and path
 *
 *     cargo build
 *     node tools/debug/vault-reader.win.mjs
 *
 * Needs Windows, Node and git. Isolated the way a new-user run is: its own
 * folder, its own LOCALAPPDATA, its own home, and a stub CLI on its PATH -- so
 * no account is used and nothing leaves the machine. Nothing of a copy somebody
 * is using is read, written or stopped. The app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9351;
const RUN = path.join(os.tmpdir(), 'sk-vaultread');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work', 'shop');
const GONE = path.join(RUN, 'work', 'shop-fix-gone');
const HOME = path.join(RUN, 'home');
const STUB = path.join(RUN, 'stub');
const CONFIG = path.join(APP, 'config', 'config.json');
const ARGV = path.join(RUN, 'argv.txt');

const LATE = '11111111-1111-4111-8111-111111111111';
const LONG = '22222222-2222-4222-8222-222222222222';
const LOST = '33333333-3333-4333-8333-333333333333';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const git = (cwd, ...args) => spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8' });
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

fs.writeFileSync(path.join(STUB, 'claude.cmd'),
  '@echo off\r\n'
  + `echo %*> "${ARGV}"\r\n`
  + ':wait\r\n'
  + 'timeout /t 60 /nobreak >nul\r\n'
  + 'goto wait\r\n');

// The project, with the branch of a worktree that has since been removed:
// removing a worktree takes its folder and leaves its branch
git(WORK, 'init', '-q', '-b', 'main');
git(WORK, '-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '--allow-empty', '-m', 'start');
git(WORK, 'branch', 'fix/gone');

// Three conversations, written the way Claude Code writes them
const records = path.join(HOME, '.claude', 'projects', 'shop');
fs.mkdirSync(records, { recursive: true });
const line = (who, cwd, content, branch = 'main') => JSON.stringify({
  type: who, cwd, gitBranch: branch, message: { role: who, content },
});
const text = (t) => [{ type: 'text', text: t }];
const call = (cwd, name, command) => JSON.stringify({
  type: 'assistant', cwd, message: { role: 'assistant', content: [{ type: 'tool_use', id: 'toolu_1', name, input: { command } }] },
});
const result = (cwd, out) => JSON.stringify({
  type: 'user', cwd, message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'toolu_1', content: out }] },
});
const write = (id, lines, minutesAgo) => {
  const at = path.join(records, id + '.jsonl');
  fs.writeFileSync(at, lines.join('\n') + '\n');
  const when = new Date(Date.now() - minutesAgo * 60000);
  fs.utimesSync(at, when, when);
};
// The word looked for comes after three megabytes of a tool's output
write(LATE, [
  line('user', WORK, 'the checkout page is slow'),
  line('assistant', WORK, text('Let me look at the logs.')),
  call(WORK, 'Bash', 'tail -n 50000 app.log'),
  result(WORK, 'GET /checkout 200\n'.repeat(180000)),
  line('assistant', WORK, text('The cart total is computed by the quasarbilling module on every request.')),
  line('user', WORK, 'cache it'),
  line('assistant', WORK, text('Cached.')),
], 30);
// Long things said, and a long block of code
const para = Array.from({ length: 40 }, (_, i) => `Line ${i + 1} of a long explanation.`).join('\n');
const code = '```js\n' + Array.from({ length: 30 }, (_, i) => `const v${i} = ${i};`).join('\n') + '\n```';
write(LONG, [
  line('user', WORK, 'explain the pricing rules'),
  line('assistant', WORK, text(para + '\n\n' + code)),
  line('user', WORK, 'thanks, and the tax part is marmalade-free'),
  line('assistant', WORK, text('Understood.')),
], 20);
// Had in a worktree that has since been removed
write(LOST, [
  line('user', GONE, 'rename the zebracorn helper', 'fix/gone'),
  line('assistant', GONE, text('Renamed the zebracorn helper to formatPrice.'), 'fix/gone'),
], 10);

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
const settings = {
  language: 'en',
  remote: { enabled: false },
  desks: [{
    name: 'Check', id: 'check',
    folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd' }] }],
  }],
};
fs.writeFileSync(CONFIG, JSON.stringify(settings, null, 2));

const starter = path.join(RUN, 'start.ps1');
fs.writeFileSync(starter, [
  '$ErrorActionPreference = "Stop"',
  `$env:LOCALAPPDATA = "${path.join(RUN, 'localappdata')}"`,
  `$env:USERPROFILE = "${HOME}"`,
  `$env:PATH = "${STUB};" + $env:PATH`,
  `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=${PORT}"`,
  'foreach ($e in @(Get-ChildItem env: | Where-Object { $_.Name -match "^(CLAUDE|ANTHROPIC)" })) { Remove-Item ("env:" + $e.Name) }',
  `Start-Process -FilePath "${path.join(APP, 'SHIKISHA-TERM.exe')}" -ArgumentList '--behind' -WorkingDirectory "${APP}"`,
].join('\r\n') + '\r\n');
const page = async (seconds) => {
  for (let i = 0; i < seconds * 2; i++) {
    try {
      const found = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const pages = found.filter((t) => t.type === 'page' && /127\.0\.0\.1:\d+\/$/.test(t.url));
      if (pages.length) return pages;
    } catch { /* not up yet */ }
    await sleep(500);
  }
  return null;
};
const connect = async () => {
  let targets;
  for (let attempt = 1; attempt <= 3 && !targets; attempt++) {
    const started = ps('-File', starter);
    if (started.status !== 0) die('the copy did not start:\n' + started.stdout + started.stderr);
    targets = await page(45);
    if (!targets) { console.log(`  (start ${attempt} came up without its DevTools port -- again)`); stopApp(); await sleep(2000); }
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
  const callCdp = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  return async (expression) => {
    const r = await callCdp('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
};
const run = await connect();
const until = async (test, what, ms = 40000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};
const search = async (q) => {
  await run(`(() => { const i = document.getElementById("vq"); i.value = ${JSON.stringify(q)};
    i.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => run(`!!(S && S.vault && S.vault.query === ${JSON.stringify(q)} && !S.vault.searching)`),
    'the search for ' + q);
  return run('S.vault.hits.filter(h => h.tab == null).map(h => ({id: h.id, snippet: h.snippet, cwd: h.cwd}))');
};
const openRow = async (id) => {
  await run(`(() => { const hits = S.vault.hits; const rows = document.querySelectorAll("#vault .vrow");
    const i = hits.findIndex(h => h.id === ${JSON.stringify(id)}); rows[i].click(); return true; })()`);
  await until(() => run('!!document.querySelector("#vault .vredge.end")'), 'the conversation to be read');
};
const configNow = () => fs.readFileSync(CONFIG, 'utf8');

try {
  await until(() => run('!!(S && S.tabs)'), 'the board');
  await run('window.__openVault(); true');

  console.log('1. every record is read all the way through, and only its words are matched');
  const late = await search('quasarbilling');
  check(late.length === 1 && late[0].id === LATE, 'a word after 3 MB of tool output is found: ' + JSON.stringify(late.map((h) => h.id)));
  check(/quasarbilling/.test(late[0]?.snippet || ''), 'the row shows the words it was found in: ' + late[0]?.snippet);
  check(await run('!!document.querySelector("#vault .vsnip mark.vmark")'), 'the word is marked in the row');
  const folder = await search('shop-fix-gone');
  check(folder.length === 0, 'a word only in the folder a conversation ran in finds nothing: ' + JSON.stringify(folder));

  console.log('2. a result opens the whole conversation, the word marked and in view');
  await search('quasarbilling');
  await run('document.querySelector("#vault .vlist").scrollTop = 0; true');
  await openRow(LATE);
  const read = await run(`(() => {
    const body = document.querySelector("#vault .vrbody");
    const marks = [...body.querySelectorAll("mark.vmark")];
    const cur = body.querySelector("mark.vmark.cur");
    const r = cur && cur.getBoundingClientRect(), b = body.getBoundingClientRect();
    return {
      turns: [...body.querySelectorAll(".rturn")].map(t => t.querySelector(".vrtext").textContent.slice(0, 40)),
      marks: marks.length, inView: !!r && r.top >= b.top && r.bottom <= b.bottom,
      count: document.querySelector("#vault .vrcount").textContent,
      listHidden: getComputedStyle(document.querySelector("#vault .vlist")).display === "none",
      start: !!body.querySelector(".vredge"), work: [...body.querySelectorAll(".vwork .vmore")].map(b => b.textContent),
      go: document.querySelector("#vault .vrgo").textContent,
    };
  })()`);
  check(read.turns.length === 4 && /^the checkout page/.test(read.turns[0]) && read.turns[3] === "Cached.",
    'everything said is there, first to last: ' + JSON.stringify(read.turns));
  check(read.start, 'it says where the conversation starts and ends');
  check(read.marks === 1 && read.inView, 'the word is marked and in view: ' + JSON.stringify(read));
  check(/1 of 1/.test(read.count), 'the foot counts the matches: ' + read.count);
  check(read.listHidden, 'the list is put away while reading');
  check(read.work.length === 1 && /Tool runs \(1\)/.test(read.work[0]), 'the work between is one line: ' + JSON.stringify(read.work));
  check(read.go === 'Resume', 'the folder is there, so Resume is one press: ' + read.go);

  console.log('3. the work opens when pressed, cut to what is worth reading');
  await run('document.querySelector("#vault .vwork .vmore").click(); true');
  await until(() => run('!!document.querySelector("#vault .vwork[data-filled] .vpiece")'), 'the work to open');
  const work = await run(`[...document.querySelectorAll("#vault .vwork .vpiece")].map(p => ({
    kind: p.className, name: (p.querySelector(".vpname") || {}).textContent || "", cut: (p.querySelector(".vpcut") || {}).textContent || "" }))`);
  check(work.some((p) => /call/.test(p.kind) && p.name === 'Bash'), 'the call names its tool: ' + JSON.stringify(work));
  check(/say/.test(work[0]?.kind || ''), 'what was said on the way to the tool is part of the work, not the answer');
  check(work.some((p) => /out/.test(p.kind) && /not shown/.test(p.cut)), 'a long output says how much is left out');

  console.log('4. Back is the same list, where it was');
  await run('document.querySelector("#vault .vrback").click(); true');
  const back = await run(`({ rows: document.querySelectorAll("#vault .vrow").length,
    reading: document.getElementById("vault").classList.contains("reading"), q: document.getElementById("vq").value })`);
  check(!back.reading && back.rows === 1 && back.q === 'quasarbilling', 'the list and the search are as they were: ' + JSON.stringify(back));

  console.log('5. long things are folded, and what holds the word is not');
  await search('marmalade');
  await openRow(LONG);
  const folds = await run(`(() => { const body = document.querySelector("#vault .vrbody");
    return { folded: body.querySelectorAll(".vrtext.vfold").length, code: body.querySelectorAll(".vmore.code").length,
      hitFolded: !!body.querySelector(".rturn[data-hit] .vfold"),
      more: [...body.querySelectorAll(".vmore:not(.code)")].map(b => b.textContent) }; })()`);
  check(folds.folded === 1, 'the long explanation is folded: ' + JSON.stringify(folds));
  check(folds.code === 1, 'the long code is folded to one line');
  check(!folds.hitFolded, 'what holds the word is left open');
  check(folds.more.some((m) => /Show more \(\d+ more lines\)/.test(m)), 'the fold says how much is under it: ' + JSON.stringify(folds.more));
  await run('document.querySelector("#vault .vrback").click(); true');

  console.log('6. a conversation whose worktree is gone is not written back into the settings');
  const before = configNow();
  await search('zebracorn');
  await openRow(LOST);
  const gone = await run('document.querySelector("#vault .vrgo").textContent');
  check(gone === 'Resume ▾', 'Resume lists where it can go instead: ' + gone);
  await run('document.querySelector("#vault .vrgo").click(); true');
  await until(() => run('!!document.querySelector(".fmenu")'), 'the list of places');
  const places = await run('[...document.querySelectorAll(".fmenu > div")].map(d => d.textContent)');
  check(places.some((p) => /The folder this conversation was had in is gone/.test(p) && p.includes('shop-fix-gone')),
    'it says the folder is gone, and which: ' + JSON.stringify(places));
  check(places.some((p) => /Make a worktree from branch fix\/gone again/.test(p)), 'it offers the branch made into a worktree again');
  check(places.some((p) => /Only committed work comes back/.test(p)), 'it says what comes back and what does not');
  check(places.some((p) => p.includes(WORK)), 'it offers this desk\'s folder by its path');
  check(configNow() === before, 'nothing was written into the settings');
  await run('closeFolderMenu(); true');

  console.log('7. the gone folder\'s conversation resumed in a folder that is there');
  await run(`[...document.querySelectorAll("#vault .vrgo")][0].click(); true`);
  await until(() => run('!!document.querySelector(".fmenu .vwhere")'), 'the list of places');
  await run('document.querySelector(".fmenu .vwhere").click(); true');
  await until(() => Promise.resolve(configNow().includes(LOST)), 'the tab to be written');
  const written = JSON.parse(configNow());
  const tabs = written.desks[0].folders.flatMap((f) => (f.tabs || []).map((t) => ({ cwd: f.cwd, ...t })));
  check(tabs.some((t) => t.resume === LOST && t.cwd === WORK), 'it is written into the folder chosen: ' + JSON.stringify(tabs));
  check(written.desks[0].folders.every((f) => f.cwd !== GONE), 'the gone folder is not written back as a folder');
  await until(() => Promise.resolve(new RegExp('--resume\\s+' + LOST).test(fs.existsSync(ARGV) ? fs.readFileSync(ARGV, 'utf8') : '')),
    'the CLI to be started resuming it');
  check(true, 'the CLI was started resuming the conversation');

  console.log('8. the branch made into a worktree again opens the worktree dialog on it');
  await run('window.__openVault(); true');
  await search('zebracorn');
  await openRow(LOST);
  await run('document.querySelector("#vault .vrgo").click(); true');
  await until(() => run('!!document.querySelector(".fmenu")'), 'the list of places');
  await run(`[...document.querySelectorAll(".fmenu > div")].find(d => /Make a worktree/.test(d.textContent)).click(); true`);
  await until(() => run('!document.getElementById("branch").hidden'), 'the worktree dialog');
  const dialog = await run(`({ name: document.getElementById("bq").value, vault: !document.getElementById("vault").hidden })`);
  check(dialog.name === 'fix/gone' && !dialog.vault, 'the dialog is on the branch, and Find is put away: ' + JSON.stringify(dialog));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
