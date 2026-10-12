/**
 * A terminal's history is searched from the board, on a phone as well.
 *
 * This checkout's build in a folder of its own, with one terminal tab that
 * prints a long history holding a word on a few of its lines, and its board
 * on this machine only. A headless Chrome at a phone's width opens the board,
 * opens the search from the terminal tab's own menu (a phone has no keys for
 * it), types the word, and moves through the matches. Checked:
 *   - the search bar opens over the terminal
 *   - every line holding the word is counted, the newest stood on first
 *   - the line stood on is on screen and marked, and the mark moves with
 *     "previous" to an older line
 *   - closing the bar puts the view back at the newest output
 *
 *     cargo build
 *     node tools/debug/terminal-find.win.mjs
 *
 * Needs Windows, Node and Chrome. Nothing of a copy somebody is using is read,
 * written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-terminal-find');
const APP = path.join(RUN, 'app');
const PORT = 9472;
const TOKEN = 'terminal-find-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;
const WORD = 'NEEDLE';
const LINES = 400;
const EVERY = 90;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-terminal-find*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
// A history to search: LINES lines, every EVERY-th of them holding the word
const script = path.join(RUN, 'history.cmd');
fs.writeFileSync(script, `@echo off\r\nfor /L %%i in (1,1,${LINES}) do (\r\n  set /a m=%%i %% ${EVERY}\r\n  call :one %%i\r\n)\r\ngoto :eof\r\n:one\r\nset /a m=%1 %% ${EVERY}\r\nif %m%==0 (echo line %1 ${WORD}) else (echo line %1)\r\ngoto :eof\r\n`);
const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en', agent_hooks: answered,
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Find', id: 'find', folders: [{ cwd: RUN, tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe', '/k', script] }] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
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
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 820, deviceScaleFactor: 2, mobile: true });
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  const page = (js) => chrome.run(js);
  await until(() => page(`!!(typeof S !== "undefined" && S && S.tabs && S.tabs.some(t => t.name === "shell"))`), 'the board to draw', 30000);
  await until(() => page(`(document.getElementById("screen").textContent || "").includes("line ${LINES}")`), 'the history to be printed', 60000);
  const tab = await page(`S.tabs.find(t => t.name === "shell").index`);
  await page(`send({kind:"select", tab:${tab}}); true`);
  await sleep(600);

  // The terminal tab's own menu, as a phone opens it
  const offered = await page(`(() => {
    const t = S.tabs.find(x => x.name === "shell");
    tabMenu(document.body, t, "tabs");
    const row = [...document.querySelectorAll(".fmenu div")].find(d => d.textContent === T["tui.menu.find_terminal"]);
    if (row) row.click();
    return !!row;
  })()`);
  check(offered, 'the terminal tab\'s menu offers the search');
  await until(() => page(`!document.getElementById("seek").hidden`), 'the bar to open', 10000);
  check(true, 'the search bar opens over the terminal');

  await page(`(() => { const i = document.querySelector("#seek input"); i.value = "${WORD.toLowerCase()}"; i.dispatchEvent(new Event("input")); return true; })()`);
  const expected = Math.floor(LINES / EVERY);
  const first = await until(() => page(`S.seek && S.seek.of ? JSON.stringify(S.seek) : ""`), 'the matches to be counted', 10000);
  const s1 = JSON.parse(first);
  check(s1.of === expected, `every line holding the word is counted (${s1.of} of ${expected})`);
  check(s1.at === s1.of, `the newest is stood on first (${s1.at} of ${s1.of})`);
  const marked = async () => page(`(() => { const r = document.querySelector("#screen > .r.seekhit"); return r ? r.textContent.trim() : ""; })()`);
  const m1 = await until(marked, 'the line to be marked', 5000);
  check(m1 === `line ${expected * EVERY} ${WORD}`, `the newest line holding it is on screen and marked ("${m1}")`);
  check(await page(`document.querySelector("#seek .count").textContent.includes("${expected}")`), 'the bar says how many');

  await page(`seekMove(true); true`);
  await until(async () => JSON.parse(await page(`JSON.stringify(S.seek)`)).at === s1.of - 1, 'the move to the one before', 5000);
  const m2 = await until(async () => { const m = await marked(); return m !== m1 && m; }, 'the mark to move', 5000);
  check(m2 === `line ${(expected - 1) * EVERY} ${WORD}`, `"previous" moves the mark to an older line ("${m2}")`);

  await page(`seekClose(); true`);
  await until(() => page(`document.getElementById("seek").hidden`), 'the bar to close', 5000);
  await until(() => page(`(document.getElementById("screen").textContent || "").includes("line ${LINES}")`), 'the view to come back', 5000);
  check(!(await page(`!!document.querySelector("#screen > .r.seekhit")`)), 'closing takes the mark away and puts the newest output back in view');
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
