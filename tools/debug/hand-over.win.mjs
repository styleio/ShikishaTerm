/**
 * Continuing a conversation with another AI: a new tab in the same folder,
 * told where the old conversation's record is, and the old tab left alone.
 *
 * This checkout's build in a folder of its own, with a home folder of its
 * own. The AIs are stand-ins under Claude Code's and Codex CLI's names: the
 * Claude one writes a conversation record where Claude Code would (named by
 * the id the app hands it) and says it has hit its usage limit; the Codex
 * one keeps none, and writes down everything typed into it. The settings
 * hold one Codex account, "Spare". Checked, through the board in a browser
 * at a phone's width -- the same page the window shows:
 *   - the tab's menu offers it, and the dialog lists this folder's AIs and
 *     the account, with the record found
 *   - starting it opens a tab of the chosen AI as the chosen account in the
 *     same folder, and types in a prompt naming the record's path
 *   - the old tab is still there
 *   - the limit notice offers it too
 *   - a tab whose CLI keeps no record passes on the end of its screen
 *   - a past conversation, found by its id, can be continued the same way
 *   - the page throws nothing
 *
 *     cargo build
 *     node tools/debug/hand-over.win.mjs
 *
 * Takes about two minutes. Needs Windows, Node and Chrome. Nothing of a copy
 * somebody is using, nor this PC's own AI folders, is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-hand-over');
const APP = path.join(RUN, 'app');
const HOME = path.join(RUN, 'home');
const WORK = path.join(RUN, 'work');
const SAID = path.join(RUN, 'said');
const PORT = 9483;
const TOKEN = 'hand-over-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'msedgewebview2.exe' -or $_.Name -eq 'node.exe') -and ($_.CommandLine + '') -like '*sk-hand-over*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [path.join(APP, 'config'), WORK, SAID, path.join(HOME, '.claude'), path.join(HOME, '.codex')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.writeFileSync(path.join(HOME, '.claude.json'), JSON.stringify({ hasCompletedOnboarding: true, projects: {} }));

// The stand-ins. Claude's writes its record under the id it was started
// with; both write down what is typed into them, by the tab's name
const bin = path.join(RUN, 'bin');
fs.mkdirSync(bin, { recursive: true });
const standIn = (name, body) => {
  fs.writeFileSync(path.join(bin, name + '.cjs'), `
const fs = require('fs'), path = require('path');
const tab = process.env.SHIKISHA_TAB || 'unknown';
const said = path.join(${JSON.stringify(SAID)}, tab + '.txt');
fs.appendFileSync(said, '');
${body}
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.on('data', (b) => fs.appendFileSync(said, b.toString('utf8')));
process.stdin.resume();
setInterval(() => {}, 1000);
`);
  fs.writeFileSync(path.join(bin, name + '.cmd'), `@echo off\r\nnode "${path.join(bin, name + '.cjs')}" %*\r\n`);
};
standIn('claude', `
const i = process.argv.indexOf('--session-id');
const id = i > 0 ? process.argv[i + 1] : '';
if (id) {
  const dir = path.join(process.env.USERPROFILE, '.claude', 'projects', 'C--work');
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, id + '.jsonl'), JSON.stringify({ type: 'user', cwd: ${JSON.stringify(WORK)}, sessionId: id, message: { role: 'user', content: 'fix the login page' } }) + '\\n');
  fs.writeFileSync(path.join(${JSON.stringify(SAID)}, tab + '.id'), id);
}
process.stdout.write('Claude stand-in. Asked: fix the login page\\r\\n');
process.stdout.write('Working on the login form...\\r\\n');
setTimeout(() => process.stdout.write("You've hit your limit · resets 5pm\\r\\n> "), 1500);
`);
standIn('codex', `
process.stdout.write('Codex stand-in in ' + process.cwd() + '\\r\\nCHECKPOINT-ON-SCREEN\\r\\n> ');
`);

const config = {
  language: 'en', agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off' },
  // On, so each tab is told who it is (SHIKISHA_TAB), which the stand-ins
  // name what they write down after
  external_api: { access: 'user' },
  ai_accounts: [{ id: 'spare', cli: 'codex', name: 'Spare' }],
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'One', id: 'one', folders: [{ cwd: WORK, tabs: [
    { name: 'fixer', id: 'fixer', command: ['claude'] },
  ] }] }],
};
const configFile = path.join(APP, 'config', 'config.json');
fs.writeFileSync(configFile, JSON.stringify(config, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX|OPENAI)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.USERPROFILE = HOME;
env.HOME = HOME;
env.PATH = bin + ';' + (env.PATH || env.Path || '');
delete env.Path;
spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const jar = new Map();
const send = async (url, init = {}) => {
  const r = await fetch(BASE + url, { redirect: 'manual', ...init,
    headers: { ...(init.headers || {}), Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') },
    signal: AbortSignal.timeout(15000) });
  for (const kv of r.headers.getSetCookie().map((c) => c.split(';')[0])) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return r;
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(800); }
  throw new Error('timed out waiting for ' + what);
};
const said = (tab) => { const f = path.join(SAID, tab + '.txt'); return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : ''; };

let chrome;
try {
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500 && jar.has('rs'), 'the board', 60000);
  const id = await until(() => { const f = path.join(SAID, 'fixer.id'); return fs.existsSync(f) && fs.readFileSync(f, 'utf8'); }, 'the Claude stand-in to start', 30000);
  const record = path.join(HOME, '.claude', 'projects', 'C--work', id + '.jsonl');

  chrome = await startChrome();
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 420, height: 900, deviceScaleFactor: 2, mobile: true });
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  await until(() => chrome.run(`typeof S !== "undefined" && !!S && (S.tabs || []).some(t => t.id === "fixer")`), 'the board in the browser', 30000);
  const run = (js) => chrome.run(js);
  // The dialog, as the page has it: open, its options, what is chosen
  const dialog = () => run(`(() => { const b = document.getElementById("sask"); if (!b || b.hidden) return null;
    const sel = [...b.querySelectorAll("select")];
    return { title: b.querySelector(".vtitle").textContent, text: b.querySelector(".blist").textContent,
      ais: sel[0] ? [...sel[0].options].map(o => o.textContent) : [], pick: sel[0] ? sel[0].selectedOptions[0].textContent : "",
      depth: sel[1] ? [...sel[1].options].map(o => o.value) : [] }; })()`);
  const choose = (label, depth) => run(`(() => { const b = document.getElementById("sask"); const sel = b.querySelectorAll("select");
    const o = [...sel[0].options].find(o => o.textContent === ${JSON.stringify(label)}); if (!o) return false;
    sel[0].value = o.value; if (sel[1] && ${JSON.stringify(depth || '')}) sel[1].value = ${JSON.stringify(depth || '')};
    b.querySelector(".brow > .go").click(); return true; })()`);

  // The tab's menu offers it
  const item = await until(() => run(`(() => { const t = S.tabs.find(t => t.id === "fixer"); tabMenu(document.body, t, "tabs");
    const it = [...document.querySelectorAll(".fmenu div")].find(d => d.textContent === "Continue with another AI…");
    if (!it) return ""; it.click(); return "clicked"; })()`), 'the menu item', 15000).catch(() => '');
  check(item === 'clicked', 'the AI tab\'s menu offers "Continue with another AI…"');
  const d1 = await until(async () => { const d = await dialog(); return d && d.ais.length && d; }, 'the dialog to fill in', 20000).catch(() => null);
  check(d1 && d1.ais.includes('Codex CLI') && d1.ais.includes('Codex CLI (Spare)') && d1.ais.includes('Claude Code (this tab\'s)'),
    `the dialog lists this folder's AIs and the account (${d1 && d1.ais.join(', ')})`);
  check(d1 && d1.pick !== 'Claude Code (this tab\'s)', `another AI is chosen to begin with, in plain sight (${d1 && d1.pick})`);
  check(d1 && d1.depth.join() === 'brief,whole', 'the record was found, so how much to read is asked');
  check(d1 && d1.text.includes('fixer') && d1.text.includes(WORK), 'the dialog says which conversation, and its folder');
  // SHOT=<folder>: the dialog as drawn, for a person to look at
  if (process.env.SHOT) {
    const shot = await chrome.send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(process.env.SHOT, 'hand-over-dialog.png'), Buffer.from(shot.data, 'base64'));
  }

  // Started: a Codex tab as Spare, in the same folder, told the record's path
  check(await choose('Codex CLI (Spare)', 'brief'), 'the account is chosen and the dialog started');
  const opened = await until(async () => run(`(S.tabs || []).find(t => /Spare/.test(t.name)) || null`), 'the new tab', 30000).catch(() => null);
  check(!!opened, `a new tab opens (${opened && opened.name})`);
  const typed = opened ? await until(() => { const s = said(opened.id || opened.name); return s.includes('Reply in') && s; }, 'the prompt to be typed in', 120000).catch(() => '') : '';
  check(typed.includes(record), 'the prompt names the record\'s path');
  check(typed.includes('Start from the end') && !typed.includes('Read all of it'), 'and says to start from its end');
  check(!typed.includes('Working on the login form'), 'the screen is not pasted when there is a record');
  const written = JSON.parse(fs.readFileSync(configFile, 'utf8'));
  const tabsWritten = written.desks[0].folders.flatMap((f) => f.tabs.map((t) => ({ ...t, cwd: f.cwd })));
  const line = tabsWritten.find((t) => /Spare/.test(t.name)) || {};
  check(line.ai_account === 'spare' && /codex/.test(JSON.stringify(line.command)) && path.resolve(line.cwd || '') === path.resolve(WORK),
    `it is written as Codex, as Spare, in the same folder (${JSON.stringify(line)})`);
  check(tabsWritten.some((t) => t.id === 'fixer') && (await run(`S.tabs.some(t => t.id === "fixer")`)), 'the old tab is still there');

  // The limit notice offers it too
  await run(`send({kind:"select", tab: S.tabs.find(t => t.id === "fixer").index})`);
  const pill = await until(() => run(`(() => { const p = document.querySelector(".pill.handon"); return p ? p.textContent : ""; })()`), 'the limit notice', 30000).catch(() => '');
  check(pill === 'Continue with another AI', `the limit notice offers it (${pill})`);
  const inSight = await run(`(() => { const r = document.querySelector(".pill.handon").getBoundingClientRect(); return r.width > 0 && r.left >= 0 && r.right <= innerWidth; })()`).catch(() => false);
  check(inSight, 'and it is in sight on a phone\'s width, not behind the notice\'s long words');
  if (process.env.SHOT) {
    const shot = await chrome.send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(process.env.SHOT, 'hand-over-limit.png'), Buffer.from(shot.data, 'base64'));
  }
  if (pill) {
    await run(`document.querySelector(".pill.handon").click()`);
    const d2 = await until(async () => { const d = await dialog(); return d && d.ais.length && d; }, 'the dialog from the notice', 20000).catch(() => null);
    check(!!d2, 'pressing it opens the same dialog');
    await run(`closeAsk()`);
  }

  // A CLI that keeps no record: the end of its screen goes instead
  const codexTab = opened && (opened.id || opened.name);
  await run(`(() => { const t = S.tabs.find(t => (t.id || t.name) === ${JSON.stringify(codexTab)}); tabMenu(document.body, t, "tabs");
    [...document.querySelectorAll(".fmenu div")].find(d => d.textContent === "Continue with another AI…").click(); })()`);
  const d3 = await until(async () => { const d = await dialog(); return d && d.ais.length && d; }, 'the dialog for the Codex tab', 20000).catch(() => null);
  check(d3 && d3.depth.length === 0 && /record was not found/.test(d3.text), 'with no record, the dialog says the screen is passed on instead');
  check(d3 && d3.pick !== 'Codex CLI (Spare) (this tab\'s)' && d3.ais.includes('Codex CLI (Spare) (this tab\'s)'), `the tab's own AI and account is marked, and not chosen (${d3 && d3.pick})`);
  check(d3 && await choose('Claude Code'), 'Claude Code is chosen and started');
  const back = await until(async () => run(`(S.tabs || []).find(t => t.name === "Claude Code") || null`), 'the Claude tab', 30000).catch(() => null);
  const typedBack = back ? await until(() => { const s = said(back.id || back.name); return s.includes('Reply in') && s; }, 'the prompt in the Claude tab', 120000).catch(() => '') : '';
  check(typedBack.includes('No saved record') && typedBack.includes('CHECKPOINT-ON-SCREEN'), 'the end of the Codex tab\'s screen is passed on');

  // A past conversation, by its id
  await run(`openHandOver({program:"claude", id:${JSON.stringify(id)}, cwd:${JSON.stringify(WORK)}, host:"", title:"work"})`);
  const d4 = await until(async () => { const d = await dialog(); return d && d.ais.length && d; }, 'the dialog for a past conversation', 20000).catch(() => null);
  check(d4 && d4.depth.join() === 'brief,whole', 'a past conversation\'s record is found');
  check(d4 && await choose('Codex CLI', 'whole'), 'Codex is chosen to read the whole of it');
  const past = await until(async () => run(`(S.tabs || []).find(t => t.name === "Codex CLI") || null`), 'the tab for the past one', 30000).catch(() => null);
  const typedPast = past ? await until(() => { const s = said(past.id || past.name); return s.includes('Reply in') && s; }, 'the prompt for the past one', 120000).catch(() => '') : '';
  check(typedPast.includes(record) && typedPast.includes('Read all of it'), 'the past one\'s prompt names its record and says to read all of it');

  // A conversation that cannot be continued says why, where the choices go
  await run(`openHandOver({program:"claude", id:"no-such-id", cwd:${JSON.stringify(WORK)}, host:"", title:"gone"})`);
  const d5 = await until(() => run(`(() => { const w = document.querySelector("#sask .bwarn"); return w ? w.textContent : ""; })()`), 'the refusal', 20000).catch(() => '');
  check(/nothing to continue from/.test(d5), `a conversation with no record says why (${d5})`);
  await run(`closeAsk()`);
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
