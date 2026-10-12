/**
 * Scheduled prompts, filled in on the settings' form: handed to an AI by the
 * app's clock or by "Run now", answered, and written down.
 *
 * This checkout's build in a folder of its own, its board on this machine
 * only. The "AI" is a stand-in under Claude Code's name (nothing is spent):
 * it takes a prompt, shows itself at work for a few seconds -- forty for a
 * prompt with SLOW in it -- and answers "ANSWER: <the prompt>". The settings
 * hold five prompts: one every minute in a tab of its own, two for an AI tab
 * that is open, one slow one in a tab of its own, and one with no prompt
 * written. Checked:
 *   - the every-minute prompt comes round by the clock, opens a tab of its
 *     own, is answered, and the answer is written down; the tab is closed
 *     once it is answered, nobody having touched it
 *   - "Run now" through the phone's door to the settings hands a prompt to
 *     the open AI tab; the answer is written down and the tab stays
 *   - a prompt asked to run while its last run is still going is skipped,
 *     and the reason written down
 *   - a prompt for a tab that is busy is skipped, not typed into the work
 *   - a prompt that cannot run is refused with the reason, not queued
 *   - with automation stopped, nothing runs by the clock, and "Run now"
 *     still runs
 *   - the settings page draws the list, the times and the history, and
 *     throws nothing
 *
 *     cargo build
 *     node tools/debug/routine.win.mjs
 *
 * Takes about six minutes. Needs Windows, Node and Chrome. Nothing of a copy
 * somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { startChrome } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-routine');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const PORT = 9479;
const TOKEN = 'routine-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;
const DESK = '6f1e2c3a-0b7d-4c55-9a10-2b3c4d5e6f70';
const HELPER = '0a1b2c3d-4e5f-4a6b-8c7d-8e9f0a1b2c3d';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-routine*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'node.exe' -and ($_.CommandLine + '') -like '*sk-routine*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.mkdirSync(WORK, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// The stand-in AI, found before any real one. It reads what it is given
// (pasted or typed), and on Enter is at work for a while -- saying so the
// way Claude Code does, which is what the board reads as busy -- then answers
const bin = path.join(RUN, 'bin');
fs.mkdirSync(bin, { recursive: true });
fs.writeFileSync(path.join(bin, 'stand-in.mjs'), `
const out = (s) => process.stdout.write(s);
out('\\x1b[?2004hready\\r\\n> ');
process.stdin.setRawMode && process.stdin.setRawMode(true);
let line = '', working = false;
process.stdin.on('data', (d) => {
  if (working) return;
  const text = d.toString().replace(/\\x1b\\[20[01]~/g, '');
  for (const ch of text) {
    if (ch !== '\\r' && ch !== '\\n') { line += ch; out(ch); continue; }
    if (!line.trim()) continue;
    const said = line.trim().replace(/\\s+/g, ' ');
    line = '';
    working = true;
    out('\\r\\n* Working (esc to interrupt)');
    setTimeout(() => { out('\\r\\x1b[2KANSWER: ' + said + '\\r\\n> '); working = false; }, said.includes('SLOW') ? 40000 : 4000);
  }
});
`);
fs.writeFileSync(path.join(bin, 'claude.cmd'), `@echo off\r\nnode "${path.join(bin, 'stand-in.mjs')}"\r\n`);

const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
const far = { repeat: 'days', days: [0, 1, 2, 3, 4, 5, 6], time: '03:33' };
const config = (routines) => ({
  language: 'en', agent_hooks: answered,
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Clock', id: 'clock', uid: DESK, folders: [{ cwd: WORK, tabs: [
    { name: 'shell', id: 'shell', command: ['cmd.exe'] },
    { name: 'helper', id: 'helper', uid: HELPER, command: ['claude'] },
  ] }] }],
  routines,
});
const routines = [
  { id: 'ping', name: 'Ping', on: true, prompt: 'hello from the clock', ai: 'claude', desk: DESK, folder: WORK,
    when: { repeat: 'every', minutes: 1 }, late: 60, tidy: true },
  { id: 'check', name: 'Check', on: true, prompt: 'status please', desk: DESK, folder: WORK, tab: HELPER, when: far, late: 60 },
  { id: 'hold', name: 'Hold', on: true, prompt: 'SLOW work for the helper', desk: DESK, folder: WORK, tab: HELPER, when: far, late: 60 },
  { id: 'slow', name: 'Slow', on: true, prompt: 'SLOW work alone', ai: 'claude', desk: DESK, folder: WORK, when: far, late: 60, tidy: false },
  { id: 'blank', name: 'Blank', on: true, prompt: '', desk: DESK, folder: WORK, when: far, late: 60 },
];
const configFile = path.join(APP, 'config', 'config.json');
fs.writeFileSync(configFile, JSON.stringify(config(routines), null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.PATH = bin + ';' + (env.PATH || env.Path || '');
delete env.Path;
spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
const began = Date.now();

const jar = new Map();
const send = async (url, init = {}) => {
  const r = await fetch(BASE + url, { redirect: 'manual', ...init,
    headers: { ...(init.headers || {}), Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') },
    signal: AbortSignal.timeout(15000) });
  for (const kv of r.headers.getSetCookie().map((c) => c.split(';')[0])) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return r;
};
const post = async (url, body) => (await send(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })).json();
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(1000); }
  throw new Error('timed out waiting for ' + what);
};
const book = () => { const f = path.join(APP, 'data', 'routine-runs.json'); return fs.existsSync(f) ? JSON.parse(fs.readFileSync(f, 'utf8')).runs || {} : {}; };
const runs = (id) => book()[id] || [];
const tabs = async () => { const s = await (await send(`/api/state?t=${TOKEN}`)).json(); return ((s.ui || s).tabs || []); };
const log = () => { const f = path.join(APP, 'logs', 'hooks.log'); return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : ''; };

let chrome;
try {
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500 && jar.has('rs'), 'the board', 60000);
  await until(async () => (await tabs()).some((t) => t.name === 'helper' && t.state !== 'BUSY'), 'the helper tab', 30000);
  // The settings, through the board, as a phone reaches them
  await send(`/cfg?t=${TOKEN}`);
  const listed = await until(async () => { const r = await send('/api/routines'); return r.status === 200 && (await r.json()); }, 'the settings', 30000);
  check(listed.next && listed.next.ping && !listed.next.blank, `the settings say when each prompt that can run comes next (${Object.keys(listed.next || {}).join(', ')})`);

  // By the clock: a minute after it was first seen, in a tab of its own
  const first = await until(() => runs('ping').find((r) => r.how === 'answered'), 'the every-minute prompt to be answered', 150000);
  const took = (Date.now() - began) / 1000;
  check(first.due && /ANSWER: hello from the clock/.test(first.reply || ''), `the clock ran it and the answer is written down (${JSON.stringify(first.reply)})`);
  check(took >= 55 && took <= 140, `it came round about a minute after it was first seen (${took.toFixed(0)} s after the start)`);
  check(first.title === 'Ping', `it went to a tab of its own (${first.title})`);
  await until(async () => !(await tabs()).some((t) => t.name === 'Ping'), 'the tab to be closed', 20000).catch(() => {});
  check(!(await tabs()).some((t) => t.name === 'Ping'), 'the tab opened for it is closed once it is answered');
  check(!JSON.stringify(JSON.parse(fs.readFileSync(configFile, 'utf8')).desks).includes('"Ping"'), 'the closed tab is gone from the settings');
  // It goes on by the clock; stopped here so the rest is read plainly
  const both = JSON.parse(fs.readFileSync(configFile, 'utf8'));
  both.routines.find((r) => r.id === 'ping').on = false;
  fs.writeFileSync(configFile, JSON.stringify(both, null, 2));
  await sleep(3000);

  // Run now, for the open AI tab
  const ran = await post('/api/routines/run', { id: 'check' });
  check(ran.ok === true, `"Run now" is taken (${JSON.stringify(ran)})`);
  const checked = await until(() => runs('check').find((r) => r.how === 'answered'), 'the open tab to answer', 40000);
  check(!checked.due && /ANSWER: status please/.test(checked.reply || '') && checked.tab === HELPER, `the open tab answered and it is written down (${JSON.stringify(checked.reply)})`);
  await sleep(3000);
  check((await tabs()).some((t) => t.name === 'helper'), 'the open tab stays open');

  // A prompt that cannot run is refused, with the reason
  const blank = await post('/api/routines/run', { id: 'blank' });
  check(!blank.ok && /prompt/i.test(blank.error || ''), `a prompt with nothing written is refused (${JSON.stringify(blank)})`);
  const unknown = await post('/api/routines/run', { id: 'nobody' });
  check(!unknown.ok && /save/i.test(unknown.error || ''), `a prompt not in the saved settings is refused (${JSON.stringify(unknown)})`);

  // One run at a time
  check((await post('/api/routines/run', { id: 'slow' })).ok === true, 'the slow prompt is taken');
  await until(() => runs('slow').some((r) => r.how === 'going'), 'the slow run to start', 15000);
  await sleep(1500);
  await post('/api/routines/run', { id: 'slow' });
  const skipped = await until(() => runs('slow').find((r) => r.how === 'skipped'), 'the second run to be skipped', 15000).catch(() => null);
  check(skipped && skipped.why === 'routine.why.last_going', `asked again while it is going, it is skipped and says why (${skipped && skipped.why})`);

  // A busy tab is not typed into
  check((await post('/api/routines/run', { id: 'hold' })).ok === true, 'the helper is set to slow work');
  await until(async () => (await tabs()).some((t) => t.name === 'helper' && t.state === 'BUSY'), 'the helper to be busy', 30000);
  const before = runs('check').length;
  await post('/api/routines/run', { id: 'check' });
  const busy = await until(() => runs('check').length > before && runs('check').at(-1), 'the run for the busy tab', 15000).catch(() => null);
  check(busy && busy.how === 'skipped' && busy.why === 'routine.why.busy', `a prompt for a busy tab is skipped (${busy && busy.how}, ${busy && busy.why})`);
  const typed = log().split('\n').filter((l) => /scheduled prompt "Check" run/.test(l)).length;
  check(typed === 1, `nothing was typed into the busy tab (${typed} run(s) of Check went in)`);

  // The brake: nothing by the clock, Run now still
  const lit = JSON.parse(fs.readFileSync(configFile, 'utf8'));
  lit.routines.find((r) => r.id === 'ping').on = true;
  fs.writeFileSync(configFile, JSON.stringify(lit, null, 2));
  await send(`/api/auto?t=${TOKEN}`, { method: 'POST', body: JSON.stringify({ on: false }) });
  const pings = runs('ping').length;
  await until(() => runs('hold').some((r) => r.how === 'answered') && runs('slow').some((r) => r.how === 'answered'), 'the slow work to be over', 90000);
  await sleep(Math.max(0, 80000 - 40000));
  check(runs('ping').length === pings, `with automation stopped, nothing ran by the clock (${pings} -> ${runs('ping').length})`);
  const braked = await post('/api/routines/run', { id: 'check' });
  const after = await until(() => runs('check').find((r) => r.how === 'answered' && r.n > checked.n), 'Run now with the brake on', 40000).catch(() => null);
  check(braked.ok && after, 'with automation stopped, "Run now" still runs');

  // The settings page draws it all
  chrome = await startChrome();
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 420, height: 900, deviceScaleFactor: 2, mobile: true });
  // The board first, as a phone opens it: the settings are reached through it
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  await until(() => chrome.run(`typeof S !== "undefined" && !!S`), 'the board in the browser', 30000);
  await chrome.send('Page.navigate', { url: `${BASE}/cfg?section=routines` });
  const drawn = await until(() => chrome.run(`(() => { const r = document.querySelectorAll(".rt"); return r.length ? JSON.stringify([...r].map(x => x.querySelector(".rtname").textContent + " | " + x.querySelector(".rtsum").textContent + " | " + ((x.querySelector(".rtstate") || {}).textContent || ""))) : ""; })()`), 'the list', 30000).catch(() => '');
  console.log('    ' + drawn);
  const rows = drawn ? JSON.parse(drawn) : [];
  check(rows.length === 5, `the page lists every prompt (${rows.length})`);
  check(rows.some((r) => r.startsWith('Check | ') && /Answered$/.test(r)), 'a prompt\'s line says how it last went');
  check(rows.some((r) => r.startsWith('Ping | Every 1 min')), 'a prompt\'s line says when it comes round');
  const history = await chrome.run(`(() => { const row = [...document.querySelectorAll(".rt")].find(x => x.querySelector(".rtname").textContent === "Check"); [...row.querySelectorAll("button")].find(b => b.textContent.startsWith("History")).click(); return new Promise(ok => setTimeout(() => ok(JSON.stringify([...document.querySelectorAll(".rtrun")].map(x => x.textContent.slice(0, 80)))), 300)); })()`);
  check(/ANSWER: status please/.test(history) && /busy/.test(history), `the history shows the answers and the reasons (${history.slice(0, 200)})`);
  const form = await chrome.run(`(() => { const row = [...document.querySelectorAll(".rt")].find(x => x.querySelector(".rtname").textContent === "Blank"); [...row.querySelectorAll("button")].find(b => b.textContent === "Edit").click(); return new Promise(ok => setTimeout(() => ok(!!document.querySelector(".rtform textarea") && !!document.querySelector(".rt .rtwarn")), 300)); })()`);
  check(form, 'the form opens, and a prompt that cannot run says why');
  const wide = await chrome.run(`document.documentElement.scrollWidth <= window.innerWidth + 1`);
  check(wide, 'nothing runs off the side of a phone\'s screen');
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
