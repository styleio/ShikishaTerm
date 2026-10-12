/**
 * Notes on the lines of a change, handed to an AI together.
 *
 * This checkout's build in a folder of its own: a git repository with one
 * file changed, a git panel, and a tab the app takes for an AI tab (Claude
 * Code's profile running only a shell, so nothing is spent), its board on
 * this machine only, opened in a headless Chrome. The change is opened, a
 * changed line is pressed with the menu button, and a note is written on it.
 * Checked:
 *   - each line shown carries the line of the file as it is now
 *   - the menu offers a note, and the question has a box to write it in
 *   - the note is kept, and drawn under the line it is about
 *   - handed to the AI tab, it lands in that tab's input as `path:line`, the
 *     words of the line and the note, and leaves the list
 *
 *     cargo build
 *     node tools/debug/diff-notes.win.mjs
 *
 * Needs Windows, Node, git and Chrome. Nothing of a copy somebody is using is
 * read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-diff-notes');
const APP = path.join(RUN, 'app');
const REPO = path.join(RUN, 'shop');
const PORT = 9474;
const TOKEN = 'diff-notes-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const git = (...args) => { const r = spawnSync('git', args, { cwd: REPO, encoding: 'utf8' }); if (r.status !== 0) die(r.stderr); };
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-diff-notes*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.mkdirSync(REPO, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// A file of twelve lines, committed; then line 5 changed and a line put in after line 9
const lines = Array.from({ length: 12 }, (_, i) => `price ${i + 1} = ${(i + 1) * 100}`);
git('init', '-q', '-b', 'main');
fs.writeFileSync(path.join(REPO, 'prices.txt'), lines.join('\n') + '\n');
git('add', '.');
git('-c', 'user.name=test', '-c', 'user.email=test@example.invalid', 'commit', '-q', '-m', 'prices');
const changed = [...lines];
changed[4] = 'price 5 = 999';
changed.splice(9, 0, 'price 9.5 = 950');
fs.writeFileSync(path.join(REPO, 'prices.txt'), changed.join('\n') + '\n');

const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en', agent_hooks: answered,
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Shop', id: 'shop', folders: [{ cwd: REPO, tabs: [
    { name: 'git', id: 'git', command: ['git'] },
    { name: 'helper', id: 'helper', command: ['claude'] },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
// The stand-in AI, found before any real one
const bin = path.join(RUN, 'bin');
fs.mkdirSync(bin, { recursive: true });
// It takes pasted text the way an AI CLI does (bracketed paste) and shows
// what it was given, which is what the check reads off its screen
fs.writeFileSync(path.join(bin, 'stand-in.mjs'),
  "process.stdout.write('\\x1b[?2004hready\\r\\n');\n" +
  "process.stdin.setRawMode && process.stdin.setRawMode(true);\n" +
  "process.stdin.on('data', d => process.stdout.write(d.toString().replace(/\\x1b\\[20[01]~/g, '').replace(/\\r?\\n/g, '\\r\\n')));\n");
fs.writeFileSync(path.join(bin, 'claude.cmd'), `@echo off\r\nnode "${path.join(bin, 'stand-in.mjs')}"\r\n`);
env.PATH = bin + ';' + (env.PATH || env.Path || '');
delete env.Path;
spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};

let chrome;
try {
  await until(async () => (await fetch(`${BASE}/?t=${TOKEN}`, { redirect: 'manual' })).status < 500, 'the board', 60000);
  chrome = await startChrome();
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  const page = (js) => chrome.run(js);
  await until(() => page(`!!(typeof S !== "undefined" && S && S.tabs && S.tabs.some(t => t.name === "git"))`), 'the board to draw', 30000);
  await page(`send({kind:"select", tab:S.tabs.find(t => t.name === "git").index}); true`);
  await sleep(1500);
  await page(`gitOpenDiff("prices.txt", "work"); true`);
  await until(() => page(`document.querySelectorAll("#editpanel .ediff [data-line]").length > 0`), 'the change to be drawn', 20000);

  const shown = JSON.parse(await page(`JSON.stringify([...document.querySelectorAll("#editpanel .ediff [data-line]")].map(s => [s.className, s.dataset.line, s.textContent.trim()]))`));
  const added = shown.find(([c, , t]) => c === 'a' && t === '+price 5 = 999');
  const later = shown.find(([c, , t]) => c === 'a' && t === '+price 9.5 = 950');
  check(added && added[1] === '5', `a changed line carries its line in the file as it is now (${added && added[1]})`);
  check(later && later[1] === '10', `a line put in later in the file carries its own (${later && later[1]})`);

  // The menu of the changed line, and a note written in the question's box
  const offered = await page(`(() => {
    const span = [...document.querySelectorAll("#editpanel .ediff [data-line]")].find(s => s.textContent.trim() === "+price 5 = 999");
    span.dispatchEvent(new MouseEvent("contextmenu", {bubbles: true, cancelable: true, clientX: 40, clientY: 40}));
    const row = [...document.querySelectorAll(".fmenu div")].find(d => d.textContent === T["md.notes.write"]);
    if (row) row.click();
    return !!row;
  })()`);
  check(offered, 'the line\'s menu offers a note for the AI');
  const box = await until(() => page(`(() => { const i = document.querySelector("#sask #sq"); return i && !document.getElementById("sask").hidden && !i.hidden && getComputedStyle(i).display !== "none"; })()`), 'the question with a box', 5000).catch(() => false);
  check(box, 'the question has a box to write the note in');
  await page(`(() => { const i = document.querySelector("#sask #sq"); i.value = "999 is a typo, it should be 500"; document.querySelector("#sask .brow > .go").click(); return true; })()`);
  const card = await until(() => page(`(() => { const c = document.querySelector("#editpanel .ediff .mdnote"); return c ? c.textContent : ""; })()`), 'the note to be drawn', 10000);
  check(card.includes('999 is a typo'), 'the note is drawn in the change');
  const under = await page(`(() => { const c = document.querySelector("#editpanel .ediff .mdnote"); const p = c && c.previousElementSibling; return p ? p.textContent.trim() : ""; })()`);
  check(under === '+price 5 = 999', `the note is under the line it is about ("${under}")`);
  const kept = JSON.parse(fs.readFileSync(path.join(APP, 'config', 'review-notes.json'), 'utf8')).notes;
  check(kept.length === 1 && kept[0].path === 'prices.txt' && kept[0].from === 5, `the note is kept for prices.txt line 5 (${JSON.stringify(kept.map((n) => [n.path, n.from, n.to]))})`);

  // Handed to the AI tab: it is in that tab's input, and off the list
  const handed = await page(`(() => {
    const go = [...document.querySelectorAll("#editpanel .ediff .mdnotes button")].find(b => b.textContent.startsWith(T["md.notes.send"]));
    if (!go) return false;
    go.click();
    const row = [...document.querySelectorAll(".mrow")].find(r => r.textContent.includes("helper"));
    if (row) row.click();
    return !!row;
  })()`);
  check(handed, 'the bar offers the AI tab');
  await page(`send({kind:"select", tab:S.tabs.find(t => t.name === "helper").index}); true`);
  const draft = await until(() => page(`(() => { const s = document.getElementById("screen").textContent || ""; return s.includes("999 is a typo") ? s : ""; })()`), 'the notes in the AI tab', 15000).catch(async () => 'said: ' + await page(`NT.said`));
  check(/prices\.txt:5\b/.test(draft) && draft.includes('price 5 = 999') && draft.includes('999 is a typo'), `the AI tab's input holds the place, the line and the note: ${JSON.stringify(draft.replace(/\s+/g, ' ').trim()).slice(0, 240)}`);
  await until(() => page(`!document.querySelector("#editpanel .ediff .mdnote")`), 'the note to leave the list', 10000).catch(() => {});
  check(!(await page(`!!document.querySelector("#editpanel .ediff .mdnote")`)), 'a handed note leaves the list');
  check(chrome.thrown.length === 0, `the page threw nothing (${chrome.thrown.join(' | ')})`);
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (chrome) chrome.stop();
  if (!process.env.KEEP) stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
