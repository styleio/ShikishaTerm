/**
 * Searching the whole folder, and opening a file by a few letters of its
 * name (Ctrl+P), through the board in a browser at a phone's width -- the
 * same page the window shows.
 *
 * This checkout's build in a folder of its own, a repository of a few files
 * and a shell tab standing in it. Checked:
 *   - "Inside" lists every place the words are, by file, with the words lit,
 *     and says how many places in how many files
 *   - whole word, case and regular expression read the words as asked; a
 *     broken pattern says why
 *   - a pattern of files leaves files out
 *   - pressing a place opens the file in the editor at that line
 *   - Ctrl+Shift+P over the terminal (Ctrl+P is the shell's own there) opens
 *     the list of files; a few letters find the file, "name:3" opens it at
 *     line 3, and the file opened last comes first with nothing typed
 *   - the page throws nothing
 *
 *     cargo build
 *     node tools/debug/project-search.win.mjs
 *
 * Takes about a minute. Needs Windows, Node, git and Chrome. Nothing of a copy
 * somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-project-search');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const PORT = 9487;
const TOKEN = 'search-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-project-search*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [path.join(APP, 'config'), path.join(WORK, 'src', 'deep'), path.join(WORK, 'dist')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// A small repository: the words in several files, one of them twice on a
// line, one inside a longer word, one in a file a pattern leaves out
fs.writeFileSync(path.join(WORK, 'src', 'routine.rs'), 'fn start() {}\nlet total = 1; // total again\nfn subtotal_test() {}\n');
fs.writeFileSync(path.join(WORK, 'src', 'deep', 'runtime.rs'), 'pub fn Total() -> u32 { 0 }\n');
fs.writeFileSync(path.join(WORK, 'dist', 'bundle.js'), 'var total=1;\n');
fs.writeFileSync(path.join(WORK, 'notes.md'), '# Notes\n\nNothing here.\n');
// Bigger than the editor opens: not looked in, and said so
fs.writeFileSync(path.join(WORK, 'huge.log'), 'total\n' + 'x'.repeat(5 * 1024 * 1024));
spawnSync('git', ['init', '-q'], { cwd: WORK });

fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en',
  // Asked once, here answered: the question would stand over the panel
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'One', id: 'one', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe'] }] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX|OPENAI)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const jar = new Map();
const send = async (url) => {
  const r = await fetch(BASE + url, { redirect: 'manual', headers: { Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') }, signal: AbortSignal.timeout(15000) });
  for (const kv of r.headers.getSetCookie().map((c) => c.split(';')[0])) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return r;
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(400); }
  throw new Error('timed out waiting for ' + what);
};

let chrome;
try {
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500, 'the board', 60000);
  chrome = await startChrome();
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 420, height: 900, deviceScaleFactor: 2, mobile: true });
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  const run = (js) => chrome.run(js);
  await until(() => run(`typeof S !== "undefined" && !!S && !!folderTab()`), 'the board with the shell tab in front', 30000);
  await run(`sideReveal("files")`);
  await until(() => run(`!!(fsUi && fsUi.q)`), 'the file list', 20000);
  // "Inside", as a person does it: the mode, then the words
  const search = (q) => run(`(() => { fsUi.modes.text.click(); fsUi.q.value = ${JSON.stringify(q)}; fsUi.q.dispatchEvent(new Event("input")); })()`);
  const shown = () => run(`(() => ({
    files: [...document.querySelectorAll("#filepanel .flist .frow")].map(r => r.title + "#" + (r.querySelector(".cnt") || {}).textContent),
    spots: [...document.querySelectorAll("#filepanel .fspot")].map(r => r.querySelector(".ln").textContent + ":" + r.querySelector("mark").textContent),
    say: document.querySelector("#filepanel .fsay").textContent }))()`);
  const settled = (want) => until(async () => { const s = await shown(); return want(s) && s; }, 'the places', 20000).catch(async () => shown());

  await search('total');
  let s = await settled((x) => x.spots.length >= 4);
  check(s.files.length === 3 && s.files.includes('src/routine.rs#3'), `every file with the words, with how many places (${s.files.join(', ')})`);
  check(s.spots.includes('2:total') && s.spots.filter((x) => x.startsWith('2:')).length === 2, `two places on one line are two rows, the words lit (${s.spots.join(', ')})`);
  check(/5 places in 3 files/.test(s.say), `the foot says how many places in how many files (${s.say})`);
  check(/Files over 4 MB, not looked in: 1/.test(s.say), `a file too big to look in is said (${s.say})`);

  // Whole word: not inside "subtotal"; case: only "Total"
  await run(`fsUi.optBtn.word.click()`);
  s = await settled((x) => x.spots.length === 4);
  check(s.spots.length === 4 && !s.spots.some((x) => x.startsWith('3:')), `whole word leaves out "subtotal" (${s.spots.join(', ')})`);
  await run(`fsUi.optBtn.word.click(); fsUi.optBtn.case.click()`);
  await search('Total');
  s = await settled((x) => x.spots.length === 1);
  check(s.spots.length === 1 && s.files[0] === 'src/deep/runtime.rs#1', `case told apart finds only "Total" (${s.files.join(', ')})`);
  await run(`fsUi.optBtn.case.click(); fsUi.optBtn.regex.click()`);
  await search('fn \\w+_test');
  s = await settled((x) => x.spots.length === 1);
  check(s.spots[0] === '3:fn subtotal_test', `a regular expression is read as one (${s.spots.join(', ')})`);
  await search('fn (');
  s = await settled((x) => /regular expression cannot be read/.test(x.say));
  check(/regular expression cannot be read/.test(s.say), `a broken pattern says why (${s.say})`);
  await run(`fsUi.optBtn.regex.click()`);

  // A pattern of files leaves the bundle out
  await run(`(() => { const i = fsUi.which.querySelectorAll("input")[1]; i.value = "dist"; i.dispatchEvent(new Event("input")); })()`);
  await search('total');
  s = await settled((x) => x.files.length === 2);
  check(s.files.length === 2 && !s.files.some((f) => f.startsWith('dist/')), `"Leave out" leaves the folder out (${s.files.join(', ')})`);

  // Pressing a place opens the file there
  await run(`[...document.querySelectorAll("#filepanel .fspot")].find(r => r.querySelector(".ln").textContent === "2").click()`);
  // Waited for until it is there: the text and the place arrive in turn
  const opened = await until(async () => { const r = await run(`(typeof ED !== "undefined" && ED.path === "src/routine.rs" && edAce && !ED.loading) ? edAce.getCursorPosition().row + 1 : 0`); return r === 2 && r; }, 'the editor at the line', 20000).catch(() => run(`edAce ? edAce.getCursorPosition().row + 1 : 0`));
  check(opened === 2, `the file opens at the place pressed (line ${opened})`);
  if (opened !== 2) console.log("   debug", await run(`JSON.stringify({goto: ED.goto, path: ED.path, loading: ED.loading, reading: ED.reading, lines: edAce && edAce.session.getLength(), row: edAce && edAce.getCursorPosition().row})`));

  // Written by somebody else while it is open: read again, and the place
  // it was being read at kept
  fs.appendFileSync(path.join(WORK, 'src', 'routine.rs'), '// written by the AI next door\n');
  const kept = await until(() => run(`(ED.text || "").includes("next door") && !ED.loading ? edAce.getCursorPosition().row + 1 : 0`), 'the file read again', 20000).catch(() => 0);
  check(kept === 2, `a file written by somebody else is read again, its place kept (line ${kept})`);

  // Ctrl+Shift+P over the terminal: the list of files
  await run(`send({kind:"select", tab: S.tabs.find(t => t.name === "shell").index})`);
  await until(() => run(`(activeTab() || {}).kind === "pty"`), 'the shell in front', 10000);
  const key = (shift) => run(`document.dispatchEvent(new KeyboardEvent("keydown", {key: "p", code: "KeyP", ctrlKey: true, shiftKey: ${shift}, bubbles: true, cancelable: true}))`);
  await key(false);
  const plainP = await run(`!document.getElementById("palette").hidden`);
  check(!plainP, 'Ctrl+P over the terminal is left to the shell');
  await key(true);
  const open = await until(() => run(`!document.getElementById("palette").hidden && palMode === "files"`), 'the list of files', 5000).catch(() => false);
  check(open, 'Ctrl+Shift+P over the terminal opens the list of files');
  const type = (q) => run(`(() => { const q = document.getElementById("pq"); q.value = ${JSON.stringify(q)}; q.dispatchEvent(new Event("input")); })()`);
  // Nothing typed: the file opened last
  const recent = await until(() => run(`palItems.length && palItems[0].grp === "recent" ? palItems[0].label : ""`), 'the files opened lately', 5000).catch(() => '');
  check(recent === 'routine.rs', `with nothing typed, the file opened last comes first (${recent})`);
  await type('rntm');
  const found = await until(() => run(`palItems.map(i => (i.sub ? i.sub + "/" : "") + i.label).join(",")`).then((v) => /runtime\.rs/.test(v) && v), 'files that fit', 10000).catch(() => '');
  check(found.split(',')[0] === 'src/deep/runtime.rs', `a few letters find the file meant (${found || JSON.stringify(await run('palItems.map(i => i.label)'))})`);
  await type('runtime:1');
  await until(() => run(`palItems.length && palItems[0].label === "runtime.rs"`), 'the file again', 10000);
  await run(`(() => { const q = document.getElementById("pq"); q.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); })()`);
  const at = await until(() => run(`(ED.path === "src/deep/runtime.rs" && edAce && !ED.loading) ? edAce.getCursorPosition().row + 1 : 0`), 'the file opened by name', 20000).catch(() => 0);
  check(at === 1 && await run(`document.getElementById("palette").hidden`), `Enter opens it at the line asked, and the list goes (line ${at})`);
  // Enter before the answer for new letters has come opens nothing old
  await run(`window.__openPalette("files")`);
  await type('rout');
  await until(() => run(`palItems.length && palItems[0].label === "routine.rs"`), 'the first answer', 10000);
  const stale = await run(`(() => { const q = document.getElementById("pq"); q.value = "notes:3"; q.dispatchEvent(new Event("input"));
    const before = palItems.length; q.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); return {before, path: ED.path}; })()`);
  check(stale.before === 0 && stale.path !== 'src/routine.rs:3', `Enter while the new letters are being looked up opens nothing old (${JSON.stringify(stale)})`);
  // A Markdown file opened at a line is shown as its text there
  await until(() => run(`palItems.length && palItems[0].label === "notes.md"`), 'notes.md', 10000);
  await run(`(() => { const q = document.getElementById("pq"); q.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); })()`);
  const md = await until(async () => { const r = await run(`(ED.path === "notes.md" && edAce && !ED.loading) ? {view: MD.view["notes.md"], row: edAce.getCursorPosition().row + 1} : null`); return r && r.row === 3 && r; }, 'notes.md at line 3', 20000).catch(() => null);
  check(md && md.view === 'source', `a Markdown file opened at a line shows its text at that line (${JSON.stringify(md)})`);
  if (at !== 1) console.log("   debug", await run(`JSON.stringify({goto: ED.goto, path: ED.path, key: ED.key, loading: ED.loading, reading: ED.reading, lines: edAce && edAce.session.getLength(), row: edAce && edAce.getCursorPosition().row})`));
  check(chrome.thrown.length === 0, `the page threw nothing (${chrome.thrown.join(' | ')})`);
  if (process.env.SHOT) {
    await run(`sideReveal("files")`);
    await search('total');
    await settled((x) => x.spots.length >= 2);
    const shot = await chrome.send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(process.env.SHOT, 'project-search.png'), Buffer.from(shot.data, 'base64'));
    await run(`window.__openPalette("files")`);
    await type('rout');
    await sleep(1500);
    const shot2 = await chrome.send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(process.env.SHOT, 'quick-open.png'), Buffer.from(shot2.data, 'base64'));
  }
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (chrome) chrome.stop();
  if (!process.env.KEEP) stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
