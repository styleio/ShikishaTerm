/**
 * A past conversation found by the search of every conversation, read in the
 * conversation panel, and picked back up.
 *
 * Starts the app built in this checkout, isolated, over a home with three
 * conversations written the way Claude Code writes them, and uses the search
 * the way a person does: from Find (INDEX, the palette) into the panel's
 * "Every conversation"; a word said after megabytes of tool output found, a
 * word only in a folder not; a row opening the conversation in the panel with
 * the word marked and the way back over it; the tool runs opening; a note
 * finding its conversation and a pin shown; a conversation whose worktree was
 * removed writing nothing until a place is chosen, and its branch opening the
 * worktree dialog.
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
  await run(`(() => { const i = cvUi.q; i.value = ${JSON.stringify(q)}; i.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => run(`!!(S && S.vault && S.vault.query === ${JSON.stringify(q)} && !S.vault.searching)`),
    'the search for ' + q);
  await until(() => run('document.querySelectorAll("#convopanel .vrow").length === S.vault.hits.length'), 'the list to be drawn');
  return run('S.vault.hits.filter(h => h.tab == null).map(h => ({id: h.id, snippet: h.snippet, cwd: h.cwd, at: h.at, pinned: !!h.pinned, noted: !!h.noted}))');
};
const openRow = async (id) => {
  await run(`(() => { const hits = S.vault.hits; const rows = document.querySelectorAll("#convopanel .vrow");
    const i = hits.findIndex(h => h.id === ${JSON.stringify(id)}); rows[i].click(); return true; })()`);
  await until(() => run(`!!(CV.past && CV.past.id === ${JSON.stringify(id)} && !CV.loading && CV.rows.length)`), 'the conversation to be read');
  await until(() => run('!!document.querySelector("#convoHead .hgo")'), 'the way to pick it back up');
};
const configNow = () => fs.readFileSync(CONFIG, 'utf8');

try {
  await until(() => run('!!(S && S.tabs)'), 'the board');

  console.log('1. Find opens the conversation panel on every conversation');
  await run('window.__openVault(); true');
  await until(() => run('!!document.querySelector("#convopanel:not([hidden]) #convoMode button.on")'), 'the panel');
  const opened = await run(`({ mode: document.querySelector("#convoMode button.on").textContent,
    boxes: document.querySelector("#convopanel .cshow").hidden, ph: cvUi.q.placeholder,
    floating: !!document.getElementById("vault") })`);
  check(opened.mode === 'Every conversation' && opened.boxes, 'on every conversation, the boxes put away: ' + JSON.stringify(opened));
  check(!opened.floating, 'there is no floating box any more');

  console.log('2. every record is read all the way through, and only its words are matched');
  const late = await search('quasarbilling');
  check(late.length === 1 && late[0].id === LATE, 'a word after 3 MB of tool output is found: ' + JSON.stringify(late.map((h) => h.id)));
  check(/quasarbilling/.test(late[0]?.snippet || ''), 'the row shows the words it was found in: ' + late[0]?.snippet);
  check(typeof late[0]?.at === 'number', 'and where in the record they are');
  check(await run('!!document.querySelector("#convopanel .vsnip mark.vmark")'), 'the word is marked in the row');
  const who = await run('(document.querySelector("#convopanel .vsnip .vwho") || {}).textContent || ""');
  check(/^AI: /.test(who), 'the row says who said it: ' + who);
  const folder = await search('shop-fix-gone');
  check(folder.length === 0, 'a word only in the folder a conversation ran in finds nothing: ' + JSON.stringify(folder));

  console.log('3. a row opens the conversation in the panel, at the place, the word marked');
  await search('quasarbilling');
  await openRow(LATE);
  const read = await run(`(() => {
    const list = cvUi.list;
    const marks = [...list.querySelectorAll("mark.vmark")];
    return { mode: document.querySelector("#convoMode button.on").textContent, q: cvUi.q.value,
      marks: marks.length, back: (document.querySelector("#convoHead .hback") || {}).textContent || "",
      go: document.querySelector("#convoHead .hgo").textContent, boxes: document.querySelector("#convopanel .cshow").hidden };
  })()`);
  check(read.mode === 'This conversation' && read.q === 'quasarbilling', 'it is read as this conversation, the words kept: ' + JSON.stringify(read));
  check(read.marks >= 1, 'the word is marked in it');
  check(/Every conversation/.test(read.back), 'the way back to the list stands over it');
  check(read.go === 'Resume', 'its folder is there, so Resume is one press: ' + read.go);

  console.log('4. the tool runs open when their kind is shown, cut to what is worth reading');
  // With the words in the box only what holds them is listed, so they are
  // cleared first: the whole conversation, its tool runs among it
  await run('(() => { cvUi.q.value = ""; cvUi.q.dispatchEvent(new Event("input")); return true; })()');
  await run('(() => { const b = cvUi.boxes.work; if (!b.checked) b.click(); return true; })()');
  await until(() => run('!!cvUi.list.querySelector(".vwork")'), 'the tool runs to be listed');
  await run('(() => { const w = cvUi.list.querySelector(".vwork"); if (!w.dataset.filled) w.querySelector(".vmore").click(); return true; })()');
  await until(() => run('!!cvUi.list.querySelector(".vwork[data-filled] .vpiece")'), 'the work to open');
  const work = await run(`[...cvUi.list.querySelectorAll(".vwork .vpiece")].map(p => ({
    kind: p.className, name: (p.querySelector(".vpname") || {}).textContent || "", cut: (p.querySelector(".vpcut") || {}).textContent || "" }))`);
  check(work.some((p) => /call/.test(p.kind) && p.name === 'Bash'), 'the call names its tool: ' + JSON.stringify(work));
  check(work.some((p) => /out/.test(p.kind) && /not shown/.test(p.cut)), 'a long output says how much is left out');

  console.log('5. back is the same list');
  await run('document.querySelector("#convoHead .hback").click(); true');
  // The words in the box carry across: cleared in 4, the list is the recent ones
  await until(() => run('cvAll && S.vault.query === "" && !S.vault.searching'), 'the list');
  check(await run('document.querySelectorAll("#convopanel .vrow").length >= 3'), 'the list is back, over what the box now holds');

  console.log('6. long things are folded, and what holds the word is not');
  await search('marmalade');
  await openRow(LONG);
  // The whole conversation, not only what holds the words
  await run('(() => { cvUi.q.value = ""; cvUi.q.dispatchEvent(new Event("input")); return true; })()');
  await until(() => run('cvUi.list.querySelectorAll(".rturn").length >= 4'), 'the rows');
  const folds = await run(`(() => { const l = cvUi.list;
    return { folded: l.querySelectorAll(".vrtext.vfold").length, code: l.querySelectorAll(".vmore.code").length }; })()`);
  check(folds.folded >= 1, 'the long explanation is folded: ' + JSON.stringify(folds));
  check(folds.code >= 1, 'the long code is folded to one line');

  console.log('7. a note written on a conversation finds it, and a pin is shown on its row');
  fs.writeFileSync(path.join(APP, 'config', 'conversation-marks.json'), JSON.stringify({
    version: 1, marks: [{ record: LONG, at: 0, pinned: true, note: 'remember the zanzibarite rule', made: Date.now(), changed: Date.now() }],
  }));
  await run('document.querySelector("#convoHead .hback").click(); true');
  const noted = await search('zanzibarite');
  check(noted.length === 1 && noted[0].id === LONG && noted[0].noted, 'the note finds its conversation: ' + JSON.stringify(noted));
  check(await run('!!document.querySelector("#convopanel .vrow .vpin")'), 'the row wears the pin');

  console.log('8. a conversation whose worktree is gone writes nothing until a place is chosen');
  const before = configNow();
  await search('zebracorn');
  await openRow(LOST);
  const gone = await run('document.querySelector("#convoHead .hgo").textContent');
  check(gone === 'Resume ▾', 'Resume lists where it can go instead: ' + gone);
  await run('document.querySelector("#convoHead .hgo").click(); true');
  await until(() => run('!!document.querySelector(".fmenu")'), 'the list of places');
  const places = await run('[...document.querySelectorAll(".fmenu > div")].map(d => d.textContent)');
  check(places.some((p) => /The folder this conversation was had in is gone/.test(p) && p.includes('shop-fix-gone')),
    'it says the folder is gone, and which: ' + JSON.stringify(places));
  check(places.some((p) => /Make a worktree from branch fix\/gone again/.test(p)), 'it offers the branch made into a worktree again');
  check(places.some((p) => p.includes(WORK)), 'it offers this desk\'s folder by its path');
  check(configNow() === before, 'nothing was written into the settings');
  await run('document.querySelector(".fmenu .vwhere").click(); true');
  await until(() => Promise.resolve(configNow().includes(LOST)), 'the tab to be written');
  const written = JSON.parse(configNow());
  const tabs = written.desks[0].folders.flatMap((f) => (f.tabs || []).map((t) => ({ cwd: f.cwd, ...t })));
  check(tabs.some((t) => t.resume === LOST && t.cwd === WORK), 'it is written into the folder chosen: ' + JSON.stringify(tabs));
  check(written.desks[0].folders.every((f) => f.cwd !== GONE), 'the gone folder is not written back as a folder');

  console.log('9. the branch made into a worktree again opens the worktree dialog on it');
  await run('window.__openVault(); true');
  await search('zebracorn');
  await openRow(LOST);
  await run('document.querySelector("#convoHead .hgo").click(); true');
  await until(() => run('!!document.querySelector(".fmenu")'), 'the list of places');
  await run(`[...document.querySelectorAll(".fmenu > div")].find(d => /Make a worktree/.test(d.textContent)).click(); true`);
  await until(() => run('!document.getElementById("branch").hidden'), 'the worktree dialog');
  check(await run('document.getElementById("bq").value') === 'fix/gone', 'the dialog is on the branch');
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
