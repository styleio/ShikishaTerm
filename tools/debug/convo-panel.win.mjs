/**
 * The column's conversation panel, end to end, with no account: who sent what,
 * who answered a question, who stopped the AI -- written down by the running
 * app and read back beside the words of the CLI's own record.
 *
 * This checkout's build in a folder of its own, over a home of its own, with
 * two tabs in one folder: a stand-in Claude and a shell. The stand-in is a
 * `claude.cmd` (so the app takes it for Claude Code) running a script that
 * keeps its conversation the way Claude Code does -- one JSON line a message,
 * under `~/.claude/projects/<folder>/<the id it was started with>.jsonl` --
 * and answers each line with a tool run and a reply. A line saying "ask me"
 * makes it ask a question the way Claude asks for a permission; a line saying
 * "slow" keeps it working, "esc to interrupt" on its screen, for a while.
 *
 *     cargo build
 *     node tools/debug/convo-panel.win.mjs [--keep]
 *
 * Checked, through the window's own DevTools port and the board's own door
 * for a remote page:
 *   - the column has a Chat panel, and it says there is nothing yet
 *   - a line sent from the window's input bar reads "You · this PC"
 *   - a line sent from a remote page reads "You · another device"
 *   - a line another tab sent with `shikisha send_to_tab` reads "From @shell",
 *     and is counted as sent by AI, not by the person
 *   - a question answered with a key pressed from a remote page is one row:
 *     how long it waited, and "answered by you (another device)"
 *   - Esc pressed at the window while the AI works is a stop "with Esc by
 *     you (this PC)"
 *   - the boxes hide and show kinds; tool runs open to what the tool was asked
 *     and gave back
 *   - typing narrows the rows at once, and the search reads the whole
 *     conversation (a word only inside a tool's output is found, in a tool run)
 *   - a pin is written to config/conversation-marks.json and "Pinned only"
 *     shows that row alone
 *   - data/conversations.db is where the record is, and holds no words
 *
 * Needs Windows and Node. Nothing of a copy somebody is using is read, written
 * or stopped; the app is stopped on the way out unless --keep is given.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9353;
const BOARD = 9354;
const RUN = path.join(os.tmpdir(), 'sk-convo');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const HOME = path.join(RUN, 'home');
const STUB = path.join(RUN, 'stub');
const CONFIG = path.join(APP, 'config', 'config.json');
const MARKS = path.join(APP, 'config', 'conversation-marks.json');

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

// The stand-in Claude. It keeps its record where Claude Code keeps one, under
// the id the app handed it, and answers every line with a tool run and a reply
fs.writeFileSync(path.join(STUB, 'claude.js'), String.raw`
const fs = require('fs'), path = require('path');
const args = process.argv.slice(2);
const at = Math.max(args.indexOf('--session-id'), args.indexOf('--resume'));
const id = at >= 0 ? args[at + 1] : 'no-id';
const dir = path.join(process.env.USERPROFILE, '.claude', 'projects', 'work');
fs.mkdirSync(dir, { recursive: true });
const file = path.join(dir, id + '.jsonl');
const write = (o) => fs.appendFileSync(file, JSON.stringify(Object.assign({ timestamp: new Date().toISOString(), sessionId: id }, o)) + '\n');
const say = (who, content) => write({ type: who, message: { role: who, content } });
const out = (s) => process.stdout.write(s);
if (process.stdin.isTTY) process.stdin.setRawMode(true);
out('stand-in ready\r\n> ');
let line = '', asking = null, pasting = false;
const answer = (text) => {
  say('assistant', [{ type: 'text', text: 'Looking into it:' }]);
  say('assistant', [{ type: 'tool_use', id: 't1', name: 'Bash', input: { command: 'grep -r parser .' } }]);
  say('user', [{ type: 'tool_result', tool_use_id: 't1', content: 'src/lib.rs: zanzibar-inside-a-tool' }]);
  say('assistant', [{ type: 'text', text: 'Done: ' + text }]);
  out('Done: ' + text + '\r\n> ');
};
const take = (text) => {
  say('user', text);
  if (/ask me/.test(text)) {
    asking = text;
    out('Do you want to proceed?\r\n❯ 1. Yes\r\n  2. No\r\n');
  } else if (/slow/.test(text)) {
    let n = 0;
    const tick = setInterval(() => {
      out('\r✻ Working ' + (n++) + 's (esc to interrupt)   ');
      if (n > 30) { clearInterval(tick); answer(text); }
    }, 500);
    process.stdin.once('data', (d) => {
      if (d.toString().includes('\x1b')) { clearInterval(tick); say('user', '[Request interrupted by user]'); out('\r\nInterrupted\r\n> '); }
    });
  } else answer(text);
};
process.stdin.on('data', (d) => {
  let s = d.toString();
  if (asking) {
    // Answered: the question is taken off the screen, as Claude Code does
    if (s.includes('1')) { const t = asking; asking = null; out('\x1b[3F\x1b[0J'); answer(t); }
    return;
  }
  s = s.replace(/\x1b\[200~/g, () => { pasting = true; return ''; }).replace(/\x1b\[201~/g, () => { pasting = false; return ''; });
  for (const c of s) {
    if (c === '\r' && !pasting) { const t = line.trim(); line = ''; out('\r\n'); if (t) take(t); }
    else if (c === '\x7f') line = line.slice(0, -1);
    else if (c !== '\x1b') { line += c; out(c); }
  }
});
`);
fs.writeFileSync(path.join(STUB, 'claude.cmd'), `@"${process.execPath}" "${path.join(STUB, 'claude.js')}" %*\r\n`);

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: true, bind: '127.0.0.1', port: BOARD },
  desks: [{ name: 'Chat', id: 'chat', folders: [{ cwd: WORK, tabs: [
    { name: 'front', id: 'front', command: [path.join(STUB, 'claude.cmd')] },
    { name: 'shell', id: 'shell', command: 'cmd.exe' },
  ] }] }],
}, null, 2));

const starter = path.join(RUN, 'start.ps1');
fs.writeFileSync(starter, [
  '$ErrorActionPreference = "Stop"',
  `$env:LOCALAPPDATA = "${path.join(RUN, 'localappdata')}"`,
  `$env:USERPROFILE = "${HOME}"`,
  `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=${PORT}"`,
  'foreach ($e in @(Get-ChildItem env: | Where-Object { $_.Name -match "^(CLAUDE|ANTHROPIC|SHIKISHA)" })) { Remove-Item ("env:" + $e.Name) }',
  `Start-Process -FilePath "${path.join(APP, 'SHIKISHA-TERM.exe')}" -ArgumentList '--behind' -WorkingDirectory "${APP}"`,
].join('\r\n') + '\r\n');
const pages = async () => {
  try {
    const found = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
    return found.filter((t) => t.type === 'page' && /127\.0\.0\.1:\d+\/$/.test(t.url));
  } catch { return []; }
};
const connect = async (target) => {
  const {ws, send, run} = await connectCdp(target);
  return { ws, send, run };
};
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};

let board;
try {
  // A copy now and then comes up without its DevTools port; it is started
  // again rather than waited on
  let targets = [];
  for (let attempt = 1; attempt <= 3 && !targets.length; attempt++) {
    const started = ps('-File', starter);
    if (started.status !== 0) die('the copy did not start:\n' + started.stdout + started.stderr);
    const end = Date.now() + 45000;
    while (Date.now() < end && !(targets = await pages()).length) await sleep(500);
    if (!targets.length) { console.log(`  (start ${attempt} came up without its DevTools port -- again)`); stopApp(); await sleep(2000); }
  }
  if (!targets.length) throw new Error('the app\'s page never opened its DevTools port');
  board = await connect(targets[0]);
  const { run } = board;
  // The door a remote page uses, with the key the board wrote for it
  const tokenFile = path.join(APP, 'data', 'remote-token');
  await until(async () => fs.existsSync(tokenFile) && fs.readFileSync(tokenFile, 'utf8').trim(), 'the remote board', 30000);
  const token = fs.readFileSync(tokenFile, 'utf8').trim();
  // Opening the link is what grants a device its session, as a phone does
  // when the QR code is read; what it is sent after that rides on the cookies
  let cookie = '';
  await until(async () => {
    const opened = await fetch(`http://127.0.0.1:${BOARD}/?t=${token}`, { redirect: 'manual' });
    await opened.text();
    cookie = opened.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
    return !!cookie;
  }, 'the remote board to open', 30000);
  const fromAfar = (o) => fetch(`http://127.0.0.1:${BOARD}/api/intent?t=${token}`, {
    method: 'POST', headers: { 'content-type': 'application/json', cookie }, body: JSON.stringify(o),
  }).then(async (r) => (r.status === 200 && (await r.json()).ok ? 200 : r.status));

  await until(() => run(`!!(S.tabs || []).find(t => t.id === "front" && t.ai && t.readable)`), 'the stand-in to be read as an AI', 40000);
  const front = await run(`S.tabs.find(t => t.id === "front").index`);
  const shell = await run(`S.tabs.find(t => t.id === "shell").index`);
  await run(`send({kind:"select", tab:${front}}); true`);
  await until(() => run(`S.active === ${front}`), 'the stand-in in front');

  // Rows as the panel shows them: which kind, and the words over them
  const rows = () => run(`[...document.querySelectorAll("#convopanel .clist [data-key]")].map(e => ({
    key: e.dataset.key, kind: e.className,
    who: (e.querySelector(".cwho") || {}).textContent || "",
    text: ((e.querySelector(".vrtext") || e).textContent || "").trim()
  }))`);
  const row = async (test) => (await rows()).find(test);

  console.log('1. the column has a Chat panel');
  await run(`setSideWidth(360); true`);
  await run(`[...document.querySelectorAll("#side .sbar button")].find(b => b.textContent === "Chat").click(); true`);
  await until(() => run(`!document.getElementById("convopanel").hidden`), 'the panel');
  check(true, 'the Chat panel is in the column');
  await until(() => run(`(document.querySelector("#convopanel .fsay") || {}).textContent === T["convo.empty"]`), 'the empty line', 15000);
  check(true, 'it says nothing has been said yet');

  console.log('2. sent from the window');
  await run(`send({kind:"say", tab:${front}, text:"fix the parser please"}); true`);
  await until(async () => !!(await row((r) => r.text === 'fix the parser please')), 'the line in the panel');
  const mine = await row((r) => r.text === 'fix the parser please');
  check(mine.who.startsWith('You · this PC'), 'it reads ' + JSON.stringify(mine.who));
  await until(async () => !!(await row((r) => r.text.startsWith('Done: fix the parser'))), 'the answer');
  check(true, 'the answer is under it');

  console.log('3. sent from a remote page');
  const frontUid = await run(`S.tabs.find(t => t.index === ${front}).uid`);
  check(await fromAfar({ kind: 'say', tab: front, uid: frontUid, text: 'and the docs too' }) === 200, 'the remote page is heard');
  await until(async () => !!(await row((r) => r.text === 'and the docs too')), 'the remote line');
  check((await row((r) => r.text === 'and the docs too')).who.startsWith('You · another device'), 'it reads "You · another device"');

  console.log('4. sent by another tab');
  // Automation does not type into a tab a person typed into in the last few
  // seconds (MANUAL_GUARD_MS); a hand-off waits that out the way a real one would
  await sleep(6000);
  await run(`send({kind:"say", tab:${shell}, text:'shikisha send_to_tab front "please review the change"'}); true`);
  await until(async () => !!(await row((r) => r.text === 'please review the change')), 'the line another tab sent', 40000);
  const theirs = await row((r) => r.text === 'please review the change');
  check(theirs.who === 'From @shell', 'it reads ' + JSON.stringify(theirs.who));
  check(/\baiin\b/.test(theirs.kind), 'it is counted as sent by AI');
  await run(`send({kind:"select", tab:${front}}); true`);
  await until(() => run(`S.active === ${front}`), 'the stand-in in front again');

  console.log('5. a question answered from a remote page');
  await run(`send({kind:"say", tab:${front}, text:"ask me before you push"}); true`);
  await until(() => run(`S.tabs.find(t => t.id === "front").state === "QUESTION"`), 'the question', 20000);
  await sleep(1500);
  check(await fromAfar({ kind: 'key', text: '1' }) === 200, 'the key from afar is heard');
  await until(() => run(`S.tabs.find(t => t.id === "front").state !== "QUESTION"`), 'the answer', 20000);
  // The row is there while the question is; it says who answered once the
  // panel has read the conversation again
  const answered = async () => { const w = await row((r) => r.key.startsWith('wait@')); return w && /answered by/.test(w.text) ? w : null; };
  await until(async () => !!(await answered()), 'the wait row with its answer', 20000);
  const wait = await answered();
  check(/Waited \d+s for an answer · answered by you \(another device\)/.test(wait.text), 'it reads ' + JSON.stringify(wait.text));

  console.log('6. Esc at the window while it works');
  await run(`send({kind:"say", tab:${front}, text:"a slow job"}); true`);
  await until(() => run(`S.tabs.find(t => t.id === "front").state === "BUSY"`), 'the AI at work', 20000);
  await run(`send({kind:"key", named:"escape"}); true`);
  await until(async () => !!(await row((r) => r.key.startsWith('stop@'))), 'the stop row', 20000);
  const stop = await row((r) => r.key.startsWith('stop@'));
  check(/with Esc by you \(this PC\)/.test(stop.text), 'it reads ' + JSON.stringify(stop.text));

  console.log('7. the boxes');
  const box = (kind) => `document.querySelectorAll("#convopanel .cshow input")[${['you', 'aiin', 'ai', 'work', 'events'].indexOf(kind)}]`;
  await run(`${box('ai')}.click(); true`);
  await sleep(300);
  check(!(await rows()).some((r) => /\bai\b/.test(r.kind) && /rturn/.test(r.kind)), 'AI replies put away');
  await run(`${box('ai')}.click(); ${box('work')}.click(); true`);
  await until(async () => (await rows()).some((r) => r.key.startsWith('w')), 'tool runs shown');
  await run(`document.querySelector("#convopanel .vwork .vmore").click(); true`);
  await until(() => run(`!!document.querySelector("#convopanel .vwork .vpiece")`), 'the tool run to open');
  check(await run(`document.querySelector("#convopanel .vwork .vpname").textContent`) === 'Bash', 'the tool run opens to what it called');
  await run(`${box('work')}.click(); true`);

  console.log('8. the search');
  await run(`(() => { const q = document.querySelector("#convopanel .fsearch input"); q.value = "docs"; q.dispatchEvent(new Event("input")); return true; })()`);
  await sleep(100);
  const narrowed = await rows();
  check(narrowed.length > 0 && narrowed.every((r) => /docs/i.test(r.text)), 'narrowed at once: ' + narrowed.length + ' rows');
  await run(`(() => { const q = document.querySelector("#convopanel .fsearch input"); q.value = "zanzibar"; q.dispatchEvent(new Event("input")); return true; })()`);
  await until(() => run(`(document.querySelector("#convopanel .fsay").textContent || "").includes("kinds that are not shown")`), 'the hidden match to be said', 15000);
  check(true, 'a word only in a tool\'s output is found, and said to be in a kind not shown');
  await run(`document.querySelector("#convopanel .fsay .cshowall").click(); true`);
  await until(() => run(`!!document.querySelector("#convopanel .vwork mark.vmark")`), 'the tool run with the word marked', 15000);
  check(true, 'shown, it opens with the word marked');
  await run(`(() => { const q = document.querySelector("#convopanel .fsearch input"); q.value = ""; q.dispatchEvent(new Event("input")); return true; })()`);
  await run(`${box('work')}.checked && ${box('work')}.click(); true`);

  console.log('9. a pin');
  await until(async () => !!(await row((r) => r.text === 'fix the parser please')), 'the row to pin');
  await run(`document.querySelector('#convopanel .rturn[data-key^="s"] .cpin') && [...document.querySelectorAll("#convopanel .rturn")].find(e => e.querySelector(".vrtext").textContent.trim() === "fix the parser please").querySelector(".cpin").click(); true`);
  await until(async () => fs.existsSync(MARKS) && JSON.parse(fs.readFileSync(MARKS, 'utf8')).marks.some((m) => m.pinned), 'the pin to be written', 10000);
  check(true, 'written to config/conversation-marks.json');
  await run(`document.querySelectorAll("#convopanel .cshow input")[5].click(); true`);
  await until(async () => { const r = await rows(); return r.length === 1 && r[0].text === 'fix the parser please'; }, '"Pinned only"', 15000);
  check(true, '"Pinned only" shows that row alone');

  console.log('10. a word only in a note');
  await run(`document.querySelectorAll("#convopanel .cshow input")[5].click(); true`);
  await until(async () => (await rows()).length > 1, 'every row back');
  await run(`CV.q = ""; convoAsk("mark", {record: CV.rows.find(r => r.text === "and the docs too").record,
    at: CV.rows.find(r => r.text === "and the docs too").at, note: "the quokka ticket"}); true`);
  await until(async () => JSON.parse(fs.readFileSync(MARKS, 'utf8')).marks.some((m) => m.note === 'the quokka ticket'), 'the note to be written', 10000);
  // Reading the conversation again from nothing: only the search can find it
  await run(`CV.rows = []; CV.rev++; (() => { const q = document.querySelector("#convopanel .fsearch input"); q.value = "quokka"; q.dispatchEvent(new Event("input")); })(); true`);
  await until(async () => { const r = await rows(); return r.length === 1 && r[0].text === 'and the docs too'; }, 'the row the note is on', 15000);
  check(true, 'the search finds the thing said by the words of its note');
  await run(`(() => { const q = document.querySelector("#convopanel .fsearch input"); q.value = ""; q.dispatchEvent(new Event("input")); convoRefresh(); })(); true`);

  console.log('11. a line found on a tab\'s screen, from the search of every conversation');
  // The shell's own line, pushed well up its history
  await run(`send({kind:"say", tab:${shell}, text:"for /l %i in (1,1,90) do @echo filler %i"}); true`);
  await sleep(3000);
  await run(`send({kind:"select", tab:${shell}}); true`);
  await until(() => run(`S.active === ${shell} && document.getElementById("screen").textContent.includes("filler 90")`), 'the shell at its newest line', 10000);
  check(!(await run(`document.getElementById("screen").textContent.includes("please review the change")`)), 'the line has scrolled out of sight');
  await run(`send({kind:"select", tab:${front}}); true`);
  await until(() => run(`S.active === ${front}`), 'the stand-in in front');
  await run(`convoModeTo(true); (() => { const q = document.querySelector("#convopanel .fsearch input"); q.value = "please review the change"; q.dispatchEvent(new Event("input")); })(); true`);
  const onScreen = `[...document.querySelectorAll("#convopanel .vrow")].find(r => r.textContent.includes("shell") && r.textContent.includes(T["vault.live"]))`;
  await until(() => run(`!!${onScreen}`), 'the shell\'s screen in the list', 30000);
  await run(`${onScreen}.click(); true`);
  await until(() => run(`S.active === ${shell}`), 'the shell in front', 10000);
  check(await run(`cvAll`), 'the list stays, for the next one, and nothing says the shell is not an AI tab');
  await until(() => run(`document.getElementById("screen").textContent.includes("please review the change")`), 'the line in sight', 10000);
  check(true, 'the shell\'s terminal is scrolled to the line it was found on');
  console.log('11b. a conversation opened from the list, and the way back to this tab\'s');
  await run(`send({kind:"select", tab:${front}}); true`);
  await until(() => run(`S.active === ${front}`), 'the stand-in in front');
  const fromRecord = `[...document.querySelectorAll("#convopanel .vrow")].find(r => !r.textContent.includes(T["vault.live"]))`;
  await until(() => run(`!!${fromRecord}`), 'a conversation from the records in the list', 30000);
  await run(`${fromRecord}.click(); true`);
  await until(() => run(`!!CV.past && !cvAll`), 'the conversation opened', 15000);
  const back = `[...document.querySelectorAll("#convoHead .hback")].find(b => b.textContent === T["convo.back.tab"])`;
  check(await run(`!!${back}`), 'there is a way back to this tab\'s conversation');
  await run(`${back}.click(); true`);
  await until(() => run(`!CV.past && CV.panel === "front"`), 'this tab\'s conversation again', 15000);
  check(true, 'it goes back to this tab\'s conversation');
  await run(`send({kind:"select", tab:${front}}); convoModeTo(false); (() => { const q = document.querySelector("#convopanel .fsearch input"); q.value = ""; q.dispatchEvent(new Event("input")); })(); true`);
  await until(() => run(`S.active === ${front}`), 'the stand-in in front again');

  console.log('12. the record');
  const db = path.join(APP, 'data', 'conversations.db');
  check(fs.existsSync(db), 'data/conversations.db is there');
  const bytes = fs.readFileSync(db).toString('latin1') + (fs.existsSync(db + '-wal') ? fs.readFileSync(db + '-wal').toString('latin1') : '');
  check(!bytes.includes('fix the parser please') && !bytes.includes('and the docs too'), 'it holds none of the words sent');
} catch (e) {
  failures += 1;
  console.error('stopped: ' + e.message);
} finally {
  try { board?.ws.close(); } catch {}
  if (!process.argv.includes('--keep')) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
